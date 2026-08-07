-- rollback-class: structural

-- Revert every object upgrade.sql creates or changes.
ALTER TABLE ${db}.events ON CLUSTER '${cluster}'
    DROP COLUMN IF EXISTS statusV2
    SETTINGS alter_sync = 2;