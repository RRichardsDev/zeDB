multi cursor shift + cmd + arrow key, only higlihts one line
tabs need a right click, "close tab", "close to the right", "close others"
cross-version robustness: we query specific system.* columns from memory; these vary by ClickHouse version (hit total_rows_count not existing in system.merges). Need a version-tolerant approach (probe system.columns / degrade) or a vetted stable-columns list, app-wide.
