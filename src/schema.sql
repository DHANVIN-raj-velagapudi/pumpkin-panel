CREATE TABLE IF NOT EXISTS users (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    email         TEXT,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL DEFAULT 'user',
    is_active     INTEGER NOT NULL DEFAULT 1,
    created_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    token      TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);

CREATE TABLE IF NOT EXISTS servers (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    binary_path  TEXT NOT NULL,
    working_dir  TEXT NOT NULL,
    args         TEXT NOT NULL DEFAULT '',
    stop_command TEXT NOT NULL DEFAULT 'stop',
    autostart    INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS server_permissions (
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    server_id   TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    can_console INTEGER NOT NULL DEFAULT 1,
    can_power   INTEGER NOT NULL DEFAULT 0,
    can_files   INTEGER NOT NULL DEFAULT 0,
    can_config  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, server_id)
);

CREATE TABLE IF NOT EXISTS audit_log (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id   TEXT,
    username  TEXT,
    server_id TEXT,
    action    TEXT NOT NULL,
    detail    TEXT,
    at        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_at ON audit_log(at DESC);

CREATE TABLE IF NOT EXISTS audit_events (
    seq        INTEGER PRIMARY KEY,
    at         INTEGER NOT NULL,
    actor_id   TEXT,
    actor      TEXT NOT NULL,
    action     TEXT NOT NULL,
    category   TEXT NOT NULL,
    server_id  TEXT,
    target     TEXT,
    result     TEXT NOT NULL,
    detail     TEXT,
    meta       TEXT,
    ip         TEXT,
    request_id TEXT NOT NULL,
    prev_hash  TEXT NOT NULL,
    hash       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_events_at ON audit_events(at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_events_category ON audit_events(category);

-- One-time codes for regaining access when the authenticator is lost.
-- Stored as SHA-256 digests: the codes are long random strings, so a fast hash
-- is appropriate here (unlike passwords, they are not guessable by dictionary).
CREATE TABLE IF NOT EXISTS recovery_codes (
    user_id   TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL,
    used_at   INTEGER,
    PRIMARY KEY (user_id, code_hash)
);

CREATE TABLE IF NOT EXISTS backups (
    id         TEXT PRIMARY KEY,
    server_id  TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    size_bytes INTEGER NOT NULL,
    file_count INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    note       TEXT
);
CREATE INDEX IF NOT EXISTS idx_backups_server ON backups(server_id, created_at DESC);

-- Servers the panel launched and expects to still be running. Used to pick
-- them up again after the panel itself restarts.
CREATE TABLE IF NOT EXISTS running_servers (
    server_id   TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
    pid         INTEGER NOT NULL,
    started_at  INTEGER NOT NULL,
    binary_path TEXT NOT NULL
);
