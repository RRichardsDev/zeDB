-- rollback-class: structural
ALTER TABLE ${db}.events DROP COLUMN IF EXISTS backfilled;
