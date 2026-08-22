# IRL issues

Raw inbox. Items graduate into a phase doc and gain a reference here;
they leave the list when shipped.

- the large-table apply confirmation says "rewrites the whole table
  (about 1.0 GB)" for a single-column codec change that only mutates
  that column (~20 MB); the number should size the actual rewrite
  (truthful labels) or users learn to ignore it (seen 2026-08-21 on
  tenant_01.events, at-column codec advice)
- agent highlight_control claims success for controls that are not
  rendered: "rollback" lives in the fleet detail panel, so with no row
  selected (or the fleet view closed) the bridge sets a highlight
  nothing shows, returns "highlighted for a few seconds", and the
  agent tells the user it flashed. Violates the ACP "no invisible UI
  changes" clause; the tool should either make the control visible
  (show fleet, and say so) or answer honestly that it is not on
  screen and what would put it there (seen 2026-08-21, prompt "show
  me where I'd roll back a migration" with no database selected)
  -> fixed (unreleased): toolbar controls bring the fleet view with
  them (narrated); detail-panel controls refuse honestly with the
  navigation hint when no database is selected
- external table engines (PostgreSQL, MySQL, ...) render with blank
  sizes and unguarded probes: NULL total_bytes should read as
  "external" in the inspector, and the sampling probes (cardinality,
  workload measurement) should sit behind a confirm because they
  execute fetch-heavy queries against a remote database someone else
  pays for (probed 2026-08-22 with a PostgreSQL-engine table; browse,
  query, export, and credential masking are all fine)
- zedb-cli `upgrade --dry-run` without `--write` refuses with "this
  connection is read-only; re-run with --write to consent", but a dry
  run is exactly what you want before consenting; it should run on the
  read-only session (it provably writes nothing) (seen 2026-08-21)
- while a storage suggestion is applying, the advice button should
  show a loading state once the mutation runs longer than ~2s; today
  there is no feedback on the button itself (seen 2026-08-21 applying
  the at-column codec on a 1 GB table)
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
