// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
import { useState, type FormEvent } from "react";
import { api, ApiError, type User } from "../api";

export default function Login({ onLogin }: { onLogin: (user: User) => void }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [needsCode, setNeedsCode] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const response = await api.login(username, password, code || undefined);
      onLogin(response.user);
    } catch (e) {
      if (e instanceof ApiError && e.mfaRequired) {
        // Password was right; ask for the second factor rather than an error.
        setNeedsCode(true);
        setError(null);
      } else {
        setError(e instanceof Error ? e.message : String(e));
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="login-wrap">
      <form className="login-card" onSubmit={submit}>
        <span className="brand-mark" style={{ width: 38, height: 38, fontSize: 18 }}>
          P
        </span>
        <h1>Sign in</h1>
        <p className="muted">Manage your Minecraft servers.</p>

        {error && <div className="banner error">{error}</div>}

        <div className="stack">
          <label htmlFor="username">Username</label>
          <input
            id="username"
            type="text"
            autoComplete="username"
            autoFocus
            value={username}
            onChange={(e) => setUsername(e.target.value)}
          />
        </div>

        <div className="stack">
          <label htmlFor="password">Password</label>
          <input
            id="password"
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </div>

        {needsCode && (
          <div className="stack">
            <label htmlFor="code">Authentication code</label>
            <input
              id="code"
              type="text"
              inputMode="numeric"
              autoComplete="one-time-code"
              placeholder="6 digits, or a recovery code"
              autoFocus
              value={code}
              onChange={(e) => setCode(e.target.value)}
            />
          </div>
        )}

        <button className="btn primary" style={{ width: "100%" }} disabled={busy} type="submit">
          {busy ? "Signing in…" : needsCode ? "Verify" : "Sign in"}
        </button>
      </form>
    </div>
  );
}
