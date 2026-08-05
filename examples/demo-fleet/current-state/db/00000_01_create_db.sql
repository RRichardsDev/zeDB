-- migration 00000: baseline schema, replicated across the cluster.
CREATE DATABASE IF NOT EXISTS ${db} ON CLUSTER ${cluster};
