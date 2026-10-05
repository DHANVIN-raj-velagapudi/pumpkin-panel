// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
import { useCallback, useEffect, useRef, useState, type DragEvent } from "react";
import { api, ApiError, type FileEntry, type Server } from "../api";

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}

function formatTime(seconds: number): string {
  return seconds ? new Date(seconds * 1000).toLocaleString() : "—";
}

/**
 * A label for the file's format.
 *
 * Deliberately open-ended: plugins will ship configs in formats nobody has
 * thought of yet, so anything textual is editable and unknown extensions are
 * simply named rather than blocked.
 */
const FORMATS: Record<string, string> = {
  toml: "TOML",
  json: "JSON",
  json5: "JSON5",
  yml: "YAML",
  yaml: "YAML",
  properties: "Properties",
  conf: "Config",
  cfg: "Config",
  ini: "INI",
  env: "Env",
  txt: "Text",
  md: "Markdown",
  log: "Log",
  csv: "CSV",
  xml: "XML",
  html: "HTML",
  css: "CSS",
  js: "JavaScript",
  ts: "TypeScript",
  lua: "Lua",
  py: "Python",
  sh: "Shell",
  rs: "Rust",
  sql: "SQL",
  mcfunction: "Function",
  snbt: "SNBT",
};

/** Formats known to be binary; everything else is attempted as text. */
const BINARY = new Set([
  "exe", "dll", "so", "dylib", "jar", "zip", "gz", "tar", "7z", "rar",
  "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp",
  "mca", "mcr", "dat", "dat_old", "nbt", "der", "wasm", "bin", "db", "sqlite",
  "ogg", "wav", "mp3", "mp4",
]);

function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot === -1 ? "" : name.slice(dot + 1).toLowerCase();
}

function formatLabel(name: string): string {
  const ext = extensionOf(name);
  if (!ext) return "File";
  return FORMATS[ext] ?? ext.toUpperCase();
}

const ARCHIVES = new Set(["zip", "jar"]);

function isArchive(entry: FileEntry): boolean {
  return !entry.is_dir && ARCHIVES.has(extensionOf(entry.name));
}

function iconFor(entry: FileEntry): string {
  if (entry.is_dir) return "📁";
  const ext = extensionOf(entry.name);
  if (BINARY.has(ext)) return "📦";
  return FORMATS[ext] ? "📝" : "📄";
}

export default function Files({ server }: { server: Server }) {
  const [path, setPath] = useState("");
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [editing, setEditing] = useState<{ path: string; content: string } | null>(null);
  const [binaryNotice, setBinaryNotice] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);

  const load = useCallback(
    async (target: string) => {
      setError(null);
      try {
        const response = await api.listFiles(server.id, target);
        setEntries(response.entries);
        setPath(target);
        setSelected(new Set());
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [server.id],
  );

  useEffect(() => {
    setEditing(null);
    void load("");
  }, [load]);

  function flash(message: string) {
    setNotice(message);
    setTimeout(() => setNotice(null), 3500);
  }

  function toggle(path_: string) {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(path_)) next.delete(path_);
      else next.add(path_);
      return next;
    });
  }

  function toggleAll() {
    setSelected((current) =>
      current.size === entries.length ? new Set() : new Set(entries.map((e) => e.path)),
    );
  }

  async function open(entry: FileEntry) {
    if (entry.is_dir) {
      void load(entry.path);
      return;
    }

    setError(null);
    setBinaryNotice(null);

    if (BINARY.has(extensionOf(entry.name))) {
      setBinaryNotice(entry.path);
      return;
    }

    try {
      const file = await api.readFile(server.id, entry.path);
      setEditing({ path: entry.path, content: file.content });
      setDirty(false);
    } catch (e) {
      // The server refuses anything that is not valid UTF-8; offer the download.
      if (e instanceof ApiError && e.message.includes("UTF-8")) {
        setBinaryNotice(entry.path);
        return;
      }
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function save() {
    if (!editing) return;
    setBusy(true);
    try {
      await api.writeFile(server.id, editing.path, editing.content);
      setDirty(false);
      flash(`Saved ${editing.path}`);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function upload(files: FileList | File[]) {
    if (!files || files.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      const names = await api.uploadFiles(server.id, path, files);
      flash(`Uploaded ${names.length} file${names.length === 1 ? "" : "s"}`);
      await load(path);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function downloadZip(paths: string[]) {
    if (paths.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      const name = await api.archive(server.id, paths);
      flash(`Downloaded ${name}`);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  /** Asks for a destination folder, defaulting to where we already are. */
  function askDestination(what: string): string | null {
    const answer = prompt(
      `${what} to which folder?

Use a path relative to ${server.name}, or leave empty for the top level.`,
      path,
    );
    return answer === null ? null : answer.trim().replace(/^\/+|\/+$/g, "");
  }

  async function extract(entry: FileEntry) {
    setBusy(true);
    setError(null);
    try {
      const result = await api.extract(server.id, entry.path, path);
      flash(`Unpacked ${result.files} file${result.files === 1 ? "" : "s"}`);
      await load(path);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function transferSelected(mode: "move" | "copy") {
    const paths = [...selected];
    if (paths.length === 0) return;
    const destination = askDestination(mode === "move" ? "Move" : "Copy");
    if (destination === null) return;

    setBusy(true);
    setError(null);
    try {
      for (const source of paths) {
        const name = source.split("/").pop() ?? source;
        const target = destination ? `${destination}/${name}` : name;
        if (target === source) continue;
        if (mode === "move") await api.movePath(server.id, source, target);
        else await api.copyPath(server.id, source, target);
      }
      flash(`${mode === "move" ? "Moved" : "Copied"} ${paths.length} item(s)`);
      await load(path);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function removeSelected() {
    const paths = [...selected];
    if (paths.length === 0) return;
    if (!confirm(`Delete ${paths.length} item${paths.length === 1 ? "" : "s"}?\n\nThis cannot be undone.`)) {
      return;
    }
    setBusy(true);
    try {
      for (const target of paths) {
        const entry = entries.find((e) => e.path === target);
        await api.deleteFile(server.id, target, entry?.is_dir ?? false);
      }
      await load(path);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function remove(entry: FileEntry) {
    const what = entry.is_dir ? "folder and everything inside it" : "file";
    if (!confirm(`Delete this ${what}?\n\n${entry.path}`)) return;
    try {
      await api.deleteFile(server.id, entry.path, entry.is_dir);
      void load(path);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function newFolder() {
    const name = prompt("Folder name");
    if (!name) return;
    try {
      await api.mkdir(server.id, path ? `${path}/${name}` : name);
      void load(path);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function newFile() {
    const name = prompt("File name, including its extension\n\ne.g. config.yml, settings.json");
    if (!name) return;
    const target = path ? `${path}/${name}` : name;
    try {
      await api.writeFile(server.id, target, "");
      await load(path);
      setEditing({ path: target, content: "" });
      setDirty(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  function onDrop(event: DragEvent) {
    event.preventDefault();
    setDragging(false);
    if (event.dataTransfer?.files?.length) void upload(event.dataTransfer.files);
  }

  const segments = path.split("/").filter(Boolean);

  if (editing) {
    const lines = editing.content.split("\n").length;
    return (
      <>
        {error && <div className="banner error">{error}</div>}
        {notice && <div className="banner ok">{notice}</div>}

        <div className="breadcrumb">
          <button
            className="crumb"
            onClick={() => {
              if (dirty && !confirm("Discard unsaved changes?")) return;
              setEditing(null);
            }}
          >
            ← Files
          </button>
          <span className="dim mono">/{editing.path}</span>
          <span className="tag">{formatLabel(editing.path)}</span>
          <span className="dim nums">{lines} lines</span>
          {dirty && <span className="tag">unsaved</span>}
        </div>

        <textarea
          className="editor"
          spellCheck={false}
          value={editing.content}
          onChange={(e) => {
            setEditing({ ...editing, content: e.target.value });
            setDirty(true);
          }}
        />

        <div className="sticky-actions">
          <button className="btn primary" disabled={busy || !dirty} onClick={() => void save()}>
            {busy ? "Saving…" : "Save"}
          </button>
          <a className="btn" href={api.downloadUrl(server.id, editing.path)} download>
            Download
          </a>
          <span className="dim">Most changes need a server restart to apply.</span>
        </div>
      </>
    );
  }

  const allSelected = entries.length > 0 && selected.size === entries.length;

  return (
    <div
      onDragOver={(e) => {
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={onDrop}
    >
      {error && <div className="banner error">{error}</div>}
      {notice && <div className="banner ok">{notice}</div>}

      {binaryNotice && (
        <div className="banner info">
          <strong>{binaryNotice}</strong> is not a text file, so there is nothing to edit.{" "}
          <a className="crumb" href={api.downloadUrl(server.id, binaryNotice)} download>
            Download it instead
          </a>
          <button className="btn ghost sm spacer" onClick={() => setBinaryNotice(null)}>
            Dismiss
          </button>
        </div>
      )}

      <div className="breadcrumb">
        <button className="crumb" onClick={() => void load("")}>
          {server.name}
        </button>
        {segments.map((segment, index) => (
          <span key={index} className="row" style={{ gap: 3 }}>
            <span className="dim">/</span>
            <button className="crumb" onClick={() => void load(segments.slice(0, index + 1).join("/"))}>
              {segment}
            </button>
          </span>
        ))}

        <div className="spacer" />

        <input
          ref={inputRef}
          type="file"
          multiple
          hidden
          onChange={(e) => {
            if (e.target.files) void upload(e.target.files);
            e.target.value = "";
          }}
        />
        <button className="btn sm primary" disabled={busy} onClick={() => inputRef.current?.click()}>
          Upload
        </button>
        <button className="btn sm" onClick={() => void newFile()}>
          New file
        </button>
        <button className="btn sm" onClick={() => void newFolder()}>
          New folder
        </button>
      </div>

      {selected.size > 0 ? (
        <div className="selection-bar">
          <span>
            <strong className="nums">{selected.size}</strong> selected
          </span>
          <button className="btn sm primary" disabled={busy} onClick={() => void downloadZip([...selected])}>
            {busy ? "Working…" : "Download as zip"}
          </button>
          <button className="btn sm" disabled={busy} onClick={() => void transferSelected("move")}>
            Move to…
          </button>
          <button className="btn sm" disabled={busy} onClick={() => void transferSelected("copy")}>
            Copy to…
          </button>
          <button className="btn sm danger" disabled={busy} onClick={() => void removeSelected()}>
            Delete
          </button>
          <button className="btn ghost sm spacer" onClick={() => setSelected(new Set())}>
            Clear
          </button>
        </div>
      ) : (
        <div className={`dropzone ${dragging ? "over" : ""}`}>
          {dragging ? "Drop to upload here" : `Drop files here to upload into /${path || server.name}`}
        </div>
      )}

      <div className="card">
        <table className="grid">
          <thead>
            <tr>
              <th style={{ width: 34 }}>
                <input
                  type="checkbox"
                  checked={allSelected}
                  onChange={toggleAll}
                  aria-label="Select all"
                />
              </th>
              <th>Name</th>
              <th style={{ width: 96 }}>Type</th>
              <th style={{ width: 100 }}>Size</th>
              <th style={{ width: 170 }}>Modified</th>
              <th style={{ width: 170 }} />
            </tr>
          </thead>
          <tbody>
            {path !== "" && (
              <tr>
                <td />
                <td colSpan={5}>
                  <button className="link-cell" onClick={() => void load(segments.slice(0, -1).join("/"))}>
                    ../
                  </button>
                </td>
              </tr>
            )}
            {entries.map((entry) => (
              <tr key={entry.path} className={selected.has(entry.path) ? "selected" : ""}>
                <td>
                  <input
                    type="checkbox"
                    checked={selected.has(entry.path)}
                    onChange={() => toggle(entry.path)}
                    aria-label={`Select ${entry.name}`}
                  />
                </td>
                <td>
                  <button className="link-cell" onClick={() => void open(entry)}>
                    {iconFor(entry)} {entry.name}
                  </button>
                </td>
                <td className="dim">{entry.is_dir ? "Folder" : formatLabel(entry.name)}</td>
                <td className="muted nums">{entry.is_dir ? "—" : formatSize(entry.size)}</td>
                <td className="muted">{formatTime(entry.modified)}</td>
                <td>
                  <div className="row">
                    <button
                      className="btn sm"
                      disabled={busy}
                      title={entry.is_dir ? "Download this folder as a zip" : "Download this file"}
                      onClick={() =>
                        entry.is_dir
                          ? void downloadZip([entry.path])
                          : window.location.assign(api.downloadUrl(server.id, entry.path))
                      }
                    >
                      Download
                    </button>
                    <button className="btn sm danger" onClick={() => void remove(entry)}>
                      Delete
                    </button>
                  </div>
                </td>
              </tr>
            ))}
            {entries.length === 0 && (
              <tr>
                <td colSpan={6}>
                  <div className="empty">
                    <div className="empty-title">This folder is empty</div>
                    Drop files here, or use Upload and New file.
                  </div>
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
