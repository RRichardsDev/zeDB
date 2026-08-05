-- rollback-class: clean
DROP VIEW IF EXISTS ${db}.events_daily ON CLUSTER ${cluster};
