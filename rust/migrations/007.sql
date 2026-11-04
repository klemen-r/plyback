-- Fence asynchronous completions independently of SQLite's reusable integer IDs.
ALTER TABLE jobs ADD COLUMN claim_token TEXT;
