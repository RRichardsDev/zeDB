-- rollback-class: clean

-- Revert every object upgrade.sql creates or changes.
ALTER TABLE ${db}.events ON CLUSTER '${cluster}'
    DROP COLUMN IF EXISTS status
    SETTINGS alter_sync = 2;