// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
import { useCallback, useEffect, useState } from "react";
import { api, type ConfigField, type Server } from "../api";

interface Draft {
  name: string;
  binary_path: string;
  working_dir: string;
  args: string;
  stop_command: string;
  autostart: boolean;
  memory_limit_mb: number;
  disk_limit_mb: number;
  cpu_cores: number;
}

const blank: Draft = {
  name: "",
  binary_path: "",
  working_dir: "",
  args: "",
  stop_command: "stop",
  autostart: false,
  memory_limit_mb: 0,
  disk_limit_mb: 0,
  cpu_cores: 0,
};

/** Distance settings live in pumpkin.toml, not the panel database. */
const DISTANCE_KEYS = [
  "networking.java.view_distance",
  "networking.java.simulation_distance",
  "networking.bedrock.view_distance",
  "networking.bedrock.simulation_distance",
];

export default function ServerSettings({
  server,
  onChanged,
}: {
  server: Server | null;
  onChanged: () => Promise<void> | void;
}) {
  const [draft, setDraft] = useState<Draft>(blank);
  const [hostCores, setHostCores] = useState(0);
  const [memoryUnit, setMemoryUnit] = useState<"MB" | "GB">("MB");
  const [distances, setDistances] = useState<Record<string, number>>({});
  const [distanceFields, setDistanceFields] = useState<ConfigField[]>([]);
  const [distanceDirty, setDistanceDirty] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setDraft(
      server
        ? {
            name: server.name,
            binary_path: server.binary_path,
            working_dir: server.working_dir,
            args: server.args,
            stop_command: server.stop_command,
            autostart: server.autostart,
            memory_limit_mb: server.memory_limit_mb ?? 0,
            disk_limit_mb: server.disk_limit_mb ?? 0,
            cpu_cores: server.cpu_cores ?? 0,
          }
        : blank,
    );
    // Show a GB figure when the value is a clean multiple, which it usually is.
    const mb = server?.memory_limit_mb ?? 0;
    setMemoryUnit(mb > 0 && mb % 1024 === 0 ? "GB" : "MB");
    setError(null);
  }, [server]);

  const loadDistances = useCallback(async () => {
    if (!server) return;
    try {
      const [stats, config] = await Promise.all([
        api.stats(server.id),
        api.readConfig(server.id, "pumpkin.toml"),
      ]);
      setHostCores(stats.host_cores);

      // Only offer the keys this particular config actually has.
      const present = config.fields.filter((f) => DISTANCE_KEYS.includes(f.key));
      setDistanceFields(present);
      setDistances(
        Object.fromEntries(present.map((f) => [f.key, Number(f.value) || 0])),
      );
      setDistanceDirty(false);
    } catch {
      // A missing pumpkin.toml just means no distance controls.
      setDistanceFields([]);
    }
  }, [server]);

  useEffect(() => {
    void loadDistances();
  }, [loadDistances]);

  function set<K extends keyof Draft>(key: K, value: Draft[K]) {
    setDraft((current) => ({ ...current, [key]: value }));
  }

  function flash(message: string) {
    setNotice(message);
    setTimeout(() => setNotice(null), 4000);
  }

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      if (server) {
        await api.updateServer(server.id, { ...draft });
        flash("Saved. Restart the server to apply the core limit.");
      } else {
        await api.createServer({ ...draft });
        setDraft(blank);
        flash("Server added.");
      }
      await onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function saveDistances() {
    if (!server) return;
    setBusy(true);
    setError(null);
    try {
      await api.writeConfig(server.id, "pumpkin.toml", distances);
      flash("Distances saved. Restart the server to apply them.");
      setDistanceDirty(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function remove() {
    if (!server) return;
    if (!confirm(`Remove "${server.name}" from the panel?\n\nFiles on disk are left untouched.`)) return;
    setBusy(true);
    try {
      await api.deleteServer(server.id);
      await onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  const memoryShown =
    memoryUnit === "GB" ? draft.memory_limit_mb / 1024 : draft.memory_limit_mb;

  function label(key: string): string {
    const edition = key.includes("bedrock") ? "Bedrock" : "Java";
    const kind = key.includes("simulation") ? "Simulation distance" : "Render distance";
    return `${kind} (${edition})`;
  }

  return (
    <>
      {error && <div className="banner error">{error}</div>}
      {notice && <div className="banner ok">{notice}</div>}

      <div className="card">
        <div className="card-head">
          <span className="card-title">Resources</span>
          <span className="card-sub">how much of this machine the server may use</span>
        </div>
        <div className="card-body">
          <div className="field">
            <div>
              <div className="field-label">Memory to allow</div>
              <div className="field-help">
                The panel warns you when the server goes past this. Set 0 for no limit.
              </div>
            </div>
            <div className="field-control">
              <div className="row">
                <input
                  type="number"
                  min={0}
                  step={memoryUnit === "GB" ? 0.5 : 128}
                  value={memoryShown || ""}
                  placeholder="0"
                  onChange={(e) => {
                    const entered = parseFloat(e.target.value) || 0;
                    set("memory_limit_mb", Math.round(memoryUnit === "GB" ? entered * 1024 : entered));
                  }}
                />
                <select
                  style={{ width: 78 }}
                  value={memoryUnit}
                  onChange={(e) => setMemoryUnit(e.target.value as "MB" | "GB")}
                >
                  <option value="MB">MB</option>
                  <option value="GB">GB</option>
                </select>
              </div>
            </div>
          </div>

          <div className="field">
            <div>
              <div className="field-label">CPU cores to allow</div>
              <div className="field-help">
                This one is enforced: the server is pinned to that many cores when it starts.
                {hostCores > 0 && ` This machine has ${hostCores}.`}
              </div>
            </div>
            <div className="field-control">
              <select
                value={draft.cpu_cores}
                onChange={(e) => set("cpu_cores", parseInt(e.target.value, 10))}
              >
                <option value={0}>All cores</option>
                {Array.from({ length: Math.max(hostCores, 8) }, (_, i) => i + 1).map((n) => (
                  <option key={n} value={n}>
                    {n} core{n === 1 ? "" : "s"}
                  </option>
                ))}
              </select>
            </div>
          </div>

          <div className="field">
            <div>
              <div className="field-label">Disk space to allow</div>
              <div className="field-help">
                Measured across the whole server folder. Set 0 for no limit.
              </div>
            </div>
            <div className="field-control">
              <div className="row">
                <input
                  type="number"
                  min={0}
                  step={1}
                  value={draft.disk_limit_mb ? draft.disk_limit_mb / 1024 : ""}
                  placeholder="0"
                  onChange={(e) =>
                    set("disk_limit_mb", Math.round((parseFloat(e.target.value) || 0) * 1024))
                  }
                />
                <span className="dim" style={{ width: 78 }}>
                  GB
                </span>
              </div>
            </div>
          </div>
        </div>
      </div>

      {server && distanceFields.length > 0 && (
        <div className="card">
          <div className="card-head">
            <span className="card-title">Player distances</span>
            <span className="card-sub">the biggest lever on CPU and memory</span>
          </div>
          <div className="card-body">
            {distanceFields.map((field) => (
              <div className="field" key={field.key}>
                <div>
                  <div className="field-label">{label(field.key)}</div>
                  <div className="field-help">
                    {field.key.includes("simulation")
                      ? "How far from a player the world actually ticks. Mobs, crops and redstone stop outside this. Lower saves the most CPU."
                      : "How many chunks players can see. Lower means less to generate, hold in memory and send."}
                  </div>
                </div>
                <div className="field-control">
                  <div className="row">
                    <input
                      type="range"
                      min={2}
                      max={32}
                      value={distances[field.key] ?? 10}
                      onChange={(e) => {
                        setDistances({ ...distances, [field.key]: parseInt(e.target.value, 10) });
                        setDistanceDirty(true);
                      }}
                    />
                    <span className="mono nums" style={{ width: 78, textAlign: "right" }}>
                      {distances[field.key] ?? 10} chunks
                    </span>
                  </div>
                </div>
              </div>
            ))}

            <div style={{ paddingTop: 12 }}>
              <button
                className="btn primary"
                disabled={busy || !distanceDirty}
                onClick={() => void saveDistances()}
              >
                Save distances
              </button>
            </div>
          </div>
        </div>
      )}

      <div className="card">
        <div className="card-head">
          <span className="card-title">{server ? "Server" : "Add a server"}</span>
          <span className="card-sub">where the panel finds and launches it</span>
        </div>
        <div className="card-body">
          <div className="field">
            <div>
              <div className="field-label">Display name</div>
              <div className="field-help">Shown in the sidebar.</div>
            </div>
            <div className="field-control">
              <input type="text" value={draft.name} onChange={(e) => set("name", e.target.value)} />
            </div>
          </div>

          <div className="field">
            <div>
              <div className="field-label">Server program</div>
              <div className="field-help">
                Full path to the executable, for example{" "}
                <span className="mono">E:\mc plugins\pumpkin mc\pumpkin-X64-Windows.exe</span>
              </div>
            </div>
            <div className="field-control">
              <input
                type="text"
                value={draft.binary_path}
                onChange={(e) => set("binary_path", e.target.value)}
              />
            </div>
          </div>

          <div className="field">
            <div>
              <div className="field-label">Server folder</div>
              <div className="field-help">
                Where the world, configs and logs live. Leave empty to use the folder containing the
                program.
              </div>
            </div>
            <div className="field-control">
              <input
                type="text"
                value={draft.working_dir}
                onChange={(e) => set("working_dir", e.target.value)}
              />
            </div>
          </div>

          <div className="field">
            <div>
              <div className="field-label">Start on panel launch</div>
              <div className="field-help">Boot this server automatically when the panel starts.</div>
            </div>
            <div className="field-control">
              <button
                type="button"
                className={`switch ${draft.autostart ? "on" : ""}`}
                aria-pressed={draft.autostart}
                onClick={() => set("autostart", !draft.autostart)}
              />
            </div>
          </div>

          <div className="field">
            <div>
              <div className="field-label">Command line arguments</div>
              <div className="field-help">Extra arguments passed to the program. Rarely needed.</div>
            </div>
            <div className="field-control">
              <input type="text" value={draft.args} onChange={(e) => set("args", e.target.value)} />
            </div>
          </div>

          <div className="field">
            <div>
              <div className="field-label">Shutdown command</div>
              <div className="field-help">
                Typed into the server console for a clean shutdown before the process is killed.
              </div>
            </div>
            <div className="field-control">
              <input
                type="text"
                value={draft.stop_command}
                onChange={(e) => set("stop_command", e.target.value)}
              />
            </div>
          </div>
        </div>
      </div>

      <div className="row">
        <button className="btn primary" disabled={busy || !draft.name} onClick={() => void submit()}>
          {busy ? "Working…" : server ? "Save changes" : "Add server"}
        </button>
        {server && (
          <button className="btn danger" disabled={busy} onClick={() => void remove()}>
            Remove from panel
          </button>
        )}
      </div>
    </>
  );
}
