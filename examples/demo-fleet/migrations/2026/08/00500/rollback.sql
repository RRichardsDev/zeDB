-- rollback-class: clean

-- Revert every object upgrade.sql creates or changes.
DROP TABLE IF EXISTS ${db}.test_scratch ON CLUSTER ${cluster} SYNC;
