CREATE TABLE report_games (
    report_id INTEGER NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
    game_id INTEGER NOT NULL,
    game_uuid TEXT NOT NULL,
    PRIMARY KEY (report_id, game_id)
);
ALTER TABLE reports ADD COLUMN scope_frozen INTEGER NOT NULL DEFAULT 0;
