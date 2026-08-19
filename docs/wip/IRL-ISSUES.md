# IRL issues

Raw inbox. Items graduate into a phase doc and gain a reference here;
they leave the list when shipped.

- ai chat drag doesnt  play nice with highlighting and scrolling
  -> shipped 2026-08-16 (phase doc retired)
- does ch cloud provide any oauth login which we could use from inside the app. It still feels super janky to set one up. And feel like its slapped onto the other way of setting up the clusters. This should feel more integrated.
  -> shipped in v0.1.31 (phase doc retired)
- Git account should be per repo on setup to allow for free switching. Allow multi account logins
  -> PHASE-10.7.md (deferred)
- when connected to a clickhouse cloud instance, the application should have a 1px border around the editor to indicate its connected which is in the clickhouse yellow. Just to show "you are using clickhouse cloud, this is better"
  -> shipped in v0.1.31 (phase doc retired)
- Akward to get out of a table deatils view. There should be some type of x button
  -> fixed (unreleased): close button in the panel header
- comments in queries breaking runs: trailing comment after the last
  semicolon ran as its own statement, cursor on an end-of-line comment
  ran the next statement, ${} in comments tripped query variables, and
  ClickHouse itself rejects comments between INSERT VALUES rows
  -> fixed (unreleased): comment-aware run pipeline + comments stripped
  from the VALUES data section on the wire
- @set redeclared with the same name used the last value everywhere
  -> fixed (unreleased): each use binds to the nearest @set above it
- native SET param_x + {x:Type} failed with UNKNOWN_QUERY_PARAMETER
  (stateless HTTP: the SET evaporated before the next statement)
  -> fixed (unreleased) for query runs; EXPLAIN/estimate/advisor,
  the column-filter probe, and export do not attach params yet, so
  those still fail on parameterized statements
