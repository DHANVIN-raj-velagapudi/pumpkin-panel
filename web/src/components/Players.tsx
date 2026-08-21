import { useCallback, useEffect, useState } from "react";
import { api, type PlayerActionName, type PlayerEntry, type PlayersResponse, type Server } from "../api";

type View = "online" | "operators" | "banned" | "whitelist" | "known";

const VIEWS: Array<{ key: View; label: string }> = [
  { key: "online", label: "Online" },
  { key: "operators", label: "Operators" },
  { key: "banned", label: "Banned" },
  { key: "whitelist", label: "Whitelist" },
  { key: "known", label: "Seen before" },
];

function Face({ name }: { name: string }) {
  return <span className="player-face">{name.slice(0, 2)}</span>;
}

export default function Players({ server }: { server: Server }) {
  const [data, setData] = useState<PlayersResponse | null>(null);
  const [view, setView] = useState<View>("online");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setData(await api.players(server.id));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [server.id]);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), 8000);
    return () => clearInterval(timer);
  }, [refresh]);

  const running = server.runtime.status === "running";

  async function act(action: PlayerActionName, target: string, value?: string) {
    setBusy(true);
    setError(null);
    try {
      await api.playerAction(server.id, action, target, value);
      setNotice(`${action.replace(/_/g, " ")} → ${target}`);
      setTimeout(() => setNotice(null), 2500);
      // Give the server a moment to rewrite ops.json / banned-players.json.
      setTimeout(() => void refresh(), 700);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  function confirmAct(action: PlayerActionName, target: string, prompt_: string | null) {
    if (prompt_) {
      const value = prompt(prompt_);
      if (value === null) return;
      void act(action, target, value || undefined);
    } else {
      void act(action, target);
    }
  }

  const isOp = (name: string) =>
    (data?.operators ?? []).some((o) => o.name.toLowerCase() === name.toLowerCase());

  const onlineRows: PlayerEntry[] = (data?.online ?? []).map((name) => ({
    name,
    uuid: null,
    reason: null,
    source: null,
    expires: null,
    level: null,
  }));

  const rows: PlayerEntry[] =
    view === "online"
      ? onlineRows
      : view === "operators"
        ? (data?.operators ?? [])
        : view === "banned"
          ? [...(data?.banned ?? []), ...(data?.banned_ips ?? [])]
          : view === "whitelist"
            ? (data?.whitelist ?? [])
            : (data?.known ?? []);

  const count = (key: View): number =>
    key === "online"
      ? (data?.online.length ?? 0)
      : key === "operators"
        ? (data?.operators.length ?? 0)
        : key === "banned"
          ? (data?.banned.length ?? 0) + (data?.banned_ips.length ?? 0)
          : key === "whitelist"
            ? (data?.whitelist.length ?? 0)
            : (data?.known.length ?? 0);

  return (
    <>
      {error && <div className="banner error">{error}</div>}
      {notice && <div className="banner ok">{notice}</div>}
      {!running && (
        <div className="banner info">
          The server is stopped. You can still review operators, bans and the whitelist, but
          changing them needs the server running.
        </div>
      )}
      {running && data?.query_error && (
        <div className="banner info">
          Live player list unavailable: {data.query_error}
        </div>
      )}

      <div className="toolbar">
        <div className="segmented">
          {VIEWS.map((v) => (
            <button
              key={v.key}
              className={view === v.key ? "active" : ""}
              onClick={() => setView(v.key)}
            >
              {v.label} <span className="dim nums">{count(v.key)}</span>
            </button>
          ))}
        </div>

        <div className="spacer" />

        <button
          className="btn sm"
          disabled={!running || busy}
          onClick={() => {
            const name = prompt("Player name to add to the whitelist");
            if (name) void act("whitelist_add", name);
          }}
        >
          Add to whitelist
        </button>
        <button
          className="btn sm"
          disabled={!running || busy}
          onClick={() => {
            const name = prompt("Player name to op");
            if (name) void act("op", name);
          }}
        >
          Op a player
        </button>
      </div>

      <div className="card">
        <div className="card-body" style={{ padding: 0 }}>
          <table className="grid">
            <thead>
              <tr>
                <th>Player</th>
                {view === "banned" && <th style={{ width: 220 }}>Reason</th>}
                {view === "operators" && <th style={{ width: 90 }}>Level</th>}
                <th style={{ width: view === "online" ? 420 : 240 }}>Actions</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((entry) => (
                <tr key={`${view}-${entry.name}`}>
                  <td>
                    <div className="player-row">
                      <Face name={entry.name} />
                      <div>
                        <div style={{ fontWeight: 550 }}>
                          {entry.name}{" "}
                          {view !== "operators" && isOp(entry.name) && (
                            <span className="tag op">OP</span>
                          )}
                        </div>
                        {entry.uuid && (
                          <div className="field-key">{entry.uuid}</div>
                        )}
                      </div>
                    </div>
                  </td>

                  {view === "banned" && (
                    <td className="muted">
                      {entry.reason || "no reason given"}
                      {entry.source && <div className="dim" style={{ fontSize: 12 }}>by {entry.source}</div>}
                    </td>
                  )}

                  {view === "operators" && <td className="muted nums">{entry.level ?? "—"}</td>}

                  <td>
                    <div className="row wrap">
                      {view === "online" && (
                        <>
                          <button
                            className="btn sm"
                            disabled={busy}
                            title="Restore full hearts"
                            onClick={() => void act("heal", entry.name)}
                          >
                            Heal
                          </button>
                          <button
                            className="btn sm"
                            disabled={busy}
                            title="Refill the hunger bar"
                            onClick={() => void act("feed", entry.name)}
                          >
                            Feed
                          </button>
                          <select
                            style={{ width: 118 }}
                            value=""
                            disabled={busy}
                            onChange={(e) => {
                              if (e.target.value) void act("gamemode", entry.name, e.target.value);
                              e.target.value = "";
                            }}
                          >
                            <option value="">Gamemode…</option>
                            <option value="survival">Survival</option>
                            <option value="creative">Creative</option>
                            <option value="adventure">Adventure</option>
                            <option value="spectator">Spectator</option>
                          </select>
                          <button
                            className="btn sm"
                            disabled={busy}
                            onClick={() => void act(isOp(entry.name) ? "deop" : "op", entry.name)}
                          >
                            {isOp(entry.name) ? "Remove op" : "Make op"}
                          </button>
                          <button
                            className="btn sm"
                            disabled={busy}
                            onClick={() => confirmAct("kick", entry.name, "Kick reason (optional)")}
                          >
                            Kick
                          </button>
                          <button
                            className="btn sm danger"
                            disabled={busy}
                            onClick={() => confirmAct("ban", entry.name, "Ban reason (optional)")}
                          >
                            Ban
                          </button>
                        </>
                      )}

                      {view === "operators" && (
                        <button
                          className="btn sm danger"
                          disabled={!running || busy}
                          onClick={() => void act("deop", entry.name)}
                        >
                          Remove op
                        </button>
                      )}

                      {view === "banned" && (
                        <button
                          className="btn sm"
                          disabled={!running || busy}
                          onClick={() =>
                            void act(
                              entry.name.includes(".") || entry.name.includes(":")
                                ? "pardon_ip"
                                : "pardon",
                              entry.name,
                            )
                          }
                        >
                          Unban
                        </button>
                      )}

                      {view === "whitelist" && (
                        <button
                          className="btn sm danger"
                          disabled={!running || busy}
                          onClick={() => void act("whitelist_remove", entry.name)}
                        >
                          Remove
                        </button>
                      )}

                      {view === "known" && (
                        <>
                          <button
                            className="btn sm"
                            disabled={!running || busy}
                            onClick={() => void act(isOp(entry.name) ? "deop" : "op", entry.name)}
                          >
                            {isOp(entry.name) ? "Remove op" : "Make op"}
                          </button>
                          <button
                            className="btn sm danger"
                            disabled={!running || busy}
                            onClick={() => confirmAct("ban", entry.name, "Ban reason (optional)")}
                          >
                            Ban
                          </button>
                        </>
                      )}
                    </div>
                  </td>
                </tr>
              ))}

              {rows.length === 0 && (
                <tr>
                  <td colSpan={4}>
                    <div className="empty">
                      <div className="empty-title">
                        {view === "online" ? "Nobody is online" : `No ${view} entries`}
                      </div>
                      {view === "online" && running && "Players who join will appear here."}
                    </div>
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>
    </>
  );
}
