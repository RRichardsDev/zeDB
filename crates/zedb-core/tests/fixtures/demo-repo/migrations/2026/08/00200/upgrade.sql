-- migration 00200: targeted backfill flag column for pilot databases.
ALTER TABLE ${db}.events ADD COLUMN IF NOT EXISTS backfilled UInt8 DEFAULT 0;
