import { useCallback, useEffect, useState } from "react";
import { api, type ActivityEvent, type IntegrityReport, type Server } from "../api";

const CATEGORIES = [
  { key: "all", label: "All" },
  { key: "security", label: "Security" },
  { key: "servers", label: "Servers" },
  { key: "files", label: "Files" },
  { key: "config", label: "Config" },
  { key: "players", label: "Players" },
  { key: "users", label: "Users" },
  { key: "backups", label: "Backups" },
];

/** Turns `EDIT_FILE` or `files.write` into something readable. */
function actionLabel(action: string): string {
  const spaced = action.replace(/[._]/g, " ").toLowerCase();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

function timeOf(at: number): string {
  return new Date(at * 1000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function dayOf(at: number): string {
  const date = new Date(at * 1000);
  const today = new Date();
  const isToday = date.toDateString() === today.toDateString();
  return isToday ? "Today" : date.toLocaleDateString([], { day: "numeric", month: "short" });
}

function ResultMark({ result }: { result: ActivityEvent["result"] }) {
  if (result === "SUCCESS") return <span className="result ok">✓</span>;
  if (result === "DENIED") return <span className="result denied">⨯</span>;
  return <span className="result failed">!</span>;
}

export default function Activity({ servers }: { servers: Server[] }) {
  const [events, setEvents] = useState<ActivityEvent[]>([]);
  const [category, setCategory] = useState("all");
  const [search, setSearch] = useState("");
  const [expanded, setExpanded] = useState<number | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [oldest, setOldest] = useState<number | null>(null);
  const [integrity, setIntegrity] = useState<IntegrityReport | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const serverName = useCallback(
    (id: string | null) => servers.find((s) => s.id === id)?.name ?? null,
    [servers],
  );

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const response = await api.activity({ category, search, limit: 60 });
      setEvents(response.events);
      setOldest(response.oldest_seq);
      setHasMore(response.has_more);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [category, search]);

  useEffect(() => {
    // Debounced so typing in the search box does not fire a request per key.
    const timer = setTimeout(() => void load(), search ? 300 : 0);
    return () => clearTimeout(timer);
  }, [load, search]);

  async function loadMore() {
    if (oldest === null) return;
    try {
      const response = await api.activity({ category, search, before: oldest, limit: 60 });
      setEvents((current) => [...current, ...response.events]);
      setOldest(response.oldest_seq);
      setHasMore(response.has_more);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function verify() {
    setChecking(true);
    try {
      setIntegrity(await api.verifyActivity());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setChecking(false);
    }
  }

  async function copyCheckpoint() {
    try {
      const point = await api.activityCheckpoint();
      const text = `seq ${point.seq} : ${point.hash}`;
      await navigator.clipboard?.writeText(text);
      setIntegrity({
        checked: point.seq ?? 0,
        intact: true,
        broken_at: null,
        head: point.hash,
        message: `Checkpoint copied — ${text}. Keep it somewhere off this machine.`,
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <>
      {error && <div className="banner error">{error}</div>}

      {integrity && (
        <div className={`banner ${integrity.intact ? "ok" : "error"}`}>
          <strong>{integrity.intact ? "Chain intact" : "Integrity failure"}</strong> —{" "}
          {integrity.message}
          {integrity.broken_at !== null && (
            <> Records from #{integrity.broken_at} onwards can no longer be trusted.</>
          )}
        </div>
      )}

      <div className="toolbar">
        <div className="segmented">
          {CATEGORIES.map((c) => (
            <button
              key={c.key}
              className={category === c.key ? "active" : ""}
              onClick={() => setCategory(c.key)}
            >
              {c.label}
            </button>
          ))}
        </div>
        <div className="spacer" />
        <input
          className="search"
          type="search"
          placeholder="Search actor, action, file…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <button className="btn sm" disabled={checking} onClick={() => void verify()}>
          {checking ? "Checking…" : "Verify integrity"}
        </button>
        <button className="btn sm" title="Copy the chain head to store off this machine" onClick={() => void copyCheckpoint()}>
          Checkpoint
        </button>
      </div>

      <div className="card">
        <div className="activity-list">
          {loading && events.length === 0 && <div className="empty">Loading…</div>}

          {!loading && events.length === 0 && (
            <div className="empty">
              <div className="empty-title">Nothing recorded yet</div>
              Actions taken through the panel show up here.
            </div>
          )}

          {events.map((event, index) => {
            const previous = events[index - 1];
            const newDay = !previous || dayOf(previous.at) !== dayOf(event.at);
            const open = expanded === event.seq;
            const server = serverName(event.server_id);

            return (
              <div key={event.seq}>
                {newDay && <div className="activity-day">{dayOf(event.at)}</div>}

                <button
                  className={`activity-row ${open ? "open" : ""}`}
                  onClick={() => setExpanded(open ? null : event.seq)}
                >
                  <span className="activity-time nums">{timeOf(event.at)}</span>
                  <span className="activity-actor">{event.actor}</span>
                  <span className="activity-what">
                    <strong>{actionLabel(event.action)}</strong>
                    {event.target && <span className="mono dim"> {event.target}</span>}
                    {server && <span className="tag">{server}</span>}
                  </span>
                  <ResultMark result={event.result} />
                </button>

                {open && (
                  <div className="activity-detail">
                    <dl>
                      <dt>Result</dt>
                      <dd>
                        {event.result}
                        {event.detail ? ` — ${event.detail}` : ""}
                      </dd>

                      <dt>Category</dt>
                      <dd>{event.category}</dd>

                      {event.target && (
                        <>
                          <dt>Target</dt>
                          <dd className="mono">{event.target}</dd>
                        </>
                      )}

                      {event.ip && (
                        <>
                          <dt>Address</dt>
                          <dd className="mono">{event.ip}</dd>
                        </>
                      )}

                      {event.meta &&
                        Object.entries(event.meta)
                          .filter(([, value]) => value !== null && value !== undefined)
                          .map(([key, value]) => (
                            <div key={key} style={{ display: "contents" }}>
                              <dt>{key.replace(/_/g, " ")}</dt>
                              <dd className="mono break">{String(value)}</dd>
                            </div>
                          ))}

                      <dt>Record</dt>
                      <dd className="mono dim">
                        #{event.seq} · req {event.request_id.slice(0, 8)} · hash {event.hash}
                      </dd>
                    </dl>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>

      {hasMore && (
        <button className="btn" onClick={() => void loadMore()}>
          Load older
        </button>
      )}
    </>
  );
}
