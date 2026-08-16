# IRL issues

Raw inbox. Items graduate into a phase doc and gain a reference here;
they leave the list when shipped.

- ai chat drag doesnt  play nice with highlighting and scrolling
  -> PHASE-10.6.md
- does ch cloud provide any oauth login which we could use from inside the app. It still feels super janky to set one up. And feel like its slapped onto the other way of setting up the clusters. This should feel more integrated.
  -> PHASE-10.5.md
- Up to date should only show a tick when its locked
  -> PHASE-10.2.md
- when its unlocked it should show up to date/ upgrade all etc.
  -> PHASE-10.2.md
- after running a regen it should auto close if successful, and rerun chain check without opening ui.
  -> PHASE-10.2.md
- Git account should be per repo on setup to allow for free switching. Allow multi account logins
  -> PHASE-10.7.md (deferred)
- sql history should be connection specific, saved should not be, tabs should not be. open tabs should be connection specific.
  -> PHASE-10.3.md
- when connected to a clickhouse cloud instance, the application should have a 1px border around the editor to indicate its connected which is in the clickhouse yellow. Just to show "you are using clickhouse cloud, this is better"
  -> PHASE-10.5.md
- native port discovery is advertised-port + remap-offset heuristics; connection settings should allow an explicit native (TCP) port per cluster node, heuristic only as fallback
  -> PHASE-10.4.md
- restored sessions re-seed tails before the restored node selection applies (wrong node's rows until a manual flip)
  -> PHASE-10.3.md
- tails freeze when switching connections (loop exits on name mismatch, never resumes)
  -> PHASE-10.3.md
