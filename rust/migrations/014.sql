-- Recheck ongoing games by ID instead of pinning every creation-time export.
-- The old cursor/pending marker is an inclusive discovery boundary: restart
-- that scan once so pre-upgrade ongoing IDs can be rebuilt without omissions.
CREATE TABLE lichess_pending_games (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    game_id TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    checked_at INTEGER NOT NULL DEFAULT 0,
    misses INTEGER NOT NULL DEFAULT 0 CHECK (misses >= 0),
    PRIMARY KEY (account_id, game_id)
);
CREATE INDEX lichess_pending_poll ON lichess_pending_games(account_id, checked_at, game_id);

UPDATE accounts
SET lichess_cursor_ms = CASE
        WHEN lichess_pending_ms IS NULL THEN lichess_cursor_ms
        ELSE MIN(COALESCE(lichess_cursor_ms, lichess_pending_ms), lichess_pending_ms)
    END,
    lichess_scan_ms = NULL,
    lichess_pending_ms = NULL,
    lichess_finished_ms = NULL
WHERE platform = 'lichess';

-- Pending-ID polling is internal bookkeeping, not a change to reviewed data.
-- Deliberately do not attach data_revision triggers to this table.
