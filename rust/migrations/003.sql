
    CREATE TABLE sessions (
        token_hash TEXT PRIMARY KEY,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        last_seen INTEGER NOT NULL,
        user_agent TEXT
    );
    DROP INDEX IF EXISTS moves_class;
    DROP INDEX IF EXISTS moves_epd;
    CREATE INDEX moves_game_color_class ON moves(game_id, color, classification);
    CREATE INDEX game_analysis_analyzed_at ON game_analysis(analyzed_at);
    CREATE INDEX jobs_status_finished ON jobs(status, finished_at);
    ALTER TABLE moves ADD COLUMN endgame_type TEXT;
    -- Covering index: account insights read only these columns, never the large JSON ones.
    CREATE INDEX moves_insights ON moves(game_id, ply, color, classification, phase, ep_loss, cp_loss, accuracy,
        win_before, win_after, clock, time_spent, is_book, tags, best_san, san, endgame_type, epd_before);
