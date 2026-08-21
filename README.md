# Pumpkin Panel

A self-hosted web control panel for [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin) Minecraft
servers. Start and stop your server, watch the live console, manage players, edit configuration
through real form controls, take backups, and see who did what.

Ships as **one binary**. No Docker, no PHP, no database server, no separate daemon.

> **Status: pre-1.0.** It runs, and it is used daily against a real server, but it has been
> developed and tested on Windows only so far, and automated test coverage is thin. See
> [Known gaps](#known-gaps) before pointing it at anything you care about.

## Why

Pumpkin ships no panel of its own; the documented route is Pterodactyl, whose
[egg](https://github.com/Pumpkin-MC/Pumpkin/blob/master/egg-pumpkin.json) clones the repository and
compiles Rust from source — its own description notes it "requires around 4GB RAM to build". So the
supported path is Docker, Wings, PHP, Laravel, MySQL and Redis, plus a Cargo build, in order to run
a server that starts in 11 milliseconds.

This is the other end of that trade, and it understands Pumpkin specifically rather than treating it
as a generic process.

## Features

**Server control** — start, stop, restart, force kill. Graceful stop writes `stop` to the server's
stdin and only escalates after a 30 second grace period. Stopping or restarting while players are
online asks first.

**Servers outlive the panel.** Updating or restarting the panel does not touch running worlds; the
panel records each process and reattaches to it on the next start.

**Live console** — streamed over a WebSocket, ANSI colour rendered, command history on the arrow
keys, and autoscroll that pauses when you scroll up to read.

**Players** — online list from the server's query port, plus operators, bans, whitelist and everyone
seen before. Op, kick, ban, pardon, gamemode, heal and feed, all driven through the server console.

**Configuration** — `pumpkin.toml` flattened into typed form controls grouped by table, with the
settings people actually change surfaced first and explained in plain English. Comments, ordering
and formatting survive a save, and a `.bak` is kept.

**Files** — browse, edit, upload, download, move, copy, extract archives, and zip a selection for
download. Any text format opens in the editor, including formats the panel has never heard of.

**Backups** — snapshot, schedule, retention and restore, stored outside the server folder. Restoring
takes a safety snapshot first and refuses to run while the server is up.

**Pumpkin awareness** — version and both protocol numbers read from the startup banner, installed
WASM plugins, the sandbox policy from `pumpkin.toml`, and a marketplace browser.

**Accounts** — administrators and users, per-server permissions for console, power, files and
config, optional TOTP two-factor with recovery codes.

**Activity log** — every action recorded with actor, target, result, address and request id. Records
are hash-chained, so altering or deleting one is detectable.

## Building

Requires Rust and Node.

```bash
cd web && npm install && npm run build && cd ..
```

```bash
cargo build --release
```

The frontend is embedded into the binary at compile time, so `web/dist` must be built **before**
`cargo build`. The result is a single executable in `target/release/`.

## Running

```bash
./pumpkin-panel
```

On first run the panel creates an administrator account and prints the password once. It is stored
only as an Argon2 hash.

Then open <http://localhost:8080>, sign in, and add a server by pointing the panel at the Pumpkin
executable and its folder.

### Configuration

All optional, read from the environment.

| Variable | Default | Meaning |
| --- | --- | --- |
| `PANEL_BIND` | `0.0.0.0:8080` | Address and port to listen on |
| `PANEL_DATA_DIR` | `./data` | Where the database, key and backups are kept |
| `PANEL_ADMIN_USER` | `admin` | Username for the bootstrap account |
| `PANEL_ADMIN_PASSWORD` | *generated* | Password for the bootstrap account |
| `PANEL_TLS_CERT` / `PANEL_TLS_KEY` | — | PEM certificate and key; enables HTTPS |
| `PANEL_BEHIND_TLS_PROXY` | `false` | Set when a proxy terminates TLS, so cookies get `Secure` |
| `PANEL_TRUST_PROXY` | `false` | Believe `X-Forwarded-For` for rate limiting |
| `PANEL_CLAMAV` | — | `host:port` of a clamd daemon to scan uploads |
| `PANEL_LOG` | `pumpkin_panel=info` | Tracing filter |

### Account recovery

If both the password and the recovery codes are lost, there is deliberately no reset link — anyone
who could use it could take over the machine. Recovery requires a shell on the host:

```bash
./pumpkin-panel recover <username>
```

That prints a new password, clears two-factor, ends every session, and records itself in the
activity log.

## Security

- Passwords hashed with Argon2. Sessions are opaque tokens in an `HttpOnly`, `SameSite=Lax` cookie,
  expiring after a day idle or two weeks absolute, and revoked on password change.
- Failed sign-ins escalate through 30 second, 5 minute and 30 minute lockouts, keyed on address and
  account together so nobody can lock out someone else's account remotely.
- File and configuration paths are resolved against the server folder with `..` and absolute paths
  rejected, then canonicalised so a symlink cannot escape. The server executable itself is read-only
  through the panel, since replacing it would mean arbitrary code execution.
- Uploads are size limited, counted against the server's disk allowance, and archives are inspected
  for decompression bombs and unsafe entry paths before anything is written.
- TOTP secrets are encrypted at rest under a key held outside the database.
- Standard security headers, including a content security policy locked to `self`.

The panel is a control plane for a machine. Put it behind HTTPS and do not expose it to the internet
without one.

## Known gaps

- **Only tested on Windows.** The Linux paths — CPU affinity, `SIGTERM` handling, `setsid` — compile
  but have never been run.
- **Thin automated test coverage.** Most behaviour has been verified by hand rather than by tests.
- **Never tested with more than one server** registered at a time.
- A reattached server's console is read-only until it is restarted, because the panel no longer
  holds its stdin.
- The marketplace is browse-only; installing still means downloading and uploading by hand.
- No scheduled restarts, and no alerting when a server crashes.

## Licence

MIT.

The panel talks to Pumpkin over stdin, stdout and the filesystem, so it is an independent program
rather than a derivative of the GPL-licensed server.
