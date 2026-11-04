-- Account IDs and usernames may both be reused after deletion. Fence each row
-- incarnation so delayed network responses cannot target its replacement.
ALTER TABLE accounts ADD COLUMN sync_generation TEXT NOT NULL DEFAULT '';
UPDATE accounts SET sync_generation = lower(hex(randomblob(16)));
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
