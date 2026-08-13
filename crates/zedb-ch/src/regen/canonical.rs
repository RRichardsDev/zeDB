use super::*;

impl Regenerator<'_> {
    pub(super) fn canonical_phase(
        &self,
        chain_texts: &[String],
        files: &mut Files,
        key_to_path: &mut HashMap<Key, String>,
    ) -> Result<(), RegenError> {
        let params = sentinel_params(&self.repo.config);
        let shared = self.shared_databases(chain_texts);

        let name_of = |key: &Key| -> Option<String> {
            match key {
                Key::Object { name, .. } => Some(name.clone()),
                _ => None,
            }
        };
        let normalize = |dump: &BTreeMap<String, String>, side: &str| -> BTreeMap<String, String> {
            dump.iter()
                .map(|(name, create)| {
                    let strip = |text: &str| {
                        restore_sentinels(
                            &text
                                .replace(&format!("{side}__"), "")
                                .replace(side, "${db}"),
                            &params,
                        )
                    };
                    (strip(name), strip(create))
                })
                .collect()
        };

        let base_kinds = [
            "DATABASE",
            "TABLE",
            "DICTIONARY",
            "FUNCTION",
            "USER",
            "ROLE",
        ];
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

        let base_texts: Vec<String> = self
            .provision_order(files)
            .into_iter()
            .filter(|path| {
                let kind = file_kind(&files[path]);
                kind.is_none()
                    || kind
                        .as_deref()
                        .map(|k| base_kinds.contains(&k))
                        .unwrap_or(true)
            })
            .map(|path| files[&path].clone())
            .collect();

        let run_a = self
            .replay
            .dump_objects(
                &[
                    ReplaySide {
                        name: CHAIN_SIDE.into(),
                        texts: chain_texts.to_vec(),
                    },
                    ReplaySide {
                        name: CAND_SIDE.into(),
                        texts: base_texts,
                    },
                ],
                &params,
                &shared,
            )
            .map_err(|error| RegenError::Replay(error.to_string()))?;

        let chain_norm = normalize(&run_a[CHAIN_SIDE], CHAIN_SIDE);
        let chain_raw: BTreeMap<String, String> = run_a[CHAIN_SIDE]
            .iter()
            .map(|(name, create)| {
                (
                    restore_sentinels(
                        &name
                            .replace(&format!("{CHAIN_SIDE}__"), "")
                            .replace(CHAIN_SIDE, "${db}"),
                        &params,
                    ),
                    create.clone(),
                )
            })
            .collect();
        let mut synthesized: HashSet<String> = HashSet::new();

        let reconcile = |files: &mut Files,
                         key_to_path: &mut HashMap<Key, String>,
                         cand_norm: &BTreeMap<String, String>,
                         synthesized: &mut HashSet<String>,
                         second_pass: bool|
         -> Result<bool, RegenError> {
            let mut changed = false;
            let paths: HashMap<String, String> = key_to_path
                .iter()
                .filter_map(|(key, path)| name_of(key).map(|name| (name, path.clone())))
                .collect();
            let mut names: Vec<&String> = chain_norm.keys().chain(cand_norm.keys()).collect();
            names.sort();
            names.dedup();
            for name in names {
                if !cand_norm.contains_key(name) {
                    if second_pass {
                        return Err(RegenError::Track(format!(
                            "the migration chain produces {name:?} but no migration ever CREATEs it; add a CREATE statement for it"
                        )));
                    }
                    continue;
                }
                if !chain_norm.contains_key(name) {
                    if let Some(path) = paths.get(name) {
                        files.remove(path);
                        key_to_path.retain(|_, p| p != path);
                        changed = true;
                    }
                    continue;
                }
                if chain_norm[name] == cand_norm[name] {
                    continue;
                }
                if synthesized.contains(name) {
                    return Err(RegenError::Internal(format!(
                        "synthesis round-trip mismatch for {name}\n  chain: {}\n  regen: {}",
                        chain_norm[name], cand_norm[name]
                    )));
                }
                let path = paths.get(name).ok_or_else(|| {
                    RegenError::Internal(format!(
                        "replayed object {name:?} maps to no current-state file"
                    ))
                })?;
                let new_content = self.synthesize(&chain_raw[name], &files[path], &params)?;
                files.insert(path.clone(), new_content);
                synthesized.insert(name.clone());
                changed = true;
            }
            Ok(changed)
        };

        reconcile(
            files,
            key_to_path,
            &normalize(&run_a[CAND_SIDE], CAND_SIDE),
            &mut synthesized,
            false,
        )?;

        let run_b = self
            .replay
            .dump_objects(
                &[ReplaySide {
                    name: CAND_SIDE.into(),
                    texts: self
                        .provision_order(files)
                        .into_iter()
                        .map(|path| files[&path].clone())
                        .collect(),
                }],
                &params,
                &shared,
            )
            .map_err(|error| RegenError::Replay(error.to_string()))?;
        let before_second = synthesized.clone();
        let changed_second = reconcile(
            files,
            key_to_path,
            &normalize(&run_b[CAND_SIDE], CAND_SIDE),
            &mut synthesized,
            true,
        )?;

        if changed_second || synthesized != before_second {
            let run_c = self
                .replay
                .dump_objects(
                    &[ReplaySide {
                        name: CAND_SIDE.into(),
                        texts: self
                            .provision_order(files)
                            .into_iter()
                            .map(|path| files[&path].clone())
                            .collect(),
                    }],
                    &params,
                    &shared,
                )
                .map_err(|error| RegenError::Replay(error.to_string()))?;
            let cand_c = normalize(&run_c[CAND_SIDE], CAND_SIDE);
            let mut names: Vec<&String> = chain_norm.keys().chain(cand_c.keys()).collect();
            names.sort();
            names.dedup();
            for name in names {
                if chain_norm.get(name) != cand_c.get(name) {
                    return Err(RegenError::Internal(format!(
                        "synthesis round-trip mismatch for {name}\n  chain: {:?}\n  regen: {:?}",
                        chain_norm.get(name),
                        cand_c.get(name)
                    )));
                }
            }
        }
        Ok(())
    }
}
