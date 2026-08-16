//! `zedb mcp`: serve read-only tools to an agent over stdio.

use std::path::Path;

use super::runtime;

pub fn serve(
    root: &Path,
    server: Option<String>,
    user: String,
    password: String,
    cache_connection: Option<String>,
) -> Result<(), String> {
    // The repo is optional here: query tools alone are useful.
    let repo = zedb_core::repo::MigrationRepo::open(root).ok();
    let config = server.map(|url| zedb_ch::ChConfig {
        url,
        user,
        password: (!password.is_empty()).then_some(password),
        database: None,
        read_only: true,
        driver: Default::default(),
        native_port: None,
    });
    let mut mcp = zedb_ch::mcp::McpServer::new(repo, config, Default::default());
    if let Some(name) = cache_connection {
        if let Some(root) = dirs::cache_dir() {
            mcp = mcp.with_schema_cache(zedb_ch::schema_cache::connection_snapshot_path(
                &root.join("zedb").join("schema"),
                &name,
            ));
        }
    }
    runtime()?
        .block_on(zedb_ch::mcp::serve_stdio(mcp))
        .map_err(|error| error.to_string())
}
