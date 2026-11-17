-- Lichess export ranges use millisecond creation timestamps. Keep that cursor
-- separate from the whole-second game timestamps shared with other sources.
ALTER TABLE accounts ADD COLUMN lichess_cursor_ms INTEGER;
