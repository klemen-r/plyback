
    ALTER TABLE accounts ADD COLUMN history_complete INTEGER NOT NULL DEFAULT 0;
    UPDATE accounts SET history_complete = 1 WHERE last_sync_at IS NOT NULL;
