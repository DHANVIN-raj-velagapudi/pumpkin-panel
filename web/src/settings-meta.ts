// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
/**
 * Plain-English metadata for Pumpkin settings.
 *
 * `pumpkin.toml` carries no comments of its own, so a raw dump of it is a wall
 * of unexplained keys. This dictionary supplies a human label, an explanation,
 * and — for the settings people actually change — a place near the top of the
 * page. Anything not listed here still renders; it just falls back to a
 * humanised version of its key.
 */

export interface SettingMeta {
  label: string;
  help: string;
  /** Options for settings that are really an enum stored as a string. */
  options?: string[];
  /** Extra warning shown under the control. */
  warn?: string;
}

/** The settings surfaced in "Common settings", in the order shown. */
export const COMMON_KEYS: string[] = [
  "networking.java.motd",
  "networking.java.max_players",
  "default_difficulty",
  "default_gamemode",
  "pvp.enabled",
  "networking.java.online_mode",
  "white_list",
  "networking.java.view_distance",
  "networking.java.address",
  "networking.bedrock.enabled",
  "hardcore",
  "seed",
];

export const SETTINGS: Record<string, SettingMeta> = {
  // ---- the things people change constantly ----
  "networking.java.motd": {
    label: "Server description",
    help: "The line players see under your server in their multiplayer list.",
  },
  "networking.java.max_players": {
    label: "Max players",
    help: "How many people can be online at once. Set 0 for no limit.",
  },
  default_difficulty: {
    label: "Difficulty",
    help: "How hard mobs hit and whether hunger can kill you.",
    options: ["Peaceful", "Easy", "Normal", "Hard"],
  },
  default_gamemode: {
    label: "Default gamemode",
    help: "What new players start in.",
    options: ["Survival", "Creative", "Adventure", "Spectator"],
  },
  "pvp.enabled": {
    label: "Player vs player",
    help: "Whether players can damage each other.",
  },
  "networking.java.online_mode": {
    label: "Require Mojang accounts",
    help: "On, only players with a paid Minecraft account can join. Off allows any username, which is what you want for offline or cracked clients.",
    warn: "With this off, anyone can join using any name. Encryption must be off too.",
  },
  white_list: {
    label: "Whitelist",
    help: "Only players on the whitelist can join. Manage the list in the Players tab.",
  },
  "networking.java.view_distance": {
    label: "View distance",
    help: "How many chunks players can see. Lower values cost less memory and CPU.",
  },
  "networking.java.address": {
    label: "Java address and port",
    help: "Where the Java edition server listens. 0.0.0.0:25565 means every network interface on the standard port.",
  },
  "networking.bedrock.enabled": {
    label: "Allow Bedrock players",
    help: "Lets phone, console and Windows 10 edition players join the same world.",
  },
  hardcore: {
    label: "Hardcore",
    help: "Death is permanent and the difficulty is locked to Hard.",
  },
  seed: {
    label: "World seed",
    help: "The number the world was generated from. Changing this only affects newly generated worlds.",
  },

  // ---- everything else, still explained ----
  op_permission_level: {
    label: "Operator level",
    help: "Power granted by the op command, from 1 to 4. Level 4 is full access.",
  },
  allow_nether: { label: "Allow the Nether", help: "Enables the Nether dimension and its portals." },
  allow_end: { label: "Allow the End", help: "Enables the End dimension and the dragon fight." },
  tps: { label: "Ticks per second", help: "Game speed. 20 is normal; changing it alters everything." },
  force_gamemode: {
    label: "Force gamemode",
    help: "Resets players to the default gamemode each time they join.",
  },
  enforce_whitelist: {
    label: "Enforce whitelist",
    help: "Kicks players already online when they are removed from the whitelist.",
  },
  scrub_ips: { label: "Hide IPs in logs", help: "Removes player IP addresses from log output." },
  use_favicon: { label: "Server icon", help: "Shows a custom icon in the multiplayer list." },
  allow_chat_reports: {
    label: "Allow chat reports",
    help: "Lets players report chat to Mojang. Requires Mojang accounts to be on.",
  },
  default_level_name: { label: "World folder", help: "Name of the folder holding the world." },

  "networking.java.encryption": {
    label: "Packet encryption",
    help: "Encrypts traffic between client and server. Required when Mojang accounts are on, and must be off when they are off.",
  },
  "networking.java.simulation_distance": {
    label: "Simulation distance",
    help: "How many chunks actually tick around a player. The biggest single lever on CPU use.",
  },
  "networking.java.keep_alive_time": {
    label: "Keep-alive timeout",
    help: "Seconds before an unresponsive client is dropped.",
  },
  "networking.java.enabled": { label: "Java edition", help: "Accept Java edition players." },
  "networking.bedrock.address": {
    label: "Bedrock address and port",
    help: "Where Bedrock clients connect. 19132 is the standard port.",
  },
  "networking.query.enabled": {
    label: "Query service",
    help: "Publishes live player counts. The panel uses this for the Players tab, so leaving it on is recommended.",
  },
  "networking.rcon.enabled": {
    label: "RCON",
    help: "Remote console over TCP. The panel does not need it; leave it off unless another tool does.",
  },
  "networking.rcon.password": { label: "RCON password", help: "Required if RCON is enabled." },
  "networking.lan_broadcast.enabled": {
    label: "LAN broadcast",
    help: "Announces the server to the local network so it appears automatically.",
  },
  "networking.proxy.enabled": {
    label: "Behind a proxy",
    help: "Enable when running behind Velocity or BungeeCord.",
  },

  "chat.format": {
    label: "Chat format",
    help: "Template for chat lines. {DISPLAYNAME} and {MESSAGE} are replaced.",
  },
  "chat.anti_spam.enabled": { label: "Chat anti-spam", help: "Rate limits players who flood chat." },

  "logging.enabled": { label: "Logging", help: "Write server output to logs/latest.log." },
  "logging.color": { label: "Coloured logs", help: "ANSI colour in console output." },

  "world.autosave_ticks": {
    label: "Autosave interval",
    help: "Ticks between world saves. 0 disables autosaving.",
  },
  "world.chunk.compression.algorithm": {
    label: "Chunk compression",
    help: "How chunks are compressed on disk. LZ4 is fastest; others save space.",
  },

  "player_data.save_player_data": {
    label: "Save player data",
    help: "Persist inventories, positions and health between sessions.",
  },
  "player_data.save_player_cron_interval": {
    label: "Player save interval",
    help: "Seconds between player data saves.",
  },

  "plugins.enabled": { label: "Plugins", help: "Load plugins from the plugins folder." },
  "plugins.hot_reload": {
    label: "Plugin hot reload",
    help: "Reload plugins without restarting. Useful when developing.",
  },
  "plugins.allow_unsigned": {
    label: "Allow unsigned plugins",
    help: "Permits plugins that carry no marketplace signature.",
    warn: "Unsigned plugins have not been verified by anyone.",
  },
  "plugins.ask_permission_confirmation": {
    label: "Confirm plugin permissions",
    help: "Prompt before granting a plugin the capabilities it requests.",
  },

  "pvp.knockback": { label: "Knockback", help: "Players are pushed back when hit." },
  "pvp.hurt_animation": { label: "Hurt animation", help: "Show the red flash when damaged." },
  "pvp.protect_creative": { label: "Protect creative", help: "Creative players cannot be damaged." },
};

/** Turns `max_players` into `Max players` when nothing better is known. */
export function humanise(name: string): string {
  const spaced = name.replace(/_/g, " ").trim();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

export function metaFor(key: string, name: string): SettingMeta {
  return SETTINGS[key] ?? { label: humanise(name), help: "" };
}
