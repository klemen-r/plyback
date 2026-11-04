
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);

    CREATE TABLE accounts (
        id INTEGER PRIMARY KEY,
        username TEXT NOT NULL UNIQUE COLLATE NOCASE,
        display_name TEXT,
        avatar TEXT,
        profile_json TEXT,
        stats_json TEXT,
        profile_fetched_at INTEGER,
        added_at INTEGER NOT NULL,
        last_sync_at INTEGER,
        last_new_game_at INTEGER,
        sync_status TEXT NOT NULL DEFAULT 'idle',
        sync_error TEXT,
        sync_enabled INTEGER NOT NULL DEFAULT 1,
        auto_analyze INTEGER NOT NULL DEFAULT 1,
        analyze_history INTEGER NOT NULL DEFAULT 1,
        import_since TEXT
    );

    CREATE TABLE archives (
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        period TEXT NOT NULL,
        url TEXT NOT NULL,
        etag TEXT,
        last_modified TEXT,
        fetched_at INTEGER,
        game_count INTEGER NOT NULL DEFAULT 0,
        complete INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (account_id, period)
    );

    CREATE TABLE games (
        id INTEGER PRIMARY KEY,
        uuid TEXT NOT NULL UNIQUE,
        url TEXT,
        source TEXT NOT NULL DEFAULT 'chesscom',
        pgn TEXT NOT NULL,
        rules TEXT NOT NULL DEFAULT 'chess',
        time_control TEXT,
        time_class TEXT,
        rated INTEGER,
        start_time INTEGER,
        end_time INTEGER,
        white_username TEXT,
        white_rating INTEGER,
        white_result TEXT,
        black_username TEXT,
        black_rating INTEGER,
        black_result TEXT,
        result TEXT,
        termination TEXT,
        eco TEXT,
        eco_url TEXT,
        initial_fen TEXT,
        plies INTEGER,
        first_moves TEXT,
        opening_eco TEXT,
        opening_name TEXT,
        opening_family TEXT,
        opening_variation TEXT,
        chesscom_accuracy_white REAL,
        chesscom_accuracy_black REAL,
        imported_at INTEGER NOT NULL
    );
    CREATE INDEX games_end_time ON games(end_time);
    CREATE INDEX games_family ON games(opening_family);

    CREATE TABLE account_games (
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        game_id INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
        color TEXT NOT NULL,
        PRIMARY KEY (account_id, game_id)
    );
    CREATE INDEX account_games_game ON account_games(game_id);

    CREATE TABLE game_analysis (
        game_id INTEGER PRIMARY KEY REFERENCES games(id) ON DELETE CASCADE,
        depth INTEGER NOT NULL,
        multipv INTEGER NOT NULL,
        engine TEXT,
        version INTEGER NOT NULL,
        analyzed_at INTEGER NOT NULL,
        duration_ms INTEGER,
        accuracy_white REAL,
        accuracy_black REAL,
        acpl_white REAL,
        acpl_black REAL,
        opening_eco TEXT,
        opening_name TEXT,
        opening_family TEXT,
        opening_variation TEXT,
        book_plies INTEGER,
        counts_json TEXT,
        key_moments_json TEXT,
        evals_json TEXT
    );
    CREATE INDEX game_analysis_family ON game_analysis(opening_family);

    CREATE TABLE moves (
        game_id INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
        ply INTEGER NOT NULL,
        color TEXT NOT NULL,
        san TEXT NOT NULL,
        uci TEXT NOT NULL,
        fen_before TEXT NOT NULL,
        epd_before TEXT NOT NULL,
        eval_before INTEGER,
        eval_after INTEGER,
        win_before REAL,
        win_after REAL,
        ep_loss REAL,
        cp_loss INTEGER,
        accuracy REAL,
        best_uci TEXT,
        best_san TEXT,
        classification TEXT,
        phase TEXT,
        clock REAL,
        time_spent REAL,
        is_book INTEGER,
        tags TEXT,
        explanation TEXT,
        lines_json TEXT,
        reply_json TEXT,
        PRIMARY KEY (game_id, ply)
    );
    CREATE INDEX moves_epd ON moves(epd_before);
    CREATE INDEX moves_class ON moves(classification);

    CREATE TABLE position_cache (
        key TEXT PRIMARY KEY,
        depth INTEGER NOT NULL,
        multipv INTEGER NOT NULL,
        data TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );

    CREATE TABLE jobs (
        id INTEGER PRIMARY KEY,
        game_id INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
        status TEXT NOT NULL DEFAULT 'queued',
        priority INTEGER NOT NULL DEFAULT 10,
        depth INTEGER,
        attempts INTEGER NOT NULL DEFAULT 0,
        error TEXT,
        progress INTEGER NOT NULL DEFAULT 0,
        total INTEGER NOT NULL DEFAULT 0,
        partial_json TEXT,
        created_at INTEGER NOT NULL,
        started_at INTEGER,
        finished_at INTEGER,
        sort_key INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX jobs_pick ON jobs(status, priority DESC, sort_key DESC);
    CREATE UNIQUE INDEX jobs_active_game ON jobs(game_id) WHERE status IN ('queued', 'running');

    CREATE TABLE reports (
        id INTEGER PRIMARY KEY,
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        title TEXT,
        filters_json TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'analyzing',
        total_games INTEGER NOT NULL DEFAULT 0,
        analyzed_games INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        completed_at INTEGER,
        error TEXT,
        report_json TEXT
    );

    CREATE TABLE events (
        id INTEGER PRIMARY KEY,
        ts INTEGER NOT NULL,
        level TEXT NOT NULL,
        source TEXT NOT NULL,
        message TEXT NOT NULL,
        account_id INTEGER
    );
    CREATE INDEX events_ts ON events(ts);
