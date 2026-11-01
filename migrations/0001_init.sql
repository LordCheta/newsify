-- newsify schema (Cloudflare D1 / SQLite).

CREATE TABLE subscribers (
  id               TEXT PRIMARY KEY,
  email            TEXT NOT NULL UNIQUE,
  status           TEXT NOT NULL CHECK (status IN ('pending', 'active', 'unsubscribed')),
  -- SHA-256 of the confirmation token; the token itself is never stored.
  token_hash       TEXT,
  token_created_at INTEGER,
  created_at       INTEGER NOT NULL,
  confirmed_at     INTEGER,
  unsubscribed_at  INTEGER,
  source           TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_subscribers_token_hash ON subscribers (token_hash);
CREATE INDEX idx_subscribers_status ON subscribers (status);

CREATE TABLE issues (
  id         TEXT PRIMARY KEY,
  subject    TEXT NOT NULL,
  text_body  TEXT NOT NULL,
  html_body  TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

-- One row per (issue, subscriber): the send queue and the delivery log.
CREATE TABLE deliveries (
  issue_id      TEXT NOT NULL REFERENCES issues (id),
  subscriber_id TEXT NOT NULL REFERENCES subscribers (id),
  status        TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sent', 'failed')),
  attempts      INTEGER NOT NULL DEFAULT 0,
  last_error    TEXT,
  updated_at    INTEGER NOT NULL,
  PRIMARY KEY (issue_id, subscriber_id)
);
CREATE INDEX idx_deliveries_pending ON deliveries (status, issue_id);

CREATE TABLE contact_messages (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  email      TEXT NOT NULL,
  company    TEXT NOT NULL DEFAULT '',
  message    TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

-- Rate-limit events. `key` is a keyed hash of the client IP, never the IP.
CREATE TABLE rate_events (
  kind       TEXT NOT NULL,
  key        TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_rate_events ON rate_events (kind, key, created_at);
