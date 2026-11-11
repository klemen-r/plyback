-- Games a sync stored for an account and still has to queue for review. The row is written in
-- the archive's transaction and removed in the transaction that queues the game, so a busy
-- database or a shutdown in between leaves an exact list for the next sync to catch up on.
CREATE TABLE sync_pending (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    game_id INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    priority INTEGER NOT NULL,
    PRIMARY KEY (account_id, game_id)
) WITHOUT ROWID;

-- Requests for a deeper review belong to the active job until it finishes.
ALTER TABLE jobs ADD COLUMN followup_priority INTEGER;
ALTER TABLE jobs ADD COLUMN followup_depth INTEGER;
