import { useCallback, useEffect, useState } from "react";
import { api, type Server, type User } from "./api";
import Login from "./components/Login";
import Overview from "./components/Overview";
import Console from "./components/Console";
import Players from "./components/Players";
import Plugins from "./components/Plugins";
import Files from "./components/Files";
import ConfigEditor from "./components/ConfigEditor";
import ServerSettings from "./components/ServerSettings";
import Users from "./components/Users";
import Activity from "./components/Activity";
import Account from "./components/Account";
import StatusBadge from "./components/StatusBadge";

type Tab = "overview" | "console" | "players" | "plugins" | "files" | "config" | "settings";

const TABS: Array<{ key: Tab; label: string }> = [
  { key: "overview", label: "Overview" },
  { key: "console", label: "Console" },
  { key: "players", label: "Players" },
  { key: "plugins", label: "Plugins" },
  { key: "files", label: "Files" },
  { key: "config", label: "Settings" },
  { key: "settings", label: "Resources" },
];

export default function App() {
  const [user, setUser] = useState<User | null>(null);
  const [booting, setBooting] = useState(true);

  useEffect(() => {
    api
      .me()
      .then((r) => setUser(r.user))
      .catch(() => setUser(null))
      .finally(() => setBooting(false));
  }, []);

  if (booting) return <div className="empty">Loading…</div>;
  if (!user) return <Login onLogin={setUser} />;
  return <Shell user={user} onLogout={() => setUser(null)} />;
}

function Shell({ user, onLogout }: { user: User; onLogout: () => void }) {
  const [servers, setServers] = useState<Server[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("overview");
  type Page = "server" | "users" | "activity" | "account";
  const [page, setPage] = useState<Page>("server");
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const list = await api.servers();
      setServers(list);
      setSelectedId((current) =>
        current && list.some((s) => s.id === current) ? current : (list[0]?.id ?? null),
      );
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    // Status is polled; the console keeps its own live socket.
    const timer = setInterval(() => void refresh(), 5000);
    return () => clearInterval(timer);
  }, [refresh]);

  const selected = servers.find((s) => s.id === selectedId) ?? null;

  async function logout() {
    await api.logout().catch(() => undefined);
    onLogout();
  }

  async function power(action: "start" | "stop" | "restart" | "kill") {
    if (!selected) return;

    // Never interrupt people mid-game without asking. The player list is only
    // fetched for the disruptive actions, and a failure to read it does not
    // block the action.
    if (action === "stop" || action === "restart" || action === "kill") {
      let online: string[] = [];
      try {
        online = (await api.players(selected.id)).online;
      } catch {
        // Query unavailable; fall through to the plain confirmation below.
      }

      if (online.length > 0) {
        const who = online.length === 1 ? online[0] : `${online.length} players`;
        const verb = action === "restart" ? "Restart" : action === "kill" ? "Force kill" : "Stop";
        if (!confirm(`${who} currently online.

${verb} the server anyway?`)) return;
      } else if (action === "kill") {
        if (!confirm("Force kill the server? The world may not be saved.")) return;
      }
    }

    try {
      await api.power(selected.id, action);
      setError(null);
      setTimeout(() => void refresh(), 400);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  const status = selected?.runtime.status;
  const running = status === "running";
  const busy = status === "starting" || status === "stopping";

  const visibleTabs = TABS.filter((t) => {
    if (!selected) return false;
    if (t.key === "console") return selected.access.console;
    if (t.key === "players") return selected.access.console;
    if (t.key === "plugins") return selected.access.files;
    if (t.key === "files") return selected.access.files;
    if (t.key === "config") return selected.access.config;
    if (t.key === "settings") return user.role === "admin";
    return true;
  });

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">P</span>
          <div>
            <div className="brand-name">Panel</div>
            <div className="brand-sub">Minecraft server manager</div>
          </div>
        </div>

        <div className="nav-label">Servers</div>
        <div className="nav-list">
          {servers.length === 0 && (
            <p className="dim" style={{ padding: "6px 11px", lineHeight: 1.5 }}>
              {user.role === "admin"
                ? "No servers yet. Add one below."
                : "No servers have been shared with you."}
            </p>
          )}
          {servers.map((server) => (
            <button
              key={server.id}
              className={`nav-item ${server.id === selectedId && page === "server" ? "active" : ""}`}
              onClick={() => {
                setSelectedId(server.id);
                setPage("server");
              }}
            >
              <span className={`dot ${server.runtime.status}`} />
              <span className="nav-item-name">{server.name}</span>
              <span className="nav-item-meta">
                {server.runtime.status === "running" ? "on" : ""}
              </span>
            </button>
          ))}

          {user.role === "admin" && (
            <>
              <div className="nav-label" style={{ paddingLeft: 11 }}>
                Manage
              </div>
              <button
                className={`nav-item ${page === "activity" ? "active" : ""}`}
                onClick={() => setPage("activity")}
              >
                <span className="dot" />
                <span className="nav-item-name">Activity</span>
              </button>
              <button
                className={`nav-item ${page === "users" ? "active" : ""}`}
                onClick={() => setPage("users")}
              >
                <span className="dot" />
                <span className="nav-item-name">Users &amp; access</span>
              </button>
              <button
                className="nav-item"
                onClick={() => {
                  setPage("server");
                  setSelectedId(null);
                }}
              >
                <span className="dot" />
                <span className="nav-item-name">Add a server</span>
              </button>
            </>
          )}
        </div>

        <div className="sidebar-footer">
          <span className="avatar">{user.username.slice(0, 2)}</span>
          <button
            className="account-link"
            title="Account and security"
            onClick={() => setPage("account")}
          >
            <div style={{ fontWeight: 550, overflow: "hidden", textOverflow: "ellipsis" }}>
              {user.username}
            </div>
            <div className="dim" style={{ fontSize: 12 }}>
              {user.role} · account
            </div>
          </button>
          <button className="btn ghost sm" onClick={() => void logout()}>
            Sign out
          </button>
        </div>
      </aside>

      <main className="main">
        {page === "account" ? (
          <>
            <div className="topbar">
              <div className="topbar-row">
                <h1>Account</h1>
                <span className="dim">security for {user.username}</span>
              </div>
            </div>
            <div className="content">
              <Account user={user} />
            </div>
          </>
        ) : page === "activity" ? (
          <>
            <div className="topbar">
              <div className="topbar-row">
                <h1>Activity</h1>
                <span className="dim">every action taken through this panel</span>
              </div>
            </div>
            <div className="content">
              <Activity servers={servers} />
            </div>
          </>
        ) : page === "users" ? (
          <>
            <div className="topbar">
              <div className="topbar-row">
                <h1>Users &amp; access</h1>
              </div>
            </div>
            <div className="content">
              <Users servers={servers} currentUserId={user.id} />
            </div>
          </>
        ) : !selected ? (
          <>
            <div className="topbar">
              <div className="topbar-row">
                <h1>{user.role === "admin" ? "Add a server" : "No server selected"}</h1>
              </div>
            </div>
            <div className="content">
              {error && <div className="banner error">{error}</div>}
              {user.role === "admin" && <ServerSettings server={null} onChanged={refresh} />}
            </div>
          </>
        ) : (
          <>
            <div className="topbar">
              <div className="topbar-row">
                <h1>{selected.name}</h1>
                <StatusBadge runtime={selected.runtime} />

                {selected.access.power && (
                  <div className="row spacer">
                    <button
                      className="btn sm primary"
                      disabled={running || busy}
                      onClick={() => void power("start")}
                    >
                      Start
                    </button>
                    <button
                      className="btn sm"
                      disabled={!running && !busy}
                      onClick={() => void power("stop")}
                    >
                      Stop
                    </button>
                    <button
                      className="btn sm"
                      disabled={!running && !busy}
                      onClick={() => void power("restart")}
                    >
                      Restart
                    </button>
                    <button
                      className="btn sm danger"
                      disabled={!running && !busy}
                      title="Terminate the process immediately, without saving"
                      onClick={() => void power("kill")}
                    >
                      Kill
                    </button>
                  </div>
                )}
              </div>

              <div className="tabs">
                {visibleTabs.map((t) => (
                  <button
                    key={t.key}
                    className={`tab ${tab === t.key ? "active" : ""}`}
                    onClick={() => setTab(t.key)}
                  >
                    {t.label}
                  </button>
                ))}
              </div>
            </div>

            {error && (
              <div style={{ padding: "16px 24px 0" }}>
                <div className="banner error">{error}</div>
              </div>
            )}

            {tab === "overview" && (
              <div className="content">
                <Overview server={selected} onTab={(t) => setTab(t as Tab)} />
              </div>
            )}
            {tab === "console" && selected.access.console && (
              <div className="content flush">
                <Console server={selected} />
              </div>
            )}
            {tab === "players" && selected.access.console && (
              <div className="content">
                <Players server={selected} />
              </div>
            )}
            {tab === "plugins" && selected.access.files && (
              <div className="content">
                <Plugins server={selected} />
              </div>
            )}
            {tab === "files" && selected.access.files && (
              <div className="content">
                <Files server={selected} />
              </div>
            )}
            {tab === "config" && selected.access.config && (
              <div className="content">
                <ConfigEditor server={selected} />
              </div>
            )}
            {tab === "settings" && user.role === "admin" && (
              <div className="content">
                <ServerSettings server={selected} onChanged={refresh} />
              </div>
            )}
          </>
        )}
      </main>
    </div>
  );
}
