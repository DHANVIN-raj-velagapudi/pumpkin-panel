// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
export type Role = "admin" | "user";

export type Status = "stopped" | "starting" | "running" | "stopping" | "crashed";

export interface User {
  id: string;
  username: string;
  email: string | null;
  role: Role;
}

export interface Access {
  console: boolean;
  power: boolean;
  files: boolean;
  config: boolean;
}

export interface Runtime {
  status: Status;
  pid: number | null;
  started_at: number | null;
  exit_code: number | null;
}

export interface Stats {
  running: boolean;
  memory_bytes: number;
  cpu_percent: number;
  disk_bytes: number;
  memory_limit_mb: number;
  disk_limit_mb: number;
  cpu_cores: number;
  host_memory_bytes: number;
  host_cores: number;
}

export interface ActivityEvent {
  seq: number;
  at: number;
  actor: string;
  action: string;
  category: string;
  server_id: string | null;
  target: string | null;
  result: "SUCCESS" | "FAILURE" | "DENIED";
  detail: string | null;
  meta: Record<string, unknown> | null;
  ip: string | null;
  request_id: string;
  hash: string;
}

export interface IntegrityReport {
  checked: number;
  intact: boolean;
  broken_at: number | null;
  message: string;
  head: string | null;
}

export interface PumpkinVersion {
  pumpkin_version: string | null;
  java_version: string | null;
  java_protocol: number | null;
  bedrock_version: string | null;
  bedrock_protocol: number | null;
}

export interface PluginFile {
  file: string;
  size_bytes: number;
  modified: number;
  name: string | null;
  version: string | null;
  developer: string | null;
  paid: boolean | null;
  is_wasm: boolean;
}

export interface PluginPolicy {
  enabled: boolean;
  hot_reload: boolean;
  allow_unsigned: boolean;
  ask_permission_confirmation: boolean;
  allowed_permissions: string[];
  blocked_permissions: string[];
  inherit_env: boolean;
  loopback_only: boolean;
}

export interface MarketPlugin {
  id: number;
  name: string;
  category: string;
  dev_name: string;
  downloads: number;
  views: number;
  price: number;
  price_cents: number;
  version: string;
  is_early_access: boolean;
  preview_path: string;
  translated_descriptions: string;
}

export interface Sample {
  at: number;
  cpu_percent: number;
  memory_bytes: number;
}

export interface PlayerEntry {
  name: string;
  uuid: string | null;
  reason: string | null;
  source: string | null;
  expires: string | null;
  level: number | null;
}

export interface PlayersResponse {
  online: string[];
  online_count: number;
  max_players: number;
  operators: PlayerEntry[];
  banned: PlayerEntry[];
  banned_ips: PlayerEntry[];
  whitelist: PlayerEntry[];
  known: PlayerEntry[];
  query_error: string | null;
}

export type PlayerActionName =
  | "op" | "deop" | "kick" | "ban" | "pardon" | "ban_ip" | "pardon_ip"
  | "heal" | "feed" | "gamemode" | "whitelist_add" | "whitelist_remove" | "kill";

export interface Server {
  id: string;
  name: string;
  binary_path: string;
  working_dir: string;
  args: string;
  stop_command: string;
  autostart: boolean;
  created_at: number;
  memory_limit_mb: number;
  disk_limit_mb: number;
  cpu_cores: number;
  runtime: Runtime;
  access: Access;
}

export interface ConsoleLine {
  stream: "stdout" | "stderr" | "system";
  line: string;
  at: number;
}

export interface FileEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  modified: number;
}

export interface ConfigField {
  key: string;
  section: string;
  name: string;
  value: unknown;
  kind: "string" | "integer" | "float" | "boolean" | "array" | "unsupported";
  description: string | null;
}

export class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
    /** Set when the password was accepted but a second factor is still needed. */
    public mfaRequired = false,
  ) {
    super(message);
  }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(`/api${path}`, {
    ...init,
    credentials: "include",
    headers:
      init.body === undefined
        ? init.headers
        : { "Content-Type": "application/json", ...init.headers },
  });

  if (!response.ok) {
    let message = response.statusText;
    let mfaRequired = false;
    try {
      const body = await response.json();
      if (body?.error) message = body.error;
      if (body?.mfa_required) mfaRequired = true;
    } catch {
      // Response had no JSON body; the status text will do.
    }
    throw new ApiError(message, response.status, mfaRequired);
  }

  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}

const get = <T,>(path: string) => request<T>(path);
const post = <T,>(path: string, body?: unknown) =>
  request<T>(path, { method: "POST", body: body === undefined ? undefined : JSON.stringify(body) });
const put = <T,>(path: string, body: unknown) =>
  request<T>(path, { method: "PUT", body: JSON.stringify(body) });
const patch = <T,>(path: string, body: unknown) =>
  request<T>(path, { method: "PATCH", body: JSON.stringify(body) });
const del = <T,>(path: string) => request<T>(path, { method: "DELETE" });

export const api = {
  me: () => get<{ user: User }>("/auth/me"),
  login: (username: string, password: string, code?: string) =>
    post<{ user: User }>("/auth/login", { username, password, code }),
  logout: () => post<{ ok: boolean }>("/auth/logout"),

  servers: () => get<Server[]>("/servers"),
  server: (id: string) => get<Server>(`/servers/${id}`),
  createServer: (body: Record<string, unknown>) => post<{ id: string }>("/servers", body),
  updateServer: (id: string, body: Record<string, unknown>) =>
    patch<{ ok: boolean }>(`/servers/${id}`, body),
  deleteServer: (id: string) => del<{ ok: boolean }>(`/servers/${id}`),
  power: (id: string, action: "start" | "stop" | "restart" | "kill") =>
    post<{ ok: boolean }>(`/servers/${id}/power`, { action }),

  consoleHistory: (id: string) =>
    get<{ lines: ConsoleLine[]; runtime: Runtime }>(`/servers/${id}/console`),

  listFiles: (id: string, path: string) =>
    get<{ path: string; entries: FileEntry[] }>(
      `/servers/${id}/files?path=${encodeURIComponent(path)}`,
    ),
  readFile: (id: string, path: string) =>
    get<{ path: string; content: string }>(
      `/servers/${id}/files/content?path=${encodeURIComponent(path)}`,
    ),
  writeFile: (id: string, path: string, content: string) =>
    put<{ ok: boolean }>(`/servers/${id}/files/content`, { path, content }),
  deleteFile: (id: string, path: string, recursive: boolean) =>
    post<{ ok: boolean }>(`/servers/${id}/files/delete`, { path, recursive }),
  mkdir: (id: string, path: string) => post<{ ok: boolean }>(`/servers/${id}/files/mkdir`, { path }),

  stats: (id: string) => get<Stats>(`/servers/${id}/stats`),
  statsHistory: (id: string) => get<{ samples: Sample[] }>(`/servers/${id}/stats/history`),

  players: (id: string) => get<PlayersResponse>(`/servers/${id}/players`),
  playerAction: (id: string, action: PlayerActionName, target: string, value?: string) =>
    post<{ ok: boolean; command: string }>(`/servers/${id}/players/action`, { action, target, value }),

  uploadFiles: async (id: string, path: string, files: FileList | File[]) => {
    const results: string[] = [];
    for (const file of Array.from(files)) {
      const form = new FormData();
      form.append("path", path);
      form.append("file", file);
      const response = await fetch(`/api/servers/${id}/files/upload`, {
        method: "POST",
        credentials: "include",
        body: form,
      });
      if (!response.ok) {
        let message = response.statusText;
        try {
          const body = await response.json();
          if (body?.error) message = body.error;
        } catch {
          // keep the status text
        }
        throw new ApiError(`${file.name}: ${message}`, response.status);
      }
      results.push(file.name);
    }
    return results;
  },
  /** Zips a selection server-side and hands the browser the result. */
  archive: async (id: string, paths: string[]) => {
    const response = await fetch(`/api/servers/${id}/files/archive`, {
      method: "POST",
      credentials: "include",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ paths }),
    });

    if (!response.ok) {
      let message = response.statusText;
      try {
        const body = await response.json();
        if (body?.error) message = body.error;
      } catch {
        // keep the status text
      }
      throw new ApiError(message, response.status);
    }

    const disposition = response.headers.get("content-disposition") ?? "";
    const name = /filename="?([^"]+)"?/.exec(disposition)?.[1] ?? "files.zip";

    const blob = await response.blob();
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = name;
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    URL.revokeObjectURL(url);
    return name;
  },

  downloadUrl: (id: string, path: string) =>
    `/api/servers/${id}/files/download?path=${encodeURIComponent(path)}`,

  extract: (id: string, path: string, destination?: string) =>
    post<{ ok: boolean; files: number }>(`/servers/${id}/files/extract`, { path, destination }),
  copyPath: (id: string, from: string, to: string) =>
    post<{ ok: boolean; files: number }>(`/servers/${id}/files/copy`, { from, to }),
  movePath: (id: string, from: string, to: string) =>
    post<{ ok: boolean }>(`/servers/${id}/files/rename`, { from, to }),

  configFiles: (id: string) => get<{ files: string[] }>(`/servers/${id}/config/files`),
  readConfig: (id: string, file: string) =>
    get<{ file: string; raw: string; fields: ConfigField[]; sections: string[] }>(
      `/servers/${id}/config?file=${encodeURIComponent(file)}`,
    ),
  writeConfig: (id: string, file: string, updates: Record<string, unknown>) =>
    put<{ ok: boolean; note: string }>(`/servers/${id}/config`, { file, updates }),
  writeConfigRaw: (id: string, file: string, raw: string) =>
    put<{ ok: boolean; note: string }>(`/servers/${id}/config`, { file, raw }),

  pumpkin: (id: string) =>
    get<{ version: PumpkinVersion | null; plugins: PluginFile[]; policy: PluginPolicy }>(
      `/servers/${id}/pumpkin`,
    ),
  marketplace: (search?: string) =>
    get<{ plugins: MarketPlugin[]; source: string }>(
      `/marketplace${search ? `?search=${encodeURIComponent(search)}` : ""}`,
    ),

  mfaStatus: () => get<{ enabled: boolean; recovery_codes_remaining: number }>("/auth/2fa"),
  mfaSetup: () => post<{ secret: string; uri: string; qr: string }>("/auth/2fa/setup"),
  mfaEnable: (code: string) =>
    post<{ ok: boolean; recovery_codes: string[]; note: string }>("/auth/2fa/enable", { code }),
  mfaDisable: (password: string) => post<{ ok: boolean }>("/auth/2fa/disable", { password }),
  logoutEverywhere: () => post<{ ok: boolean }>("/auth/logout-all"),

  activity: (params: {
    category?: string;
    search?: string;
    before?: number;
    limit?: number;
  }) => {
    const query = new URLSearchParams();
    if (params.category && params.category !== "all") query.set("category", params.category);
    if (params.search) query.set("search", params.search);
    if (params.before !== undefined) query.set("before", String(params.before));
    if (params.limit !== undefined) query.set("limit", String(params.limit));
    return get<{ events: ActivityEvent[]; oldest_seq: number | null; has_more: boolean }>(
      `/activity?${query.toString()}`,
    );
  },
  verifyActivity: () => get<IntegrityReport>("/activity/verify"),
  activityCheckpoint: () =>
    get<{ seq: number | null; hash: string | null; note: string }>("/activity/checkpoint"),

  users: () => get<User[]>("/users"),
  createUser: (body: Record<string, unknown>) => post<{ id: string }>("/users", body),
  updateUser: (id: string, body: Record<string, unknown>) =>
    patch<{ ok: boolean }>(`/users/${id}`, body),
  deleteUser: (id: string) => del<{ ok: boolean }>(`/users/${id}`),
  setGrant: (userId: string, body: Record<string, unknown>) =>
    put<{ ok: boolean }>(`/users/${userId}/servers`, body),
  grants: (userId: string) =>
    get<Array<{ server_id: string; can_console: boolean; can_power: boolean; can_files: boolean; can_config: boolean }>>(
      `/users/${userId}/servers`,
    ),
};

export function consoleSocket(id: string): WebSocket {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  return new WebSocket(`${protocol}//${window.location.host}/api/servers/${id}/console/ws`);
}
