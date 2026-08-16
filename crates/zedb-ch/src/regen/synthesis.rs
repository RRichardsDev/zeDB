use super::*;

impl Regenerator<'_> {
    /// Rebuild a current-state file from the chain's canonical dump. The
    /// dump is authoritative for the definition; the old file donates what
    /// the declustered replay cannot know: attached comments, Replicated
    /// engine arguments, definer clauses, and the ON CLUSTER form.
    pub(super) fn synthesize(
        &self,
        raw_create: &str,
        old_content: &str,
        params: &BTreeMap<String, String>,
    ) -> Result<String, RegenError> {
        let mut render_params = params.clone();
        render_params.insert("db".into(), CHAIN_SIDE.into());
        let (comments, old_body) = partition_chunk(old_content.trim_end_matches('\n'));
        let mut sql = raw_create.to_string();

        // clickhouse-local runs everything as `default`; restore the
        // declared definer, or drop the injected clause entirely.
        static INJECTED: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\s*DEFINER = \w+ SQL SECURITY \w+").expect("static regex")
        });
        static DECLARED_DEFINER: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"DEFINER\s*=\s*(\S+)").expect("static regex"));
        static DECLARED_SECURITY: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?i)SQL\s+SECURITY\s+(\w+)").expect("static regex"));
        if let Some(injected) = INJECTED.find(&sql.clone()) {
            let definer = DECLARED_DEFINER.captures(&old_body);
            let security = DECLARED_SECURITY.captures(&old_body);
            if definer.is_some() || security.is_some() {
                let who_template = definer
                    .map(|c| c[1].to_string())
                    .unwrap_or_else(|| "CURRENT_USER".into());
                let who = render(&who_template, &render_params).unwrap_or(who_template);
                let mode = security
                    .map(|c| c[1].to_string())
                    .unwrap_or_else(|| "DEFINER".into());
                sql.replace_range(
                    injected.range(),
                    &format!(" DEFINER = {who} SQL SECURITY {mode}"),
                );
            } else {
                sql.replace_range(injected.range(), "");
            }
        }

        // Transplant the Replicated (or Cloud's Shared) engine
        // prefix: zk path and replica are declarations the
        // declustered replay stripped. An explicit plain engine is
        // deliberate and left alone.
        let engine = REPLICATED.captures(&old_body).map(|captures| {
            (
                captures[1].to_string(),
                captures[2].to_string(),
                captures[3].to_string(),
                captures[4].to_string(),
            )
        });
        if let Some((kind, family, zk_template, replica)) = engine {
            let zk = render(&zk_template, &render_params).unwrap_or(zk_template);
            let with_args =
                Regex::new(&format!(r"ENGINE = {family}\(([^)]*)\)")).expect("family is a word");
            let mut replaced = false;
            sql = with_args
                .replacen(&sql, 1, |captures: &regex::Captures| {
                    replaced = true;
                    let args = captures[1].trim().to_string();
                    let joined = if args.is_empty() {
                        String::new()
                    } else {
                        format!(", {args}")
                    };
                    format!("ENGINE = {kind}{family}({zk}, {replica}{joined})")
                })
                .into_owned();
            if !replaced {
                let bare_engine =
                    Regex::new(&format!(r"ENGINE = {family}\b")).expect("family is a word");
                let mut count = 0;
                sql = bare_engine
                    .replacen(&sql, 1, |_: &regex::Captures| {
                        count += 1;
                        format!("ENGINE = {kind}{family}({zk}, {replica})")
                    })
                    .into_owned();
                if count == 0 {
                    return Err(RegenError::Internal(format!(
                        "could not transplant {kind}{family} engine into:\n{sql}"
                    )));
                }
            }
        }

        static HEAD: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^CREATE (MATERIALIZED VIEW|VIEW|TABLE|DICTIONARY) (\S+)")
                .expect("static regex")
        });
        let head = HEAD.captures(&sql).ok_or_else(|| {
            RegenError::Internal(format!("unexpected canonical dump shape:\n{sql}"))
        })?;
        let kind = head[1].to_string();
        let name = head[2].to_string();
        let guard = if kind == "VIEW" { "OR REPLACE " } else { "" };
        let exists = if kind == "VIEW" { "" } else { "IF NOT EXISTS " };
        // ON CLUSTER travels only where the file declared it; repos
        // without clusters must not gain the clause.
        let cluster = if ON_CLUSTER_DECL.is_match(&old_body) {
            format!(" ON CLUSTER {}", params["cluster"])
        } else {
            String::new()
        };
        let head_len = head.get(0).expect("whole match").end();
        sql = format!(
            "CREATE {guard}{kind} {exists}{name}{cluster}{}",
            &sql[head_len..]
        );

        let formatted = self
            .replay
            .format_sql(&sql)
            .map_err(|error| RegenError::Replay(error.to_string()))?;
        let mut restored = formatted.replace(&format!("{CHAIN_SIDE}__"), "");
        restored = restored.replace(CHAIN_SIDE, "${db}");
        restored = restore_sentinels(&restored, params);
        Ok(format!("{comments}{};\n", restored.trim_end_matches('\n')))
    }
}
