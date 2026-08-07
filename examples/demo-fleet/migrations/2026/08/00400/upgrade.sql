-- migration 00400: describe the change here

ALTER TABLE ${db}.events ON CLUSTER '${cluster}'
    ADD COLUMN IF NOT EXISTS statusV2 String DEFAULT 'pending'
    SETTINGS alter_sync = 2;