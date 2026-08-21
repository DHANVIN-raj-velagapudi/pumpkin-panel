import { useCallback, useEffect, useState } from "react";
import { api, type Server, type User } from "../api";

interface Grant {
  server_id: string;
  can_console: boolean;
  can_power: boolean;
  can_files: boolean;
  can_config: boolean;
}

type GrantKey = "can_console" | "can_power" | "can_files" | "can_config";

const PERMISSIONS: Array<{ key: GrantKey; label: string }> = [
  { key: "can_console", label: "Console" },
  { key: "can_power", label: "Power" },
  { key: "can_files", label: "Files" },
  { key: "can_config", label: "Config" },
];

export default function Users({ servers, currentUserId }: { servers: Server[]; currentUserId: string }) {
  const [users, setUsers] = useState<User[]>([]);
  const [selected, setSelected] = useState<User | null>(null);
  const [grants, setGrants] = useState<Grant[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState({ username: "", password: "", role: "user" });

  const loadUsers = useCallback(async () => {
    try {
      setUsers(await api.users());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void loadUsers();
  }, [loadUsers]);

  const loadGrants = useCallback(async (user: User) => {
    setSelected(user);
    try {
      setGrants(await api.grants(user.id));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  function grantFor(serverId: string): Grant {
    return (
      grants.find((g) => g.server_id === serverId) ?? {
        server_id: serverId,
        can_console: false,
        can_power: false,
        can_files: false,
        can_config: false,
      }
    );
  }

  async function toggle(serverId: string, key: GrantKey) {
    if (!selected) return;
    const current = grantFor(serverId);
    const next = { ...current, [key]: !current[key] };
    // A grant with nothing enabled is the same as no grant at all.
    const empty = !next.can_console && !next.can_power && !next.can_files && !next.can_config;

    try {
      await api.setGrant(selected.id, { ...next, revoke: empty });
      await loadGrants(selected);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function create() {
    try {
      await api.createUser(creating);
      setCreating({ username: "", password: "", role: "user" });
      await loadUsers();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function changePassword(user: User) {
    const password = prompt(`New password for "${user.username}" (at least 8 characters)`);
    if (!password) return;
    try {
      await api.updateUser(user.id, { password });
      // The server drops every session for that account on a password change.
      if (user.id === currentUserId) {
        alert("Password changed. You will be signed out.");
        window.location.reload();
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function remove(user: User) {
    if (!confirm(`Delete the account "${user.username}"?`)) return;
    try {
      await api.deleteUser(user.id);
      if (selected?.id === user.id) setSelected(null);
      await loadUsers();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <>
      {error && <div className="banner error">{error}</div>}

      <div className="card">
        <div className="card-head">Accounts</div>
        <div className="card-body">
          <table className="grid">
            <thead>
              <tr>
                <th>Username</th>
                <th style={{ width: 100 }}>Role</th>
                <th style={{ width: 200 }} />
              </tr>
            </thead>
            <tbody>
              {users.map((user) => (
                <tr key={user.id}>
                  <td className="mono">{user.username}</td>
                  <td className="muted">{user.role}</td>
                  <td>
                    <div className="row">
                      {user.role !== "admin" && (
                        <button className="btn sm" onClick={() => void loadGrants(user)}>
                          Access
                        </button>
                      )}
                      <button className="btn sm" onClick={() => void changePassword(user)}>
                        Password
                      </button>
                      <button
                        className="btn sm danger"
                        disabled={user.id === currentUserId}
                        title={user.id === currentUserId ? "You cannot delete your own account" : ""}
                        onClick={() => void remove(user)}
                      >
                        Delete
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>

      <div className="card">
        <div className="card-head">Add an account</div>
        <div className="card-body">
          <div className="field">
            <div className="field-label">username</div>
            <input
              type="text"
              value={creating.username}
              onChange={(e) => setCreating({ ...creating, username: e.target.value })}
            />
          </div>
          <div className="field">
            <div>
              <div className="field-label">password</div>
              <div className="field-help">At least 8 characters.</div>
            </div>
            <input
              type="password"
              value={creating.password}
              onChange={(e) => setCreating({ ...creating, password: e.target.value })}
            />
          </div>
          <div className="field">
            <div>
              <div className="field-label">role</div>
              <div className="field-help">
                Administrators can see and do everything. Users only see servers granted to them.
              </div>
            </div>
            <select
              value={creating.role}
              onChange={(e) => setCreating({ ...creating, role: e.target.value })}
            >
              <option value="user">user</option>
              <option value="admin">admin</option>
            </select>
          </div>
          <div style={{ paddingTop: 12 }}>
            <button
              className="btn primary"
              disabled={!creating.username || creating.password.length < 8}
              onClick={() => void create()}
            >
              Create account
            </button>
          </div>
        </div>
      </div>

      {selected && (
        <div className="card">
          <div className="card-head">
            Server access for {selected.username}
            <button
              className="btn sm"
              style={{ marginLeft: "auto" }}
              onClick={() => setSelected(null)}
            >
              Close
            </button>
          </div>
          <div className="card-body">
            {servers.length === 0 && <p className="muted">No servers to grant yet.</p>}
            <table className="grid">
              <thead>
                <tr>
                  <th>Server</th>
                  {PERMISSIONS.map((p) => (
                    <th key={p.key} style={{ width: 90 }}>
                      {p.label}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {servers.map((server) => {
                  const grant = grantFor(server.id);
                  return (
                    <tr key={server.id}>
                      <td>{server.name}</td>
                      {PERMISSIONS.map((permission) => (
                        <td key={permission.key}>
                          <button
                            type="button"
                            className={`switch ${grant[permission.key] ? "on" : ""}`}
                            aria-pressed={grant[permission.key]}
                            onClick={() => void toggle(server.id, permission.key)}
                          />
                        </td>
                      ))}
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </>
  );
}
