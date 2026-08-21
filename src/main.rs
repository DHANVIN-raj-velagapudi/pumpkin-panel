mod affinity;
mod api;
mod audit;
mod backup;
mod crypto;
mod auth;
mod db;
mod error;
mod metrics;
mod middleware;
mod query;
mod security;
mod supervisor;
mod web;

use axum::Router;
use security::{LoginThrottle, RateLimiter};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use supervisor::Supervisor;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::SqlitePool,
    pub sup: Arc<Supervisor>,
    pub limiter: Arc<RateLimiter>,
    pub throttle: Arc<LoginThrottle>,
    /// Serialises audit appends so the hash chain cannot fork.
    pub audit_lock: Arc<tokio::sync::Mutex<()>>,
    /// Whether `X-Forwarded-For` may be believed.
    pub trust_proxy: bool,
    /// Whether cookies should carry the `Secure` flag.
    pub secure_cookies: bool,
    /// The panel's own data directory, where backups and the database live.
    pub data_dir: PathBuf,
    /// Key for secrets that must be readable again, such as TOTP seeds.
    pub secret_key: Arc<[u8; 32]>,
    /// `host:port` of a clamd daemon, when virus scanning is switched on.
    pub clamav: Option<String>,
}

struct Settings {
    bind: SocketAddr,
    data_dir: PathBuf,
    admin_user: String,
    admin_password: Option<String>,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
    trust_proxy: bool,
    /// Set when the panel sits behind a TLS-terminating proxy.
    behind_tls_proxy: bool,
    /// Address of a clamd daemon, if uploads should be virus scanned.
    clamav: Option<String>,
}

impl Settings {
    fn from_env() -> anyhow::Result<Self> {
        let bind = std::env::var("PANEL_BIND")
            .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
            .parse()?;

        let flag = |name: &str| {
            std::env::var(name)
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false)
        };

        Ok(Self {
            bind,
            data_dir: std::env::var("PANEL_DATA_DIR")
                .unwrap_or_else(|_| "./data".to_string())
                .into(),
            admin_user: std::env::var("PANEL_ADMIN_USER").unwrap_or_else(|_| "admin".to_string()),
            admin_password: std::env::var("PANEL_ADMIN_PASSWORD").ok(),
            tls_cert: std::env::var("PANEL_TLS_CERT").ok().map(PathBuf::from),
            tls_key: std::env::var("PANEL_TLS_KEY").ok().map(PathBuf::from),
            trust_proxy: flag("PANEL_TRUST_PROXY"),
            behind_tls_proxy: flag("PANEL_BEHIND_TLS_PROXY"),
            clamav: std::env::var("PANEL_CLAMAV").ok().filter(|v| !v.trim().is_empty()),
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("PANEL_LOG")
                .unwrap_or_else(|_| EnvFilter::new("pumpkin_panel=info,tower_http=warn")),
        )
        .init();

    let settings = Settings::from_env()?;
    tokio::fs::create_dir_all(&settings.data_dir).await?;

    // `pumpkin-panel recover <username>` is the deliberate way back in when
    // both the password and the recovery codes are gone. It is not an HTTP
    // endpoint on purpose: running it requires a shell on this machine, so a
    // remote attacker cannot reach it, while the actual owner always can.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("recover") {
        let Some(username) = args.get(2) else {
            eprintln!("usage: pumpkin-panel recover <username>");
            std::process::exit(2);
        };
        let pool = db::connect(&settings.data_dir.join("panel.db")).await?;
        return recover_account(pool, username).await;
    }

    let tls = match (&settings.tls_cert, &settings.tls_key) {
        (Some(cert), Some(key)) => Some((cert.clone(), key.clone())),
        (Some(_), None) | (None, Some(_)) => {
            anyhow::bail!("PANEL_TLS_CERT and PANEL_TLS_KEY must both be set");
        }
        (None, None) => None,
    };

    // The supervisor reports unexpected exits here; the task below turns them
    // into audit records once the state exists.
    let (crash_tx, mut crash_rx) = tokio::sync::mpsc::channel(32);

    let state = AppState {
        db: db::connect(&settings.data_dir.join("panel.db")).await?,
        sup: Arc::new(Supervisor::with_crash_reporting(crash_tx)),
        // Roughly 240 requests a minute sustained, with a burst of 120. The UI
        // polls a handful of endpoints every five seconds, so this leaves
        // plenty of headroom while still stopping a scripted flood.
        limiter: Arc::new(RateLimiter::new(120.0, 4.0)),
        throttle: Arc::new(LoginThrottle::new()),
        audit_lock: Arc::new(tokio::sync::Mutex::new(())),
        trust_proxy: settings.trust_proxy,
        secure_cookies: tls.is_some() || settings.behind_tls_proxy,
        data_dir: settings.data_dir.clone(),
        secret_key: Arc::new(crypto::load_or_create_key(&settings.data_dir)?),
        clamav: settings.clamav.clone(),
    };

    if let Some(address) = &state.clamav {
        tracing::info!(%address, "uploads will be scanned by clamd");
    }

    // On a fresh install, create the first administrator and print the password
    // once. It is never stored anywhere in plain text.
    let generated = settings
        .admin_password
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()[..16].to_string());

    if api::session::ensure_bootstrap_admin(&state, &settings.admin_user, &generated).await? {
        tracing::info!("created the first administrator account");
        println!();
        println!("  ┌──────────────────────────────────────────────┐");
        println!("  │  Pumpkin Panel is ready                      │");
        println!("  ├──────────────────────────────────────────────┤");
        println!("  │  username : {:<32} │", settings.admin_user);
        println!("  │  password : {:<32} │", generated);
        println!("  └──────────────────────────────────────────────┘");
        println!("  Save that password now - it is not shown again.");
        println!();
    }

    {
        let state = state.clone();
        tokio::spawn(async move {
            while let Some(event) = crash_rx.recv().await {
                let name = api::load_server(&state, &event.server_id)
                    .await
                    .map(|s| s.name)
                    .unwrap_or_else(|_| event.server_id.clone());

                // The server is gone, so it must not be adopted next time.
                let _ = sqlx::query("DELETE FROM running_servers WHERE server_id = ?1")
                    .bind(&event.server_id)
                    .execute(&state.db)
                    .await;

                if !event.crashed {
                    continue;
                }

                tracing::warn!(server = %name, code = ?event.exit_code, "server exited unexpectedly");

                audit::record(
                    &state,
                    audit::Event::new("SERVER_CRASHED", audit::Category::Servers, audit::Outcome::Failure)
                        .server(&event.server_id)
                        .target(&name)
                        .detail(match event.exit_code {
                            Some(code) => format!("exited unexpectedly with code {code}"),
                            None => "terminated without a clean shutdown".to_string(),
                        }),
                )
                .await;
            }
        });
    }

    // Scheduled snapshots and retention run for the life of the process.
    tokio::spawn(api::backups::run_scheduler(state.clone()));

    reattach_running_servers(&state).await;

    api::servers::run_autostart(&state).await;

    let app = Router::new()
        .nest("/api", api::router())
        .fallback(web::static_handler)
        .layer(axum::middleware::from_fn(middleware::security_headers))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit,
        ))
        .with_state(state.clone());

    // ConnectInfo is required so the rate limiter can see who is calling.
    let service = app.into_make_service_with_connect_info::<SocketAddr>();

    match tls {
        Some((cert, key)) => {
            // rustls 0.23 refuses to pick a crypto backend on its own, and
            // panics deep inside the TLS handshake setup if none is installed.
            // Doing it here means a missing provider can never take the panel
            // down at startup.
            if rustls::crypto::ring::default_provider()
                .install_default()
                .is_err()
            {
                tracing::debug!("a rustls crypto provider was already installed");
            }

            tracing::info!("panel listening on https://{}", settings.bind);
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("could not load TLS certificate {}: {e}", cert.display())
                })?;

            // axum-server owns the accept loop, so graceful shutdown is handled
            // by its own handle rather than axum::serve.
            let handle = axum_server::Handle::new();
            let shutdown_handle = handle.clone();
            let shutdown_state = state.clone();
            tokio::spawn(async move {
                shutdown(shutdown_state).await;
                shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
            });

            axum_server::bind_rustls(settings.bind, config)
                .handle(handle)
                .serve(service)
                .await?;
        }
        None => {
            let listener = tokio::net::TcpListener::bind(settings.bind).await?;
            tracing::info!("panel listening on http://{}", settings.bind);
            if !settings.behind_tls_proxy {
                tracing::warn!(
                    "running without TLS; set PANEL_TLS_CERT and PANEL_TLS_KEY, or put the panel behind an HTTPS proxy, before exposing it beyond your network"
                );
            }
            axum::serve(listener, service)
                .with_graceful_shutdown(shutdown(state))
                .await?;
        }
    }

    Ok(())
}

/// Picks up servers that were left running when the panel last stopped.
///
/// Without this, a panel restart would orphan every world: the processes keep
/// running but nothing is watching them, and pressing Start would try to bind
/// ports that are already taken.
async fn reattach_running_servers(state: &AppState) {
    use sqlx::Row;

    let rows = match sqlx::query("SELECT server_id, pid, started_at, binary_path FROM running_servers")
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "could not read previously running servers");
            return;
        }
    };

    for row in rows {
        let server_id: String = row.get("server_id");
        let pid = row.get::<i64, _>("pid") as u32;
        let started_at: i64 = row.get("started_at");
        let binary: String = row.get("binary_path");

        let path = PathBuf::from(&binary);
        let alive = tokio::task::spawn_blocking(move || metrics::process_matches(pid, &path))
            .await
            .unwrap_or(false);

        if !alive {
            // Either it exited while the panel was down, or the id now belongs
            // to something else entirely. Either way there is nothing to adopt.
            let _ = sqlx::query("DELETE FROM running_servers WHERE server_id = ?1")
                .bind(&server_id)
                .execute(&state.db)
                .await;
            continue;
        }

        let Ok(server) = api::load_server(state, &server_id).await else {
            continue;
        };

        let instance = state.sup.instance(&server_id).await;
        match instance.adopt(&server.spec(), pid, started_at).await {
            Ok(()) => {
                tracing::info!(server = %server.name, pid, "reattached to a running server");
                audit::record(
                    state,
                    audit::Event::new(
                        "SERVER_REATTACHED",
                        audit::Category::Servers,
                        audit::Outcome::Success,
                    )
                    .server(&server_id)
                    .target(&server.name)
                    .detail(format!("still running as pid {pid}")),
                )
                .await;
            }
            Err(e) => tracing::warn!(server = %server.name, error = %e, "could not reattach"),
        }
    }
}

/// Resets an account from the host: new password, second factor cleared, every
/// session revoked. The reset is itself recorded in the activity log, so a
/// recovery can never happen silently.
async fn recover_account(pool: sqlx::SqlitePool, username: &str) -> anyhow::Result<()> {
    use sqlx::Row;

    let row = sqlx::query("SELECT id, username FROM users WHERE username = ?1")
        .bind(username)
        .fetch_optional(&pool)
        .await?;

    let Some(row) = row else {
        eprintln!("No account called \"{username}\".");
        std::process::exit(1);
    };
    let id: String = row.get("id");
    let name: String = row.get("username");

    let password = format!(
        "{}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8],
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let hash = auth::hash_password(&password)
        .map_err(|e| anyhow::anyhow!("could not hash the new password: {e}"))?;

    sqlx::query(
        "UPDATE users SET password_hash = ?1, totp_secret = NULL, totp_enabled = 0,
                          is_active = 1 WHERE id = ?2",
    )
    .bind(&hash)
    .bind(&id)
    .execute(&pool)
    .await?;

    sqlx::query("DELETE FROM sessions WHERE user_id = ?1")
        .bind(&id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM recovery_codes WHERE user_id = ?1")
        .bind(&id)
        .execute(&pool)
        .await?;

    // Written directly, since the full AppState is not built in this mode.
    let previous: Option<String> =
        sqlx::query("SELECT hash FROM audit_events ORDER BY seq DESC LIMIT 1")
            .fetch_optional(&pool)
            .await?
            .map(|r| r.get("hash"));
    let seq: i64 = sqlx::query("SELECT COALESCE(MAX(seq), 0) + 1 AS next FROM audit_events")
        .fetch_one(&pool)
        .await?
        .get("next");
    let at = db::now();
    let request_id = uuid::Uuid::new_v4().simple().to_string();
    let prev_hash = previous.unwrap_or_else(|| "genesis".to_string());
    let event_hash = audit::hash_for_cli(
        seq, at, &name, "ACCOUNT_RECOVERED", "security", "SUCCESS",
        "reset from the host console", &request_id, &prev_hash,
    );

    sqlx::query(
        "INSERT INTO audit_events
            (seq, at, actor_id, actor, action, category, server_id, target,
             result, detail, meta, ip, request_id, prev_hash, hash)
         VALUES (?1, ?2, ?3, ?4, 'ACCOUNT_RECOVERED', 'security', NULL, NULL,
                 'SUCCESS', 'reset from the host console', NULL, NULL, ?5, ?6, ?7)",
    )
    .bind(seq)
    .bind(at)
    .bind(&id)
    .bind(&name)
    .bind(&request_id)
    .bind(&prev_hash)
    .bind(&event_hash)
    .execute(&pool)
    .await?;

    println!();
    println!("  ┌──────────────────────────────────────────────┐");
    println!("  │  Account recovered                           │");
    println!("  ├──────────────────────────────────────────────┤");
    println!("  │  username : {name:<32} │");
    println!("  │  password : {password:<32} │");
    println!("  └──────────────────────────────────────────────┘");
    println!("  Two-factor authentication has been switched off and every");
    println!("  session signed out. Set both up again after signing in.");
    println!();

    Ok(())
}

/// Stops managed servers cleanly when the panel itself is asked to exit,
/// rather than leaving orphaned Minecraft processes behind.
async fn shutdown(state: AppState) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => tracing::warn!(error = %e, "could not listen for SIGTERM"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }

    // Managed servers are deliberately left running.
    //
    // The panel is a control plane, not the thing keeping worlds alive: shutting
    // it down to apply an update must not disconnect everyone. The running
    // processes are recorded in `running_servers`, and the next start reattaches
    // to them.
    match api::load_all_servers(&state).await {
        Ok(servers) => {
            let mut left = Vec::new();
            for server in servers {
                let running = matches!(
                    state.sup.instance(&server.id).await.runtime().await.status,
                    supervisor::Status::Running | supervisor::Status::Starting
                );
                if running {
                    left.push(server.name);
                }
            }

            if left.is_empty() {
                tracing::info!("shutting down");
            } else {
                tracing::info!(
                    servers = %left.join(", "),
                    "shutting down; these servers are left running and will be picked up again on the next start"
                );
            }
        }
        Err(e) => tracing::warn!(error = %e, "shutting down"),
    }

}
