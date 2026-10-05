// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
import { useCallback, useEffect, useState } from "react";
import {
  api,
  type MarketPlugin,
  type PluginFile,
  type PluginPolicy,
  type PumpkinVersion,
  type Server,
} from "../api";

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
}

/** The marketplace ships descriptions as a JSON map of locale to text. */
function describe(plugin: MarketPlugin): string {
  try {
    const map = JSON.parse(plugin.translated_descriptions) as Record<string, string>;
    const text = map["en-US"] ?? Object.values(map)[0] ?? "";
    return text.replace(/[#*`]/g, "").split("\n").filter(Boolean)[0] ?? "";
  } catch {
    return "";
  }
}

export default function Plugins({ server }: { server: Server }) {
  const [version, setVersion] = useState<PumpkinVersion | null>(null);
  const [installed, setInstalled] = useState<PluginFile[]>([]);
  const [policy, setPolicy] = useState<PluginPolicy | null>(null);
  const [market, setMarket] = useState<MarketPlugin[]>([]);
  const [search, setSearch] = useState("");
  const [showMarket, setShowMarket] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loadingMarket, setLoadingMarket] = useState(false);

  const load = useCallback(async () => {
    try {
      const data = await api.pumpkin(server.id);
      setVersion(data.version);
      setInstalled(data.plugins);
      setPolicy(data.policy);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [server.id]);

  useEffect(() => {
    void load();
  }, [load]);

  const loadMarket = useCallback(async () => {
    setLoadingMarket(true);
    try {
      const data = await api.marketplace(search || undefined);
      setMarket(data.plugins);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingMarket(false);
    }
  }, [search]);

  useEffect(() => {
    if (!showMarket) return;
    const timer = setTimeout(() => void loadMarket(), search ? 300 : 0);
    return () => clearTimeout(timer);
  }, [showMarket, loadMarket, search]);

  const isInstalled = (name: string) =>
    installed.some(
      (p) =>
        p.name?.toLowerCase() === name.toLowerCase() ||
        p.file.toLowerCase().startsWith(name.toLowerCase()),
    );

  return (
    <>
      {error && <div className="banner error">{error}</div>}

      <div className="card">
        <div className="card-head">
          <span className="card-title">This server</span>
          <span className="card-sub">read from what Pumpkin reports at startup</span>
        </div>
        <div className="card-body pad">
          {version?.pumpkin_version ? (
            <div className="stat-grid" style={{ marginBottom: 0 }}>
              <div className="stat">
                <div className="stat-label">Pumpkin</div>
                <div className="stat-value" style={{ fontSize: 17 }}>
                  {version.pumpkin_version}
                </div>
              </div>
              <div className="stat">
                <div className="stat-label">Java edition</div>
                <div className="stat-value" style={{ fontSize: 17 }}>
                  {version.java_version ?? "—"}
                </div>
                <div className="stat-foot">protocol {version.java_protocol ?? "?"}</div>
              </div>
              <div className="stat">
                <div className="stat-label">Bedrock edition</div>
                <div className="stat-value" style={{ fontSize: 17 }}>
                  {version.bedrock_version ?? "—"}
                </div>
                <div className="stat-foot">protocol {version.bedrock_protocol ?? "?"}</div>
              </div>
            </div>
          ) : (
            <p className="muted" style={{ margin: 0 }}>
              Start the server once and its version, and both protocol numbers, will be read from
              the startup banner. Players must be on a client matching the version shown here.
            </p>
          )}
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <span className="card-title">Installed plugins</span>
          <span className="card-sub">
            {installed.length} in the plugins folder
          </span>
          <button className="btn sm spacer" onClick={() => void load()}>
            Refresh
          </button>
        </div>
        <div className="card-body" style={{ padding: 0 }}>
          {installed.length === 0 ? (
            <div className="empty">
              <div className="empty-title">No plugins installed</div>
              Drop a <span className="mono">.wasm</span> file into the plugins folder, or browse the
              marketplace below.
            </div>
          ) : (
            <table className="grid">
              <thead>
                <tr>
                  <th>File</th>
                  <th style={{ width: 150 }}>Plugin</th>
                  <th style={{ width: 110 }}>Type</th>
                  <th style={{ width: 90 }}>Size</th>
                </tr>
              </thead>
              <tbody>
                {installed.map((plugin) => (
                  <tr key={plugin.file}>
                    <td className="mono">{plugin.file}</td>
                    <td>
                      {plugin.name ? (
                        <>
                          {plugin.name}
                          {plugin.version && <span className="dim"> {plugin.version}</span>}
                          {plugin.developer && (
                            <div className="dim" style={{ fontSize: 12 }}>
                              by {plugin.developer}
                            </div>
                          )}
                        </>
                      ) : (
                        <span className="dim">unpublished build</span>
                      )}
                    </td>
                    <td>
                      {plugin.is_wasm ? (
                        <span className="tag op">WASM</span>
                      ) : (
                        <span className="tag">not a component</span>
                      )}
                    </td>
                    <td className="muted nums">{formatSize(plugin.size_bytes)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>

      {policy && (
        <div className="card">
          <div className="card-head">
            <span className="card-title">Sandbox policy</span>
            <span className="card-sub">what plugins are allowed to reach</span>
          </div>
          <div className="card-body pad">
            <p className="muted" style={{ marginTop: 0 }}>
              Pumpkin runs plugins as sandboxed WebAssembly. They only get the capabilities they ask
              for, and only if this policy permits them.
            </p>
            <div className="policy-grid">
              <Flag on={policy.enabled} label="Plugins loaded" />
              <Flag on={policy.ask_permission_confirmation} label="Confirm permissions" good />
              <Flag on={!policy.allow_unsigned} label="Signed plugins only" good />
              <Flag on={policy.loopback_only} label="Network limited to localhost" good />
              <Flag on={!policy.inherit_env} label="Environment hidden" good />
              <Flag on={policy.hot_reload} label="Hot reload" />
            </div>

            <div style={{ marginTop: 16 }}>
              <div className="field-label">Permissions allowed without asking</div>
              <div className="muted" style={{ fontSize: 12.5, marginTop: 4 }}>
                {policy.allowed_permissions.length > 0
                  ? policy.allowed_permissions.join(", ")
                  : "None — every capability a plugin requests will prompt."}
              </div>
            </div>
            {policy.blocked_permissions.length > 0 && (
              <div style={{ marginTop: 12 }}>
                <div className="field-label">Always refused</div>
                <div className="muted" style={{ fontSize: 12.5, marginTop: 4 }}>
                  {policy.blocked_permissions.join(", ")}
                </div>
              </div>
            )}
            <p className="dim" style={{ fontSize: 12.5, marginBottom: 0, marginTop: 14 }}>
              Change these under Settings → <span className="mono">plugins</span>.
            </p>
          </div>
        </div>
      )}

      <div className="card">
        <div className="card-head">
          <span className="card-title">Marketplace</span>
          <span className="card-sub">market.pumpkinmc.org</span>
          <button className="btn sm spacer" onClick={() => setShowMarket((v) => !v)}>
            {showMarket ? "Hide" : "Browse"}
          </button>
        </div>
        {showMarket && (
          <div className="card-body pad">
            <input
              type="search"
              className="search"
              style={{ maxWidth: 340, marginBottom: 14 }}
              placeholder="Search plugins…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />

            {loadingMarket && market.length === 0 ? (
              <div className="empty">Loading…</div>
            ) : market.length === 0 ? (
              <div className="empty">Nothing matched.</div>
            ) : (
              <div className="market-grid">
                {market.map((plugin) => (
                  <div className="market-card" key={plugin.id}>
                    <div className="row" style={{ gap: 8 }}>
                      <strong>{plugin.name}</strong>
                      {plugin.price_cents > 0 ? (
                        <span className="tag op">${plugin.price.toFixed(2)}</span>
                      ) : (
                        <span className="tag">free</span>
                      )}
                      {isInstalled(plugin.name) && <span className="tag op">installed</span>}
                    </div>
                    <div className="dim" style={{ fontSize: 12 }}>
                      {plugin.category} · by {plugin.dev_name} · {plugin.downloads} downloads
                    </div>
                    <p className="muted" style={{ fontSize: 12.5, margin: "8px 0 0" }}>
                      {describe(plugin).slice(0, 130)}
                    </p>
                  </div>
                ))}
              </div>
            )}

            <p className="dim" style={{ fontSize: 12.5, marginBottom: 0, marginTop: 14 }}>
              Browsing only for now. Download a plugin from the marketplace and upload it to the
              plugins folder under Files.
            </p>
          </div>
        )}
      </div>
    </>
  );
}

function Flag({ on, label, good }: { on: boolean; label: string; good?: boolean }) {
  // `good` marks settings where "on" is the safer choice, so the colour means
  // something rather than just repeating the boolean.
  const tone = good ? (on ? "ok" : "warn") : on ? "on" : "off";
  return (
    <div className={`policy-flag ${tone}`}>
      <span>{on ? "✓" : "✕"}</span>
      {label}
    </div>
  );
}
