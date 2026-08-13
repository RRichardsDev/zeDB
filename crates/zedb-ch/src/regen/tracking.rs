use super::*;

impl<'a> Regenerator<'a> {
    pub fn new(repo: &'a MigrationRepo, binary: PathBuf) -> Self {
        let db_scope = repo
            .config
            .scopes
            .iter()
            .find(|(_, scope)| scope.param.as_deref() == Some("db"))
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "db".into());
        let global_scope = repo
            .config
            .scopes
            .iter()
            .find(|(_, scope)| scope.param.is_none())
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "global".into());
        Self {
            repo,
            replay: LocalReplay::new(binary),
            db_scope,
            global_scope,
        }
    }

    fn key_scope(&self, key: &Key) -> &str {
        let names = match key {
            Key::Object { name, .. } => vec![name.as_str()],
            Key::Grant { target, grantee } => vec![target.as_str(), grantee.as_str()],
            Key::GrantRole { role, user } => vec![role.as_str(), user.as_str()],
        };
        if names.iter().any(|name| name.contains("${db}")) {
            &self.db_scope
        } else {
            &self.global_scope
        }
    }

    pub(super) fn provision_order(&self, files: &Files) -> Vec<String> {
        let mut global: Vec<String> = files
            .keys()
            .filter(|path| path.starts_with(&format!("{}/", self.global_scope)))
            .cloned()
            .collect();
        let mut db: Vec<String> = files
            .keys()
            .filter(|path| path.starts_with(&format!("{}/", self.db_scope)))
            .cloned()
            .collect();
        global.sort();
        db.sort();
        global.extend(db);
        global
    }

    /// Databases shared across the fleet: declared in config (created by
    /// cluster bootstrap) plus literal ones the chain creates itself.
    /// Ephemeral replays pre-create and isolate them per side.
    pub(super) fn shared_databases(&self, chain_texts: &[String]) -> Vec<String> {
        let mut shared = self.repo.config.replay.shared_databases.clone();
        for text in chain_texts {
            for chunk in split_statements(text) {
                let (_, body) = partition_chunk(&chunk);
                if let Some(captures) = CREATE.captures(body.trim()) {
                    if captures["kind"].eq_ignore_ascii_case("database") {
                        let name = norm_name(&captures["name"]);
                        if !name.contains("${") && !shared.contains(&name) {
                            shared.push(name);
                        }
                    }
                }
            }
        }
        shared
    }

    /// Replay the migration chain; returns `{relpath: content}` for
    /// current-state/.
    pub fn regenerate(&self) -> Result<Files, RegenError> {
        let chain: Vec<_> = self
            .repo
            .migrations
            .iter()
            .filter(|migration| migration.targeted.is_none())
            .collect();
        let Some(first) = chain.first() else {
            return Ok(Files::new());
        };
        if first.number != 0 {
            return Err(RegenError::Track(
                "baseline migration 00000 not found".into(),
            ));
        }

        let mut files: Files = Files::new();
        let mut key_to_path: HashMap<Key, String> = HashMap::new();

        // Baseline: every statement creates an object.
        let baseline_text = first
            .upgrade_sql()
            .map_err(|error| RegenError::Repo(error.to_string()))?;
        let mut counters: HashMap<String, u32> = HashMap::new();
        for chunk in split_statements(&baseline_text) {
            let (comments, body) = partition_chunk(&chunk);
            if body.is_empty() {
                continue;
            }
            let key = object_key(&body).ok_or_else(|| {
                RegenError::Track(format!(
                    "baseline: cannot identify the object created by {:?}; the baseline may only contain CREATE and GRANT statements",
                    body.lines().next().unwrap_or_default()
                ))
            })?;
            if key_to_path.contains_key(&key) {
                return Err(RegenError::Track(format!(
                    "baseline: {key:?} is defined twice"
                )));
            }
            let path = self.place(&key, &body, 0, &mut counters)?;
            files.insert(path.clone(), format!("{comments}{body};\n"));
            key_to_path.insert(key, path);
        }

        let mut dropped: HashSet<String> = HashSet::new();
        let mut needs_replay = false;
        for migration in chain.iter().skip(1) {
            let text = migration
                .upgrade_sql()
                .map_err(|error| RegenError::Repo(error.to_string()))?;
            let mut counters: HashMap<String, u32> = HashMap::new();
            for chunk in split_statements(&text) {
                let (comments, body) = partition_chunk(&chunk);
                if body.is_empty() {
                    continue;
                }
                let trimmed = body.trim().to_string();
                if ACCESS_UNSUPPORTED.is_match(&trimmed) {
                    return Err(RegenError::Track(format!(
                        "migration {:05}: regen cannot track {:?}; express access-control changes as CREATE [OR REPLACE] USER/ROLE, GRANT, REVOKE, or DROP USER/ROLE",
                        migration.number,
                        trimmed.lines().next().unwrap_or_default()
                    )));
                }
                if let Some(captures) = DROP_ACCESS.captures(&trimmed) {
                    self.apply_drop_access(
                        &captures,
                        migration.number,
                        &key_to_path,
                        &mut dropped,
                        &trimmed,
                    )?;
                    continue;
                }
                if let Some(captures) = DROP.captures(&trimmed) {
                    let target = norm_name(&captures["name"]);
                    let key = key_to_path
                        .keys()
                        .find(|key| matches!(key, Key::Object { name, .. } if *name == target));
                    match key {
                        Some(key) => {
                            dropped.insert(key_to_path[key].clone());
                        }
                        None if IF_EXISTS.is_match(&trimmed) => {}
                        None => {
                            return Err(RegenError::Track(format!(
                                "migration {:05}: DROP of unknown object {target:?}",
                                migration.number
                            )));
                        }
                    }
                    continue;
                }
                if trimmed.to_uppercase().starts_with("RENAME") {
                    let captures = RENAME.captures(&trimmed).ok_or_else(|| {
                        RegenError::Track(format!(
                            "migration {:05}: only a single 'RENAME TABLE a TO b' is supported",
                            migration.number
                        ))
                    })?;
                    self.apply_rename(
                        &captures,
                        migration.number,
                        &mut files,
                        &mut key_to_path,
                        &mut dropped,
                    )?;
                    needs_replay = true;
                    continue;
                }
                if trimmed.to_uppercase().starts_with("REVOKE") {
                    self.apply_revoke(
                        &trimmed,
                        migration.number,
                        &files,
                        &key_to_path,
                        &mut dropped,
                    )?;
                    continue;
                }
                match object_key(&body) {
                    Some(key) => {
                        let content = format!("{comments}{body};\n");
                        if let Some(path) = key_to_path.get(&key) {
                            files.insert(path.clone(), content);
                            dropped.remove(path);
                        } else {
                            let path = self.place(&key, &body, migration.number, &mut counters)?;
                            if files.contains_key(&path) {
                                return Err(RegenError::Track(format!(
                                    "migration {:05}: {path:?} collides with an existing file",
                                    migration.number
                                )));
                            }
                            files.insert(path.clone(), content);
                            key_to_path.insert(key, path);
                        }
                    }
                    None => {
                        // ALTER, EXCHANGE, INSERT, OPTIMIZE, guards, ...:
                        // the canonical replay picks up any consequence.
                        needs_replay = true;
                    }
                }
            }
        }

        files.retain(|path, _| !dropped.contains(path));
        key_to_path.retain(|_, path| files.contains_key(path));
        self.assign_db_scope_paths(&mut files, &mut key_to_path)?;

        if needs_replay {
            let chain_texts: Vec<String> = chain
                .iter()
                .map(|migration| migration.upgrade_sql())
                .collect::<Result<_, _>>()
                .map_err(|error| RegenError::Repo(error.to_string()))?;
            self.canonical_phase(&chain_texts, &mut files, &mut key_to_path)?;
            self.assign_db_scope_paths(&mut files, &mut key_to_path)?;
        }

        Ok(files)
    }

    fn place(
        &self,
        key: &Key,
        body: &str,
        migration: u32,
        counters: &mut HashMap<String, u32>,
    ) -> Result<String, RegenError> {
        let scope = self.key_scope(key).to_string();
        let counter = counters.entry(scope.clone()).or_insert(0);
        *counter += 1;
        if *counter > 99 {
            return Err(RegenError::Track(format!(
                "migration {migration:05} introduces more than 99 {scope} objects"
            )));
        }
        Ok(format!(
            "{scope}/{migration:05}_{:02}_{}.sql",
            counter,
            stem_for(key, body)
        ))
    }

    fn apply_drop_access(
        &self,
        captures: &regex::Captures,
        migration: u32,
        key_to_path: &HashMap<Key, String>,
        dropped: &mut HashSet<String>,
        body: &str,
    ) -> Result<(), RegenError> {
        let kind = captures["kind"].to_uppercase();
        let name = norm_name(&captures["name"]);
        let stripped = name.trim_matches(|c| c == '\'' || c == '"').to_string();
        let key = key_to_path.keys().find(|key| {
            matches!(key, Key::Object { kind: k, name: n }
                if *k == kind && norm_name(n).trim_matches(|c| c == '\'' || c == '"') == stripped)
        });
        let Some(key) = key else {
            if IF_EXISTS.is_match(body) {
                return Ok(());
            }
            return Err(RegenError::Track(format!(
                "migration {migration:05}: DROP {kind} of unknown {name:?}"
            )));
        };
        dropped.insert(key_to_path[key].clone());
        // A dropped user/role takes its grants with it, as on the server.
        for (other, path) in key_to_path {
            let grantee_like: Vec<&String> = match other {
                Key::Grant { grantee, .. } => vec![grantee],
                Key::GrantRole { role, user } => vec![role, user],
                Key::Object { .. } => vec![],
            };
            if grantee_like
                .iter()
                .any(|g| norm_name(g).trim_matches(|c| c == '\'' || c == '"') == stripped)
            {
                dropped.insert(path.clone());
            }
        }
        Ok(())
    }

    fn apply_rename(
        &self,
        captures: &regex::Captures,
        migration: u32,
        files: &mut Files,
        key_to_path: &mut HashMap<Key, String>,
        dropped: &mut HashSet<String>,
    ) -> Result<(), RegenError> {
        let old_raw = captures["old"].to_string();
        let new_raw = captures["new"].to_string();
        let old = norm_name(&old_raw);
        let new = norm_name(&new_raw);
        let key = key_to_path
            .keys()
            .find(|key| matches!(key, Key::Object { name, .. } if *name == old))
            .cloned()
            .ok_or_else(|| {
                RegenError::Track(format!(
                    "migration {migration:05}: RENAME of unknown object {old:?}"
                ))
            })?;
        let old_path = key_to_path.remove(&key).expect("key was found above");
        let mut content = files.remove(&old_path).ok_or_else(|| {
            RegenError::Internal(format!("rename source file {old_path:?} is missing"))
        })?;
        for variant in [old_raw.as_str(), old.as_str()] {
            content = content.replace(variant, &new_raw);
        }
        let Key::Object { kind, .. } = key else {
            unreachable!("rename keys are objects");
        };
        let new_key = Key::Object { kind, name: new };
        let new_path = match STATE_PATH.captures(&old_path) {
            Some(path_captures) if path_captures["scope"] == *self.db_scope => format!(
                "{}/{}_{}_{}.sql",
                &path_captures["scope"],
                &path_captures["mig"],
                &path_captures["seq"],
                stem_for(&new_key, "")
            ),
            _ => old_path.clone(),
        };
        files.insert(new_path.clone(), content);
        key_to_path.insert(new_key, new_path);
        dropped.remove(&old_path);
        Ok(())
    }

    fn apply_revoke(
        &self,
        body: &str,
        migration: u32,
        files: &Files,
        key_to_path: &HashMap<Key, String>,
        dropped: &mut HashSet<String>,
    ) -> Result<(), RegenError> {
        if let Some(captures) = REVOKE.captures(body) {
            let key = Key::Grant {
                target: captures["target"].to_string(),
                grantee: captures["grantee"].to_string(),
            };
            let path = key_to_path.get(&key).ok_or_else(|| {
                RegenError::Track(format!(
                "migration {migration:05}: REVOKE does not match any tracked GRANT (on {} from {})",
                &captures["target"], &captures["grantee"]
            ))
            })?;
            let revoked = captures["what"]
                .to_uppercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let (_, granted_body) =
                partition_chunk(files[path].trim_end_matches('\n').trim_end_matches(';'));
            let granted = GRANT
                .captures(granted_body.trim())
                .map(|c| {
                    c["what"]
                        .to_uppercase()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            if revoked != "ALL" && revoked != "ALL PRIVILEGES" && revoked != granted {
                return Err(RegenError::Track(format!(
                    "migration {migration:05}: partial REVOKE ({revoked:?} of the tracked {granted:?}); REVOKE the full grant and re-GRANT what remains"
                )));
            }
            dropped.insert(path.clone());
            return Ok(());
        }
        if let Some(captures) = REVOKE_ROLE.captures(body) {
            let key = Key::GrantRole {
                role: captures["role"].to_string(),
                user: captures["user"].to_string(),
            };
            let path = key_to_path.get(&key).ok_or_else(|| {
                RegenError::Track(format!(
                    "migration {migration:05}: REVOKE does not match any tracked role grant ({} from {})",
                    &captures["role"], &captures["user"]
                ))
            })?;
            dropped.insert(path.clone());
            return Ok(());
        }
        Err(RegenError::Track(format!(
            "migration {migration:05}: cannot parse REVOKE statement {:?}",
            body.lines().next().unwrap_or_default()
        )))
    }

    /// Renumber db-scope files so every object sorts after what it
    /// references: a deterministic Kahn walk over whole-identifier
    /// references; in the common case nothing moves.
    fn assign_db_scope_paths(
        &self,
        files: &mut Files,
        key_to_path: &mut HashMap<Key, String>,
    ) -> Result<(), RegenError> {
        let prefix = format!("{}/", self.db_scope);
        let scope_paths: Vec<String> = files
            .keys()
            .filter(|path| path.starts_with(&prefix))
            .cloned()
            .collect();
        let mut base_key: HashMap<String, (u32, u32)> = HashMap::new();
        let mut stems: HashMap<String, String> = HashMap::new();
        for path in &scope_paths {
            let captures = STATE_PATH.captures(path).ok_or_else(|| {
                RegenError::Internal(format!(
                    "current-state path {path:?} does not follow scope/NNNNN_SS_name.sql"
                ))
            })?;
            base_key.insert(
                path.clone(),
                (
                    captures["mig"].parse().expect("five digits"),
                    captures["seq"].parse().expect("two digits"),
                ),
            );
            stems.insert(path.clone(), captures["stem"].to_string());
        }

        let referenceable: HashMap<String, String> = key_to_path
            .iter()
            .filter_map(|(key, path)| match key {
                Key::Object { kind, name }
                    if matches!(kind.as_str(), "TABLE" | "VIEW" | "DICTIONARY" | "FUNCTION")
                        && base_key.contains_key(path) =>
                {
                    Some((path.clone(), name.clone()))
                }
                _ => None,
            })
            .collect();

        let file_kind = |content: &str| -> Option<String> {
            let (_, body) = partition_chunk(content);
            CREATE.captures(body.trim()).map(|captures| {
                if captures.name("mat").is_some() {
                    "MATERIALIZED VIEW".to_string()
                } else {
                    captures["kind"].to_uppercase()
                }
            })
        };
        let references = |content: &str, name: &str| -> bool {
            let (_, body) = partition_chunk(content);
            let body = body.replace('`', "");
            let mut start = 0;
            while let Some(found) = body[start..].find(name) {
                let end = start + found + name.len();
                let boundary = body[end..]
                    .chars()
                    .next()
                    .map(|c| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(true);
                if boundary {
                    return true;
                }
                start = start + found + 1;
            }
            false
        };

        let mut deps: HashMap<String, HashSet<String>> = HashMap::new();
        for path in &scope_paths {
            let kind = file_kind(&files[path]);
            let edges = match kind.as_deref() {
                None | Some("USER") | Some("ROLE") | Some("DATABASE") => HashSet::new(),
                _ => referenceable
                    .iter()
                    .filter(|(other, name)| *other != path && references(&files[path], name))
                    .map(|(other, _)| other.clone())
                    .collect(),
            };
            deps.insert(path.clone(), edges);
        }

        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        for (path, edges) in &deps {
            for dep in edges {
                dependents
                    .entry(dep.clone())
                    .or_default()
                    .push(path.clone());
            }
        }
        let mut remaining: HashMap<String, HashSet<String>> = deps.clone();
        let mut ready: Vec<String> = scope_paths
            .iter()
            .filter(|path| remaining[*path].is_empty())
            .cloned()
            .collect();
        ready.sort_by_key(|path| base_key[path]);
        let mut assigned: HashMap<String, (u32, u32)> = HashMap::new();
        let mut used: HashSet<(u32, u32)> = HashSet::new();
        while let Some(path) = ready.first().cloned() {
            ready.remove(0);
            let mut candidate = base_key[&path];
            for dep in &deps[&path] {
                let dep_assigned = assigned[dep];
                if dep_assigned >= candidate {
                    candidate = (dep_assigned.0, dep_assigned.1 + 1);
                }
            }
            while used.contains(&candidate) {
                candidate = (candidate.0, candidate.1 + 1);
            }
            if candidate.1 > 99 {
                return Err(RegenError::Track(format!(
                    "ran out of statement-index slots placing {path:?}"
                )));
            }
            assigned.insert(path.clone(), candidate);
            used.insert(candidate);
            if let Some(waiters) = dependents.get(&path) {
                let mut newly_ready = Vec::new();
                for waiter in waiters {
                    let edges = remaining.get_mut(waiter).expect("waiter has deps");
                    edges.remove(&path);
                    if edges.is_empty() {
                        newly_ready.push(waiter.clone());
                    }
                }
                ready.extend(newly_ready);
                ready.sort_by_key(|p| base_key[p]);
                ready.dedup();
            }
        }
        if assigned.len() != scope_paths.len() {
            let mut cycle: Vec<&String> = scope_paths
                .iter()
                .filter(|path| !assigned.contains_key(*path))
                .collect();
            cycle.sort();
            return Err(RegenError::Track(format!(
                "circular references among current-state objects: {cycle:?}"
            )));
        }

        let rename: HashMap<String, String> = scope_paths
            .iter()
            .map(|path| {
                let (mig, seq) = assigned[path];
                (
                    path.clone(),
                    format!("{}/{mig:05}_{seq:02}_{}.sql", self.db_scope, stems[path]),
                )
            })
            .collect();
        let old_files = std::mem::take(files);
        for (path, content) in old_files {
            let new_path = rename.get(&path).cloned().unwrap_or(path);
            files.insert(new_path, content);
        }
        for path in key_to_path.values_mut() {
            if let Some(new_path) = rename.get(path) {
                *path = new_path.clone();
            }
        }
        Ok(())
    }
}
