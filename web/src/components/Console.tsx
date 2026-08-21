import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { consoleSocket, type ConsoleLine, type Server } from "../api";
import { parseAnsi } from "../ansi";

/** Keeps the console from growing without bound during a long session. */
const MAX_LINES = 5000;

export default function Console({ server }: { server: Server }) {
  const [lines, setLines] = useState<ConsoleLine[]>([]);
  const [connected, setConnected] = useState(false);
  const [draft, setDraft] = useState("");

  const socketRef = useRef<WebSocket | null>(null);
  const outputRef = useRef<HTMLDivElement | null>(null);
  const pinnedRef = useRef(true);
  const historyRef = useRef<string[]>([]);
  const historyIndexRef = useRef(-1);

  useEffect(() => {
    setLines([]);
    const socket = consoleSocket(server.id);
    socketRef.current = socket;

    socket.onopen = () => setConnected(true);
    socket.onclose = () => setConnected(false);
    socket.onerror = () => setConnected(false);
    socket.onmessage = (event) => {
      try {
        const line = JSON.parse(event.data as string) as ConsoleLine;
        setLines((current) => {
          const next = [...current, line];
          return next.length > MAX_LINES ? next.slice(next.length - MAX_LINES) : next;
        });
      } catch {
        // Ignore anything that is not a console frame.
      }
    };

    return () => {
      socket.onclose = null;
      socket.close();
      socketRef.current = null;
    };
  }, [server.id]);

  // Follow the tail only while the user has not scrolled up to read history.
  useEffect(() => {
    const output = outputRef.current;
    if (output && pinnedRef.current) {
      output.scrollTop = output.scrollHeight;
    }
  }, [lines]);

  function onScroll() {
    const output = outputRef.current;
    if (!output) return;
    const distance = output.scrollHeight - output.scrollTop - output.clientHeight;
    pinnedRef.current = distance < 40;
  }

  function send() {
    const command = draft.trim();
    const socket = socketRef.current;
    if (!command || !socket || socket.readyState !== WebSocket.OPEN) return;

    socket.send(command);
    historyRef.current = [command, ...historyRef.current.filter((c) => c !== command)].slice(0, 50);
    historyIndexRef.current = -1;
    setDraft("");
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Enter") {
      event.preventDefault();
      send();
      return;
    }

    // Up and down walk previously sent commands, like a real terminal.
    if (event.key === "ArrowUp") {
      event.preventDefault();
      const next = Math.min(historyIndexRef.current + 1, historyRef.current.length - 1);
      if (next >= 0) {
        historyIndexRef.current = next;
        setDraft(historyRef.current[next]);
      }
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      const next = historyIndexRef.current - 1;
      historyIndexRef.current = next;
      setDraft(next >= 0 ? historyRef.current[next] : "");
    }
  }

  const canType = server.runtime.status === "running" && connected;

  return (
    <>
      <div className="console-output" ref={outputRef} onScroll={onScroll}>
        {lines.length === 0 && (
          <div className="muted">
            {server.runtime.status === "stopped"
              ? "Server is stopped. Press Start to boot it."
              : "Waiting for output…"}
          </div>
        )}
        {lines.map((line, index) => (
          <div key={index} className={`console-line ${line.stream}`}>
            {parseAnsi(line.line).map((segment, part) => (
              <span key={part} className={segment.className}>
                {segment.text}
              </span>
            ))}
          </div>
        ))}
      </div>

      <div className="console-input">
        <input
          type="text"
          placeholder={
            canType ? "Type a command and press Enter…" : "Console is available while the server runs"
          }
          value={draft}
          disabled={!canType}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKeyDown}
        />
        <button className="btn" disabled={!canType || draft.trim() === ""} onClick={send}>
          Send
        </button>
      </div>
    </>
  );
}
