import { useCallback, useEffect, useState } from "react";
import { api, type PlayersResponse, type Sample, type Server, type Stats } from "../api";
import Chart from "./Chart";

function formatBytes(bytes: number): { value: string; unit: string } {
  if (bytes >= 1024 ** 3) return { value: (bytes / 1024 ** 3).toFixed(1), unit: "GB" };
  if (bytes >= 1024 ** 2) return { value: (bytes / 1024 ** 2).toFixed(0), unit: "MB" };
  if (bytes >= 1024) return { value: (bytes / 1024).toFixed(0), unit: "KB" };
  return { value: String(bytes), unit: "B" };
}

function formatUptime(since: number): string {
  const total = Math.max(0, Math.floor(Date.now() / 1000) - since);
  const days = Math.floor(total / 86400);
  const hours = Math.floor((total % 86400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m ${total % 60}s`;
}

function Meter({ used, limit }: { used: number; limit: number }) {
  if (limit <= 0) return null;
  const ratio = Math.min(used / limit, 1);
  const tone = ratio >= 1 ? "over" : ratio >= 0.85 ? "warn" : "";
  return (
    <div className="meter">
      <span className={tone} style={{ width: `${ratio * 100}%` }} />
    </div>
  );
}

export default function Overview({
  server,
  onTab,
}: {
  server: Server;
  onTab: (tab: string) => void;
}) {
  const [stats, setStats] = useState<Stats | null>(null);
  const [players, setPlayers] = useState<PlayersResponse | null>(null);
  const [samples, setSamples] = useState<Sample[]>([]);
  const [, tick] = useState(0);

  const refresh = useCallback(async () => {
    const [s, p, h] = await Promise.allSettled([
      api.stats(server.id),
      api.players(server.id),
      api.statsHistory(server.id),
    ]);
    if (s.status === "fulfilled") setStats(s.value);
    if (p.status === "fulfilled") setPlayers(p.value);
    if (h.status === "fulfilled") setSamples(h.value.samples);
  }, [server.id]);

  useEffect(() => {
    void refresh();
    // The recorder samples every 5s, so polling at 5s keeps the chart moving.
    const timer = setInterval(() => void refresh(), 5000);
    return () => clearInterval(timer);
  }, [refresh]);

  // Keeps the uptime figure ticking between polls.
  useEffect(() => {
    if (server.runtime.status !== "running") return;
    const timer = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, [server.runtime.status]);

  const running = server.runtime.status === "running";
  const memory = formatBytes(stats?.memory_bytes ?? 0);
  const disk = formatBytes(stats?.disk_bytes ?? 0);
  const memoryLimitBytes = (stats?.memory_limit_mb ?? 0) * 1024 * 1024;
  const diskLimitBytes = (stats?.disk_limit_mb ?? 0) * 1024 * 1024;

  return (
    <>
      <div className="stat-grid">
        <div className="stat">
          <div className="stat-label">Players online</div>
          <div className="stat-value">
            {running ? (players?.online_count ?? 0) : "—"}
            {running && players ? <small>/ {players.max_players}</small> : null}
          </div>
          <div className="stat-foot">
            {running
              ? players?.online.length
                ? players.online.slice(0, 3).join(", ") +
                  (players.online.length > 3 ? ` +${players.online.length - 3}` : "")
                : "Nobody is online"
              : "Server is not running"}
          </div>
        </div>

        <div className="stat">
          <div className="stat-label">Memory</div>
          <div className="stat-value">
            {running ? memory.value : "—"}
            {running ? <small>{memory.unit}</small> : null}
          </div>
          <div className="stat-foot">
            {stats?.memory_limit_mb
              ? `limit ${stats.memory_limit_mb} MB`
              : stats?.host_memory_bytes
                ? `of ${formatBytes(stats.host_memory_bytes).value} ${formatBytes(stats.host_memory_bytes).unit} installed`
                : "no limit set"}
          </div>
          <Meter used={stats?.memory_bytes ?? 0} limit={memoryLimitBytes} />
        </div>

        <div className="stat">
          <div className="stat-label">Disk</div>
          <div className="stat-value">
            {disk.value}
            <small>{disk.unit}</small>
          </div>
          <div className="stat-foot">
            {stats?.disk_limit_mb ? `limit ${stats.disk_limit_mb} MB` : "world and server files"}
          </div>
          <Meter used={stats?.disk_bytes ?? 0} limit={diskLimitBytes} />
        </div>

        <div className="stat">
          <div className="stat-label">Uptime</div>
          <div className="stat-value">
            {running && server.runtime.started_at ? formatUptime(server.runtime.started_at) : "—"}
          </div>
          <div className="stat-foot">
            {running
              ? `pid ${server.runtime.pid ?? "?"} · ${(stats?.cpu_percent ?? 0).toFixed(0)}% CPU`
              : server.runtime.exit_code !== null
                ? `last exit code ${server.runtime.exit_code}`
                : "never started this session"}
          </div>
        </div>
      </div>

      <div className="chart-grid">
        <div className="chart-card">
          <div className="chart-head">
            <span className="stat-label">CPU</span>
            <span className="chart-now">{(stats?.cpu_percent ?? 0).toFixed(1)}%</span>
            <span className="dim spacer">
              {stats?.cpu_cores
                ? `${stats.cpu_cores} of ${stats.host_cores} cores allowed`
                : `${stats?.host_cores ?? 0} cores`}
            </span>
          </div>
          <Chart
            values={samples.map((s) => s.cpu_percent)}
            max={100}
            color="var(--accent)"
            format={(v) => `${v.toFixed(0)}%`}
          />
        </div>

        <div className="chart-card">
          <div className="chart-head">
            <span className="stat-label">Memory</span>
            <span className="chart-now">
              {memory.value}
              <small style={{ fontSize: 14, color: "var(--text-3)" }}> {memory.unit}</small>
            </span>
            <span className="dim spacer">
              {stats?.memory_limit_mb ? `${stats.memory_limit_mb} MB allowed` : "no limit"}
            </span>
          </div>
          <Chart
            values={samples.map((s) => s.memory_bytes / 1024 / 1024)}
            max={stats?.memory_limit_mb || undefined}
            color="#7b5cff"
            format={(v) => `${v.toFixed(0)} MB`}
          />
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <span className="card-title">Quick actions</span>
        </div>
        <div className="card-body pad">
          <div className="row wrap">
            <button className="btn" onClick={() => onTab("console")}>
              Open console
            </button>
            <button className="btn" onClick={() => onTab("players")}>
              Manage players
            </button>
            <button className="btn" onClick={() => onTab("config")}>
              Server settings
            </button>
            <button className="btn" onClick={() => onTab("files")}>
              Browse files
            </button>
          </div>
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <span className="card-title">How to connect</span>
        </div>
        <div className="card-body pad">
          <p className="muted" style={{ marginTop: 0 }}>
            In Minecraft, choose <strong>Multiplayer → Direct Connection</strong> and enter:
          </p>
          <p className="mono" style={{ fontSize: 15 }}>
            localhost
          </p>
          <p className="dim" style={{ marginBottom: 0, fontSize: 12.5 }}>
            Other machines on your network use this computer's IP address instead. The client
            version has to match the server exactly.
          </p>
        </div>
      </div>
    </>
  );
}
