-- migration 00100: daily rollup view.
CREATE VIEW ${db}.events_daily AS
SELECT
    kind,
    toDate(occurred_at) AS day,
    count() AS events
FROM ${db}.events
GROUP BY kind, day;
