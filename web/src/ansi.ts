/**
 * Minimal ANSI SGR parser.
 *
 * Pumpkin ships with `color = true` under `[logging]`, so every console line
 * arrives wrapped in escape sequences. Rendering them as styled spans keeps the
 * panel console looking like the terminal one instead of showing raw `[32m`
 * noise.
 */

export interface Segment {
  text: string;
  className: string;
}

const FOREGROUND: Record<number, string> = {
  30: "ansi-black",
  31: "ansi-red",
  32: "ansi-green",
  33: "ansi-yellow",
  34: "ansi-blue",
  35: "ansi-magenta",
  36: "ansi-cyan",
  37: "ansi-white",
  90: "ansi-bright-black",
  91: "ansi-red",
  92: "ansi-green",
  93: "ansi-yellow",
  94: "ansi-blue",
  95: "ansi-magenta",
  96: "ansi-cyan",
  97: "ansi-white",
};

// Matches a CSI colour sequence. The escape byte is required, so a chat
// message that happens to contain "[0m" is left alone.
const PATTERN = /\x1b\[([0-9;]*)m/g;

export function parseAnsi(input: string): Segment[] {
  const segments: Segment[] = [];
  let color = "";
  let dim = false;
  let bold = false;
  let cursor = 0;

  const push = (text: string) => {
    if (text === "") return;
    const classes = [color, dim ? "ansi-dim" : "", bold ? "ansi-bold" : ""]
      .filter(Boolean)
      .join(" ");
    segments.push({ text, className: classes });
  };

  for (const match of input.matchAll(PATTERN)) {
    const index = match.index ?? 0;
    push(input.slice(cursor, index));
    cursor = index + match[0].length;

    // An empty parameter list means reset, same as `0`.
    const codes = (match[1] === "" ? "0" : match[1]).split(";").map(Number);
    for (const code of codes) {
      if (code === 0) {
        color = "";
        dim = false;
        bold = false;
      } else if (code === 1) {
        bold = true;
      } else if (code === 2) {
        dim = true;
      } else if (code === 22) {
        dim = false;
        bold = false;
      } else if (code === 39) {
        color = "";
      } else if (FOREGROUND[code]) {
        color = FOREGROUND[code];
      }
    }
  }

  push(input.slice(cursor));
  return segments;
}

/** Strips escape sequences entirely, for search and copy. */
export function stripAnsi(input: string): string {
  return input.replace(PATTERN, "");
}
