-- rollback-class: structural
ALTER TABLE ${db}.events ON CLUSTER ${cluster} DROP COLUMN IF EXISTS backfilled;
