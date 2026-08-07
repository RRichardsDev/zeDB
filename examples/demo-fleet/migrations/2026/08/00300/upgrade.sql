-- migration 00300: describe the change here
ALTER TABLE ${db}.events ON CLUSTER '${cluster}'
ADD COLUMN status String DEFAULT 'pending';