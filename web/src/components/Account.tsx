// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Dhanvin Raj Velagapudi
import { useCallback, useEffect, useState } from "react";
import { api, type User } from "../api";

type Stage = "idle" | "enrolling" | "codes";

export default function Account({ user }: { user: User }) {
  const [enabled, setEnabled] = useState(false);
  const [remaining, setRemaining] = useState(0);
  const [stage, setStage] = useState<Stage>("idle");
  const [setup, setSetup] = useState<{ qr: string; secret: string } | null>(null);
  const [code, setCode] = useState("");
  const [codes, setCodes] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const status = await api.mfaStatus();
      setEnabled(status.enabled);
      setRemaining(status.recovery_codes_remaining);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function beginSetup() {
    setBusy(true);
    setError(null);
    try {
      setSetup(await api.mfaSetup());
      setStage("enrolling");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function confirmEnrolment() {
    setBusy(true);
    setError(null);
    try {
      const response = await api.mfaEnable(code.trim());
      setCodes(response.recovery_codes);
      setStage("codes");
      setCode("");
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function turnOff() {
    const password = prompt("Enter your password to switch off two-factor authentication");
    if (!password) return;
    setBusy(true);
    try {
      await api.mfaDisable(password);
      setNotice("Two-factor authentication is off.");
      setTimeout(() => setNotice(null), 4000);
      setStage("idle");
      await refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function signOutEverywhere() {
    if (!confirm("Sign out of every device, including this one?")) return;
    await api.logoutEverywhere().catch(() => undefined);
    window.location.reload();
  }

  return (
    <>
      {error && <div className="banner error">{error}</div>}
      {notice && <div className="banner ok">{notice}</div>}

      <div className="card">
        <div className="card-head">
          <span className="card-title">Two-factor authentication</span>
          <span className="card-sub">
            {enabled ? "on" : "off"} · {user.username}
          </span>
        </div>
        <div className="card-body pad">
          {stage === "codes" ? (
            <>
              <div className="banner ok">
                Two-factor authentication is on. Save these recovery codes now — they are shown
                once, and each works a single time.
              </div>
              <div className="code-grid">
                {codes.map((c) => (
                  <span key={c} className="mono">
                    {c}
                  </span>
                ))}
              </div>
              <div className="row" style={{ marginTop: 14 }}>
                <button
                  className="btn"
                  onClick={() => void navigator.clipboard?.writeText(codes.join("\n"))}
                >
                  Copy all
                </button>
                <button className="btn primary" onClick={() => setStage("idle")}>
                  I have saved them
                </button>
              </div>
            </>
          ) : stage === "enrolling" && setup ? (
            <>
              <p className="muted" style={{ marginTop: 0 }}>
                Scan this with Google Authenticator, Authy, 1Password, or any other authenticator
                app, then enter the six-digit code it shows.
              </p>
              <div className="row" style={{ alignItems: "flex-start", gap: 22 }}>
                <img src={setup.qr} alt="Authenticator QR code" className="qr" />
                <div style={{ flex: 1 }}>
                  <div className="field-label">Or type this key in by hand</div>
                  <div className="mono break" style={{ margin: "6px 0 16px", fontSize: 12.5 }}>
                    {setup.secret}
                  </div>
                  <div className="field-label">Code from your app</div>
                  <input
                    type="text"
                    inputMode="numeric"
                    placeholder="000000"
                    style={{ maxWidth: 160, marginTop: 6 }}
                    value={code}
                    onChange={(e) => setCode(e.target.value)}
                  />
                  <div className="row" style={{ marginTop: 14 }}>
                    <button
                      className="btn primary"
                      disabled={busy || code.trim().length < 6}
                      onClick={() => void confirmEnrolment()}
                    >
                      {busy ? "Checking…" : "Turn on"}
                    </button>
                    <button className="btn" onClick={() => setStage("idle")}>
                      Cancel
                    </button>
                  </div>
                </div>
              </div>
            </>
          ) : enabled ? (
            <>
              <p className="muted" style={{ marginTop: 0 }}>
                Your account asks for a code from your authenticator app at sign-in.{" "}
                <strong>{remaining}</strong> recovery {remaining === 1 ? "code" : "codes"} left.
              </p>
              {remaining === 0 && (
                <div className="banner error">
                  You have no recovery codes left. If you lose your authenticator you will need
                  host access to get back in. Switch two-factor off and on again to get a fresh set.
                </div>
              )}
              <button className="btn danger" disabled={busy} onClick={() => void turnOff()}>
                Switch off
              </button>
            </>
          ) : (
            <>
              <p className="muted" style={{ marginTop: 0 }}>
                Add a second step at sign-in, so a stolen password is not enough on its own. You
                will get ten single-use recovery codes for when your phone is not to hand.
              </p>
              <button className="btn primary" disabled={busy} onClick={() => void beginSetup()}>
                {busy ? "Preparing…" : "Set up"}
              </button>
            </>
          )}
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <span className="card-title">Sessions</span>
        </div>
        <div className="card-body pad">
          <p className="muted" style={{ marginTop: 0 }}>
            Signing out normally ends this device only. Sessions expire after a day of inactivity,
            and after two weeks regardless.
          </p>
          <button className="btn" onClick={() => void signOutEverywhere()}>
            Sign out everywhere
          </button>
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <span className="card-title">Lost everything?</span>
        </div>
        <div className="card-body pad">
          <p className="muted" style={{ marginTop: 0 }}>
            If both your password and your recovery codes are gone, there is deliberately no reset
            link — anyone who could use it could take over the machine. Recovery runs from a shell
            on the host instead:
          </p>
          <p className="mono" style={{ fontSize: 13 }}>
            pumpkin-panel recover {user.username}
          </p>
          <p className="dim" style={{ marginBottom: 0, fontSize: 12.5 }}>
            That prints a new password, clears two-factor, ends every session, and records itself in
            the activity log.
          </p>
        </div>
      </div>
    </>
  );
}
