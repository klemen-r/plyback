-- The migration runner disables foreign keys before the surrounding transaction
-- and checks them before commit, so rebuilding the parent never cascades deletes.
CREATE TABLE accounts_new (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL COLLATE NOCASE,
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
    import_since TEXT,
    history_complete INTEGER NOT NULL DEFAULT 0,
    sync_generation TEXT NOT NULL DEFAULT '',
    platform TEXT NOT NULL DEFAULT 'chesscom',
    UNIQUE (platform, username COLLATE NOCASE)
);
INSERT INTO accounts_new (
    id,username,display_name,avatar,profile_json,stats_json,profile_fetched_at,
    added_at,last_sync_at,last_new_game_at,sync_status,sync_error,sync_enabled,
    auto_analyze,analyze_history,import_since,history_complete,sync_generation
)
SELECT id,username,display_name,avatar,profile_json,stats_json,profile_fetched_at,
    added_at,last_sync_at,last_new_game_at,sync_status,sync_error,sync_enabled,
    auto_analyze,analyze_history,import_since,history_complete,sync_generation
FROM accounts;
DROP TABLE accounts;
ALTER TABLE accounts_new RENAME TO accounts;
CREATE UNIQUE INDEX accounts_sync_generation ON accounts(sync_generation);
CREATE TRIGGER accounts_sync_generation_insert AFTER INSERT ON accounts
WHEN NEW.sync_generation = ''
BEGIN
    UPDATE accounts SET sync_generation = lower(hex(randomblob(16))) WHERE id = NEW.id;
END;
CREATE TRIGGER accounts_sync_generation_immutable BEFORE UPDATE OF sync_generation ON accounts
WHEN OLD.sync_generation <> '' AND NEW.sync_generation <> OLD.sync_generation
BEGIN
    SELECT RAISE(ABORT, 'Account sync generation is immutable');
END;
CREATE TRIGGER revision_accounts_insert AFTER INSERT ON accounts BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_accounts_update AFTER UPDATE ON accounts BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
CREATE TRIGGER revision_accounts_delete AFTER DELETE ON accounts BEGIN UPDATE data_revision SET revision=revision+1 WHERE id=1; END;
