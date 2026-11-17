-- A bounded ascending scan must progress even while an older ongoing game pins
-- lichess_cursor_ms. These checkpoints are cleared after a complete scan.
ALTER TABLE accounts ADD COLUMN lichess_scan_ms INTEGER;
ALTER TABLE accounts ADD COLUMN lichess_pending_ms INTEGER;
ALTER TABLE accounts ADD COLUMN lichess_finished_ms INTEGER;
