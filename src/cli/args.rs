use super::super::*;

/// One command's arguments, after validation.
pub(crate) struct Args {
    pub(crate) names: Vec<String>,
    pub(crate) flags: Vec<String>,
    /// `--project <name>`, narrowing to one project of the solution.
    pub(crate) project: Option<String>,
    /// Server profile name. It is only resolved by server commands.
    pub(crate) profile: Option<String>,
    /// Raw entity GET destination.
    pub(crate) out: Option<PathBuf>,
    /// Repeated deploy entity selectors.
    pub(crate) only: Vec<String>,
    /// Deploy project selectors parsed from comma-separated values.
    pub(crate) only_projects: Vec<String>,
    /// Per-request timeout for an opaque service call.
    pub(crate) timeout: Option<Duration>,
    /// Where `config-table --backup` writes, and what `--restore` reads.
    pub(crate) backup: Option<PathBuf>,
    pub(crate) restore: Option<PathBuf>,
    /// `adopt --entity`: name fragments, repeatable.
    pub(crate) entity_filters: Vec<String>,
    /// Flags that take one value and need no parsing here, such as `logs --since`.
    pub(crate) values: std::collections::BTreeMap<String, String>,
}

/// Flags whose value is kept as text in `Args::values`, for the command to read.
const VALUE_FLAGS: &[&str] = &[
    "--since",
    "--from",
    "--to",
    "--level",
    "--grep",
    "--regex",
    "--user",
    "--thread",
    "--origin",
    "--limit",
    "--sublogger",
    "--version",
    "--section",
    "--member",
    "--min-confidence",
    "--depth",
    "--repository",
    "--path",
    "--collection",
    "--tags",
    "--zip",
    "--search",
    "--thing",
    "--max-rows",
    "-q",
    "--sql-dir",
    "--map",
    "--as",
    "--add-shapes",
    "--remove-shapes",
    "--type",
    "--display-name",
    "--description",
    "--parent",
    "--root",
    "--base-extension",
];

impl Args {
    /// Split arguments into names and flags, refusing anything the command does not accept.
    pub(crate) fn parse(args: &[String], known: &[&str]) -> Result<Args, String> {
        let mut names = Vec::new();
        let mut flags = Vec::new();
        let mut project = None;
        let mut profile = None;
        let mut out = None;
        let mut only = Vec::new();
        let mut only_projects = Vec::new();
        let mut timeout = None;
        let mut backup = None;
        let mut restore = None;
        let mut entity_filters = Vec::new();
        let mut values = std::collections::BTreeMap::new();
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            if let Some(flag) = arg
                .strip_prefix("--")
                .map(|_| arg.as_str())
                .or_else(|| (arg == "-q").then_some("-q"))
            {
                if !known.contains(&flag) {
                    return Err(format!(
                        "`{flag}` is not a flag this command takes ({})",
                        if known.is_empty() {
                            "it takes none".to_string()
                        } else {
                            known.join(", ")
                        }
                    ));
                }
                match flag {
                    "--project" => {
                        project = Some(
                            rest.next()
                                .cloned()
                                .ok_or_else(|| "--project needs a name".to_string())?,
                        );
                    }
                    "--profile" => {
                        profile = Some(
                            rest.next()
                                .cloned()
                                .ok_or_else(|| "--profile needs a name".to_string())?,
                        );
                    }
                    "--out" => {
                        out = Some(PathBuf::from(
                            rest.next()
                                .cloned()
                                .ok_or_else(|| "--out needs a path".to_string())?,
                        ));
                    }
                    "--only" => only.push(
                        rest.next()
                            .cloned()
                            .ok_or_else(|| "--only needs an entity name".to_string())?,
                    ),
                    "--only-projects" => {
                        let value = rest.next().cloned().ok_or_else(|| {
                            "--only-projects needs a comma-separated list".to_string()
                        })?;
                        let selected: Vec<String> = value
                            .split(',')
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .map(str::to_string)
                            .collect();
                        if selected.is_empty() {
                            return Err("--only-projects needs at least one project".to_string());
                        }
                        only_projects.extend(selected);
                    }
                    "--timeout" => {
                        let value = rest.next().ok_or_else(|| {
                            "--timeout needs a positive number of seconds".to_string()
                        })?;
                        let seconds = value.parse::<u64>().map_err(|_| {
                            "--timeout needs a positive whole number of seconds".to_string()
                        })?;
                        if seconds == 0 {
                            return Err(
                                "--timeout needs a positive whole number of seconds".to_string()
                            );
                        }
                        timeout = Some(Duration::from_secs(seconds));
                    }
                    "--entity" => entity_filters.push(
                        rest.next()
                            .cloned()
                            .ok_or_else(|| "--entity needs a name fragment".to_string())?,
                    ),
                    "--backup" => {
                        backup = Some(PathBuf::from(
                            rest.next()
                                .cloned()
                                .ok_or_else(|| "--backup needs a file".to_string())?,
                        ));
                    }
                    "--restore" => {
                        restore = Some(PathBuf::from(
                            rest.next()
                                .cloned()
                                .ok_or_else(|| "--restore needs a file".to_string())?,
                        ));
                    }
                    valued if VALUE_FLAGS.contains(&valued) => {
                        let value = rest
                            .next()
                            .cloned()
                            .ok_or_else(|| format!("{valued} needs a value"))?;
                        if values.insert(valued.to_string(), value).is_some() {
                            return Err(format!("{valued} is given twice"));
                        }
                    }
                    _ => flags.push(flag.to_string()),
                }
            } else {
                names.push(arg.clone());
            }
        }
        if flags.iter().any(|f| f == "--all") && !names.is_empty() {
            return Err(format!(
                "--all and {names:?} say different things; pass one or the other"
            ));
        }
        Ok(Args {
            names,
            flags,
            project,
            profile,
            out,
            only,
            only_projects,
            timeout,
            backup,
            restore,
            entity_filters,
            values,
        })
    }

    pub(crate) fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}
