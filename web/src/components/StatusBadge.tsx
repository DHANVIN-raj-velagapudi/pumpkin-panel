// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
import { useEffect, useState } from "react";
import type { Runtime } from "../api";

function uptime(since: number): string {
  const total = Math.max(0, Math.floor(Date.now() / 1000) - since);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

export default function StatusBadge({ runtime }: { runtime: Runtime }) {
  const [, tick] = useState(0);

  useEffect(() => {
    if (runtime.status !== "running") return;
    const timer = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, [runtime.status]);

  const detail =
    runtime.status === "running" && runtime.started_at
      ? `up ${uptime(runtime.started_at)}`
      : runtime.status === "crashed" && runtime.exit_code !== null
        ? `exit ${runtime.exit_code}`
        : null;

  return (
    <span className={`pill ${runtime.status}`}>
      <span className={`dot ${runtime.status}`} />
      {runtime.status}
      {detail && <span style={{ opacity: 0.75, fontWeight: 500 }}>· {detail}</span>}
    </span>
  );
}
