-- migration 00200: pilot backfill flag (targeted).
ALTER TABLE ${db}.events ON CLUSTER ${cluster} ADD COLUMN IF NOT EXISTS backfilled UInt8 DEFAULT 0;
