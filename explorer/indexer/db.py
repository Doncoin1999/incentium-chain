"""SQLite storage shared by the indexer (writer) and the API (reader). WAL mode, so reads never block."""
from __future__ import annotations

import json
import sqlite3

SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS blocks (
  n INTEGER PRIMARY KEY, hash TEXT NOT NULL, ts INTEGER, author TEXT,
  n_events INTEGER NOT NULL, n_extrinsics INTEGER NOT NULL);

-- Raw chain events, for the block page.
CREATE TABLE IF NOT EXISTS events (
  block INTEGER NOT NULL, idx INTEGER NOT NULL, extrinsic INTEGER,
  module TEXT NOT NULL, name TEXT NOT NULL, data TEXT NOT NULL,
  PRIMARY KEY (block, idx));

-- One row per account touched by an event, in readable form.
CREATE TABLE IF NOT EXISTS account_events (
  block INTEGER NOT NULL, idx INTEGER NOT NULL, account TEXT NOT NULL,
  kind TEXT NOT NULL, title TEXT NOT NULL, counterparty TEXT, amount INTEGER, note TEXT,
  PRIMARY KEY (block, idx, account));
CREATE INDEX IF NOT EXISTS account_events_by_account ON account_events (account, block DESC, idx DESC);
CREATE INDEX IF NOT EXISTS account_events_by_block ON account_events (block DESC, idx DESC);

-- Latest balance snapshot, all amounts in base units (1 ICT = 10^9).
CREATE TABLE IF NOT EXISTS accounts (
  addr TEXT PRIMARY KEY, label TEXT, protocol INTEGER NOT NULL DEFAULT 0, role TEXT,
  free INTEGER NOT NULL DEFAULT 0, reserved INTEGER NOT NULL DEFAULT 0,
  stake INTEGER NOT NULL DEFAULT 0, unstaking INTEGER NOT NULL DEFAULT 0,
  bond INTEGER NOT NULL DEFAULT 0, unbonding INTEGER NOT NULL DEFAULT 0,
  vault_locked INTEGER NOT NULL DEFAULT 0, vault_claimed INTEGER NOT NULL DEFAULT 0,
  total INTEGER NOT NULL DEFAULT 0, tier INTEGER, uid INTEGER, first_block INTEGER);
CREATE INDEX IF NOT EXISTS accounts_by_total ON accounts (protocol, total DESC);

-- Totals per snapshot, kept for 3 days, for the 24 h change column.
CREATE TABLE IF NOT EXISTS account_history (
  block INTEGER NOT NULL, addr TEXT NOT NULL, total INTEGER NOT NULL, PRIMARY KEY (block, addr));
"""


def connect(path: str, readonly: bool = False) -> sqlite3.Connection:
    if readonly:
        con = sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=10, check_same_thread=False)
    else:
        con = sqlite3.connect(path, timeout=30)
        con.execute("PRAGMA journal_mode=WAL")
        con.execute("PRAGMA synchronous=NORMAL")
        con.executescript(SCHEMA)
    con.row_factory = sqlite3.Row
    return con


def get_meta(con: sqlite3.Connection, key: str, default=None):
    row = con.execute("SELECT value FROM meta WHERE key = ?", (key,)).fetchone()
    return json.loads(row[0]) if row else default


def set_meta(con: sqlite3.Connection, key: str, value) -> None:
    con.execute("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, json.dumps(value)))
