-- Practice: a per-account Leitner schedule over positions from the account's own
-- reviewed mistakes. Rows are created on the first answer; puzzles that were
-- never answered are derived from `moves` and need no row.
CREATE TABLE practice_items (
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    game_id INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
    ply INTEGER NOT NULL,
    box INTEGER NOT NULL DEFAULT 0,
    due_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    correct INTEGER NOT NULL DEFAULT 0,
    lapses INTEGER NOT NULL DEFAULT 0,
    first_seen_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    last_correct INTEGER,
    PRIMARY KEY (account_id, game_id, ply)
);
CREATE INDEX practice_items_due ON practice_items(account_id, due_at);
