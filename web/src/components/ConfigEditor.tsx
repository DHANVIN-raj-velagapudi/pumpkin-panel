// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
import { useCallback, useEffect, useMemo, useState } from "react";
import { api, type ConfigField, type Server } from "../api";
import { COMMON_KEYS, metaFor } from "../settings-meta";

export default function ConfigEditor({ server }: { server: Server }) {
  const [files, setFiles] = useState<string[]>([]);
  const [file, setFile] = useState<string>("");
  const [fields, setFields] = useState<ConfigField[]>([]);
  const [raw, setRaw] = useState("");
  const [rawMode, setRawMode] = useState(false);
  const [rawDraft, setRawDraft] = useState("");
  const [edits, setEdits] = useState<Record<string, unknown>>({});
  const [search, setSearch] = useState("");
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api
      .configFiles(server.id)
      .then((response) => {
        setFiles(response.files);
        setFile((current) =>
          current && response.files.includes(current)
            ? current
            : (response.files.find((f) => f === "pumpkin.toml") ?? response.files[0] ?? ""),
        );
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
  }, [server.id]);

  const load = useCallback(async () => {
    if (!file) return;
    setError(null);
    try {
      const response = await api.readConfig(server.id, file);
      setFields(response.fields);
      setRaw(response.raw);
      setRawDraft(response.raw);
      setEdits({});
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [server.id, file]);

  useEffect(() => {
    void load();
  }, [load]);

  const byKey = useMemo(() => {
    const map = new Map<string, ConfigField>();
    for (const field of fields) map.set(field.key, field);
    return map;
  }, [fields]);

  const common = useMemo(
    () => COMMON_KEYS.map((key) => byKey.get(key)).filter((f): f is ConfigField => Boolean(f)),
    [byKey],
  );

  const query = search.trim().toLowerCase();

  const matches = useCallback(
    (field: ConfigField) => {
      if (!query) return true;
      const meta = metaFor(field.key, field.name);
      return (
        field.key.toLowerCase().includes(query) ||
        meta.label.toLowerCase().includes(query) ||
        meta.help.toLowerCase().includes(query)
      );
    },
    [query],
  );

  // When searching, everything is shown flat; otherwise grouped by TOML table.
  const grouped = useMemo(() => {
    const map = new Map<string, ConfigField[]>();
    for (const field of fields) {
      if (!matches(field)) continue;
      const list = map.get(field.section) ?? [];
      list.push(field);
      map.set(field.section, list);
    }
    return [...map.entries()];
  }, [fields, matches]);

  function setValue(key: string, value: unknown, original: unknown) {
    setEdits((current) => {
      const next = { ...current };
      if (JSON.stringify(value) === JSON.stringify(original)) delete next[key];
      else next[key] = value;
      return next;
    });
  }

  async function save() {
    setBusy(true);
    setError(null);
    try {
      const response = rawMode
        ? await api.writeConfigRaw(server.id, file, rawDraft)
        : await api.writeConfig(server.id, file, edits);
      setNotice(response.note ?? "Saved");
      setTimeout(() => setNotice(null), 5000);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  const changed = Object.keys(edits).length;
  const canSave = rawMode ? rawDraft !== raw : changed > 0;

  const renderField = (field: ConfigField) => {
    const meta = metaFor(field.key, field.name);
    const isChanged = field.key in edits;
    const value = isChanged ? edits[field.key] : field.value;

    return (
      <div className={`field ${isChanged ? "changed" : ""}`} key={field.key}>
        <div>
          <div className="field-label">{meta.label}</div>
          {meta.help && <div className="field-help">{meta.help}</div>}
          {meta.warn && <div className="field-warn">{meta.warn}</div>}
          <div className="field-key">{field.key}</div>
        </div>
        <div className="field-control">
          <FieldInput
            field={field}
            options={meta.options}
            value={value}
            onChange={(next) => setValue(field.key, next, field.value)}
          />
        </div>
      </div>
    );
  };

  return (
    <>
      {error && <div className="banner error">{error}</div>}
      {notice && <div className="banner ok">{notice}</div>}

      <div className="toolbar">
        <input
          className="search"
          type="search"
          placeholder="Search settings…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          disabled={rawMode}
        />
        {files.length > 1 && (
          <select value={file} style={{ width: 190 }} onChange={(e) => setFile(e.target.value)}>
            {files.map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
          </select>
        )}
        <div className="spacer" />
        <span className="dim">{fields.length} settings</span>
        <button className="btn sm" onClick={() => setRawMode((m) => !m)}>
          {rawMode ? "Form view" : "Edit raw TOML"}
        </button>
      </div>

      {rawMode ? (
        <textarea
          className="editor"
          spellCheck={false}
          value={rawDraft}
          onChange={(e) => setRawDraft(e.target.value)}
        />
      ) : (
        <>
          {!query && common.length > 0 && (
            <div className="card">
              <div className="card-head">
                <span className="card-title">Common settings</span>
                <span className="card-sub">the ones people change most often</span>
              </div>
              <div className="card-body">{common.map(renderField)}</div>
            </div>
          )}

          {query && grouped.length === 0 && (
            <div className="empty">
              <div className="empty-title">No settings match “{search}”</div>
              Try a different word, or switch to raw TOML.
            </div>
          )}

          {grouped.map(([section, sectionFields]) => {
            const title = section === "" ? "General" : section;
            // Sections start collapsed unless searching, so the page opens calm.
            const isOpen = query ? true : (open[title] ?? false);
            return (
              <div className="card" key={title || "general"}>
                <button className="section-toggle" onClick={() => setOpen({ ...open, [title]: !isOpen })}>
                  <span className={`chevron ${isOpen ? "open" : ""}`}>▶</span>
                  <span className="card-title">{title}</span>
                  <span className="card-sub">
                    {sectionFields.length} setting{sectionFields.length === 1 ? "" : "s"}
                  </span>
                </button>
                {isOpen && <div className="card-body">{sectionFields.map(renderField)}</div>}
              </div>
            );
          })}
        </>
      )}

      <div className="sticky-actions">
        <button className="btn primary" disabled={busy || !canSave} onClick={() => void save()}>
          {busy ? "Saving…" : rawMode ? "Save file" : changed ? `Save ${changed} change${changed === 1 ? "" : "s"}` : "Save"}
        </button>
        <button
          className="btn"
          disabled={busy || !canSave}
          onClick={() => {
            setEdits({});
            setRawDraft(raw);
          }}
        >
          Discard
        </button>
        <span className="dim">Restart the server for changes to take effect. A .bak is kept.</span>
      </div>
    </>
  );
}

function FieldInput({
  field,
  value,
  options,
  onChange,
}: {
  field: ConfigField;
  value: unknown;
  options?: string[];
  onChange: (value: unknown) => void;
}) {
  if (field.kind === "boolean") {
    const on = value === true;
    return (
      <button
        type="button"
        className={`switch ${on ? "on" : ""}`}
        aria-pressed={on}
        onClick={() => onChange(!on)}
      />
    );
  }

  if (options && field.kind === "string") {
    const current = typeof value === "string" ? value : "";
    return (
      <select value={current} onChange={(e) => onChange(e.target.value)}>
        {!options.includes(current) && <option value={current}>{current || "(unset)"}</option>}
        {options.map((option) => (
          <option key={option} value={option}>
            {option}
          </option>
        ))}
      </select>
    );
  }

  if (field.kind === "integer" || field.kind === "float") {
    return (
      <input
        type="number"
        step={field.kind === "float" ? "any" : 1}
        value={typeof value === "number" ? value : ""}
        onChange={(e) => {
          const parsed =
            field.kind === "float" ? parseFloat(e.target.value) : parseInt(e.target.value, 10);
          onChange(Number.isNaN(parsed) ? 0 : parsed);
        }}
      />
    );
  }

  if (field.kind === "array") {
    const items = Array.isArray(value) ? value : [];
    return (
      <input
        type="text"
        value={items.map(String).join(", ")}
        placeholder="comma separated"
        onChange={(e) =>
          onChange(
            e.target.value
              .split(",")
              .map((part) => part.trim())
              .filter(Boolean),
          )
        }
      />
    );
  }

  if (field.kind === "unsupported") {
    return <span className="dim">edit in raw TOML</span>;
  }

  return (
    <input
      type="text"
      value={typeof value === "string" ? value : ""}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}
