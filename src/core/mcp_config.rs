//! `twaco mcp configure`: register twaco's MCP server with a coding assistant.
//!
//! Each client keeps its servers in its own file, which belongs to the user: only twaco's own
//! entry is ever added or replaced, every other byte of the file stays as it was, a changed file
//! is backed up beside itself first, and a file twaco cannot edit safely is refused rather than
//! rewritten. An entry that already says what twaco would write is left alone.
//!
//! - Codex: `~/.codex/config.toml`, table `[mcp_servers.twaco]` (`command`, `args`, `env`).
//! - Cursor: `<project>/.cursor/mcp.json`, or `~/.cursor/mcp.json`, key `mcpServers.twaco`.
//! - Claude Code: `<project>/.mcp.json`, key `mcpServers.twaco`. Its user scope lives in
//!   `~/.claude.json`, a large file Claude Code manages itself, so for that twaco prints the
//!   `claude mcp add` command rather than editing it.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The name the server is registered under.
pub const SERVER: &str = "twaco";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    Codex,
    ClaudeCode,
    Cursor,
}

impl Client {
    pub fn parse(word: &str) -> Result<Client, String> {
        match word {
            "codex" => Ok(Client::Codex),
            "claude" | "claude-code" => Ok(Client::ClaudeCode),
            "cursor" => Ok(Client::Cursor),
            other => Err(format!(
                "no client {other:?}: twaco configures codex, claude-code or cursor"
            )),
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Client::Codex => "codex",
            Client::ClaudeCode => "claude-code",
            Client::Cursor => "cursor",
        }
    }

    /// Where the client reads servers by default.
    pub fn default_scope(self) -> Scope {
        match self {
            Client::Codex => Scope::User,
            Client::ClaudeCode | Client::Cursor => Scope::Project,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every project of this user.
    User,
    /// This solution only: a file in its root, which a team can commit.
    Project,
}

impl Scope {
    pub fn parse(word: &str) -> Result<Scope, String> {
        match word {
            "user" => Ok(Scope::User),
            "project" => Ok(Scope::Project),
            other => Err(format!("no scope {other:?}: user or project")),
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project => "project",
        }
    }
}

/// What to register: the program and, for a user-wide entry, the solution it serves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub command: String,
    pub args: Vec<String>,
    /// `TWACO_ROOT`, for an entry the client starts outside the solution.
    pub root: Option<String>,
}

impl Entry {
    fn json(&self) -> Value {
        let mut entry = Map::new();
        entry.insert("command".into(), Value::from(self.command.clone()));
        entry.insert(
            "args".into(),
            Value::Array(self.args.iter().cloned().map(Value::from).collect()),
        );
        if let Some(root) = &self.root {
            let mut env = Map::new();
            env.insert("TWACO_ROOT".into(), Value::from(root.clone()));
            entry.insert("env".into(), Value::Object(env));
        }
        Value::Object(entry)
    }
}

/// What configuring would do, and the file's text afterwards.
#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    /// The entry is absent: it would be added.
    Add,
    /// The file has twaco's entry, saying the same.
    Same,
    /// The file has a different entry under twaco's name: replaced only when asked.
    Differs { current: String },
}

#[derive(Debug)]
pub struct Plan {
    pub client: Client,
    pub scope: Scope,
    /// The file the client reads; `None` when twaco does not edit it (Claude Code, user scope).
    pub file: Option<PathBuf>,
    pub change: Change,
    /// The entry as the file will hold it.
    pub entry_text: String,
    /// The whole file after the change; `None` for [`Change::Same`] or no file.
    after: Option<String>,
    /// A command that registers the entry when twaco does not edit the file itself.
    pub command: Option<String>,
}

impl Plan {
    /// Whether applying would write.
    pub fn writes(&self) -> bool {
        self.after.is_some()
    }
}

/// The file a client keeps its servers in, for a scope.
pub fn config_file(client: Client, scope: Scope, project: &Path, home: &Path) -> Option<PathBuf> {
    match (client, scope) {
        (Client::Codex, Scope::User) => Some(home.join(".codex").join("config.toml")),
        // Codex reads a project's servers from `.codex/config.toml` only when the project is
        // trusted; the user file is the one it always reads.
        (Client::Codex, Scope::Project) => Some(project.join(".codex").join("config.toml")),
        (Client::Cursor, Scope::User) => Some(home.join(".cursor").join("mcp.json")),
        (Client::Cursor, Scope::Project) => Some(project.join(".cursor").join("mcp.json")),
        (Client::ClaudeCode, Scope::Project) => Some(project.join(".mcp.json")),
        (Client::ClaudeCode, Scope::User) => None,
    }
}

/// Work out the change, reading the client's file if it exists. Nothing is written.
pub fn plan(
    client: Client,
    scope: Scope,
    entry: &Entry,
    project: &Path,
    home: &Path,
    replace: bool,
) -> Result<Plan, String> {
    let Some(file) = config_file(client, scope, project, home) else {
        let env = entry
            .root
            .as_ref()
            .map(|root| format!(" -e TWACO_ROOT={}", quote(root)))
            .unwrap_or_default();
        return Ok(Plan {
            client,
            scope,
            file: None,
            change: Change::Add,
            entry_text: serde_json::to_string_pretty(&entry.json()).expect("JSON serialises"),
            after: None,
            command: Some(format!(
                "claude mcp add --scope user{env} {SERVER} -- {} {}",
                quote(&entry.command),
                entry
                    .args
                    .iter()
                    .map(|arg| quote(arg))
                    .collect::<Vec<_>>()
                    .join(" ")
            )),
        });
    };
    let existing = match std::fs::read_to_string(&file) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("{}: {error}", file.display())),
    };
    let (change, entry_text, after) = match client {
        Client::Codex => codex(existing.as_deref(), entry, &file)?,
        Client::ClaudeCode | Client::Cursor => json(existing.as_deref(), entry, &file)?,
    };
    if let (Change::Differs { .. }, false) = (&change, replace) {
        return Ok(Plan {
            client,
            scope,
            file: Some(file),
            change,
            entry_text,
            after: None,
            command: None,
        });
    }
    Ok(Plan {
        client,
        scope,
        after: (change != Change::Same).then_some(after),
        file: Some(file),
        change,
        entry_text,
        command: None,
    })
}

/// Write the planned file: the old one is copied to `<file>.twaco-backup-<stamp>` first, and the
/// new one goes in by an atomic rename. Returns the backup's path, if there was a file to back up.
pub fn apply(plan: &Plan) -> Result<Option<PathBuf>, String> {
    let (Some(file), Some(after)) = (&plan.file, &plan.after) else {
        return Ok(None);
    };
    let dir = file
        .parent()
        .ok_or_else(|| format!("{} has no folder", file.display()))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let backup = if file.exists() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let backup = dir.join(format!("{name}.twaco-backup-{stamp}"));
        std::fs::copy(file, &backup).map_err(|e| format!("{}: {e}", backup.display()))?;
        Some(backup)
    } else {
        None
    };
    let staged = dir.join(format!(".twaco-configure-{}", std::process::id()));
    std::fs::write(&staged, after).map_err(|e| format!("{}: {e}", staged.display()))?;
    if let Err(error) = std::fs::rename(&staged, file) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("{}: {error}", file.display()));
    }
    Ok(backup)
}

/// Codex's TOML: only the `[mcp_servers.twaco]` table is touched, through `toml_edit`, so the
/// rest of the file (comments, order, other servers, a gateway's wrapped entries) stays.
fn codex(
    existing: Option<&str>,
    entry: &Entry,
    file: &Path,
) -> Result<(Change, String, String), String> {
    let text = existing.unwrap_or("");
    let mut document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| format!("{} is not TOML twaco can edit: {error}", file.display()))?;
    let mut table = toml_edit::Table::new();
    table.insert("command", toml_edit::value(entry.command.clone()));
    let mut args = toml_edit::Array::new();
    for arg in &entry.args {
        args.push(arg.clone());
    }
    table.insert("args", toml_edit::value(args));
    if let Some(root) = &entry.root {
        let mut env = toml_edit::Table::new();
        env.insert("TWACO_ROOT", toml_edit::value(root.clone()));
        table.insert("env", toml_edit::Item::Table(env));
    }
    let wanted = toml_edit::Item::Table(table);
    // As the file will hold it, nested `env` table included: only a document displays that.
    let entry_text = {
        let mut preview = toml_edit::DocumentMut::new();
        let mut servers = toml_edit::Table::new();
        servers.set_implicit(true);
        servers.insert(SERVER, wanted.clone());
        preview.insert("mcp_servers", toml_edit::Item::Table(servers));
        preview.to_string().trim().to_string()
    };

    let servers = document.entry("mcp_servers").or_insert_with(|| {
        let mut servers = toml_edit::Table::new();
        servers.set_implicit(true);
        toml_edit::Item::Table(servers)
    });
    let Some(servers) = servers.as_table_like_mut() else {
        return Err(format!("{}: mcp_servers is not a table", file.display()));
    };
    let change = match servers.get(SERVER) {
        None => Change::Add,
        Some(current) if same_codex(current, entry) => Change::Same,
        Some(current) => Change::Differs {
            current: current.to_string().trim().to_string(),
        },
    };
    servers.insert(SERVER, wanted);
    Ok((change, entry_text, document.to_string()))
}

fn same_codex(current: &toml_edit::Item, entry: &Entry) -> bool {
    let text = |key: &str| {
        current
            .get(key)
            .and_then(toml_edit::Item::as_str)
            .map(str::to_string)
    };
    let args: Option<Vec<String>> = current.get("args").and_then(|args| {
        args.as_array().map(|array| {
            array
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
    });
    let root = current
        .get("env")
        .and_then(|env| env.get("TWACO_ROOT"))
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string);
    text("command").as_deref() == Some(entry.command.as_str())
        && args.as_deref() == Some(entry.args.as_slice())
        && root == entry.root
}

/// A JSON `mcpServers` file. Its key order is kept; a file with comments, or that is not JSON,
/// is refused rather than rewritten without them.
fn json(
    existing: Option<&str>,
    entry: &Entry,
    file: &Path,
) -> Result<(Change, String, String), String> {
    let mut document: Value = match existing {
        None => Value::Object(Map::new()),
        Some(text) if text.trim().is_empty() => Value::Object(Map::new()),
        Some(text) => serde_json::from_str(text).map_err(|error| {
            format!(
                "{} is not plain JSON ({error}); twaco does not rewrite it, so add the entry by hand",
                file.display()
            )
        })?,
    };
    let Some(root) = document.as_object_mut() else {
        return Err(format!("{} does not hold a JSON object", file.display()));
    };
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(servers) = servers.as_object_mut() else {
        return Err(format!("{}: mcpServers is not an object", file.display()));
    };
    let wanted = entry.json();
    let change = match servers.get(SERVER) {
        None => Change::Add,
        Some(current) if *current == wanted => Change::Same,
        Some(current) => Change::Differs {
            current: serde_json::to_string_pretty(current).expect("JSON serialises"),
        },
    };
    let entry_text = serde_json::to_string_pretty(&wanted).expect("JSON serialises");
    servers.insert(SERVER.into(), wanted);
    let newline = existing.map_or("\n", |text| super::sync::newline_of(text));
    let after = format!(
        "{}\n",
        serde_json::to_string_pretty(&document).expect("JSON serialises")
    )
    .replace('\n', newline);
    Ok((change, entry_text, after))
}

/// A word for a shell command line, quoted when it needs to be.
fn quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:\\=".contains(c))
    {
        word.to_string()
    } else {
        format!("\"{}\"", word.replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(root: Option<&str>) -> Entry {
        Entry {
            command: "C:/tools/twaco.exe".into(),
            args: vec!["mcp".into()],
            root: root.map(str::to_string),
        }
    }

    #[test]
    fn codex_gets_only_its_own_table_and_keeps_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        let (project, home) = (dir.path().join("p"), dir.path().join("h"));
        let file = home.join(".codex/config.toml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let original = "# my settings\nmodel = \"x\"\n\n[mcp_servers.other]\ncommand = 'gw.exe' # wrapped\nargs = ['--', 'other']\n";
        std::fs::write(&file, original).unwrap();

        let plan = super::plan(
            Client::Codex,
            Scope::User,
            &entry(Some("C:/sol")),
            &project,
            &home,
            false,
        )
        .unwrap();
        assert_eq!(plan.change, Change::Add);
        assert!(
            plan.entry_text.contains("[mcp_servers.twaco.env]"),
            "{}",
            plan.entry_text
        );
        let backup = apply(&plan).unwrap().expect("a backup of the old file");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        let after = std::fs::read_to_string(&file).unwrap();
        assert!(
            after.starts_with(original),
            "the old text is untouched:\n{after}"
        );
        assert!(after.contains("[mcp_servers.twaco]"), "{after}");
        assert!(after.contains("TWACO_ROOT = \"C:/sol\""), "{after}");

        // Again: nothing to do.
        let again = super::plan(
            Client::Codex,
            Scope::User,
            &entry(Some("C:/sol")),
            &project,
            &home,
            false,
        )
        .unwrap();
        assert_eq!(again.change, Change::Same);
        assert!(!again.writes());

        // A different twaco entry is replaced only when asked.
        let other = Entry {
            command: "twaco".into(),
            ..entry(Some("C:/sol"))
        };
        let refused =
            super::plan(Client::Codex, Scope::User, &other, &project, &home, false).unwrap();
        assert!(matches!(refused.change, Change::Differs { .. }));
        assert!(!refused.writes());
        let replaced =
            super::plan(Client::Codex, Scope::User, &other, &project, &home, true).unwrap();
        apply(&replaced).unwrap();
        let after = std::fs::read_to_string(&file).unwrap();
        assert!(after.contains("command = \"twaco\""), "{after}");
        assert!(
            after.contains("[mcp_servers.other]\ncommand = 'gw.exe' # wrapped"),
            "{after}"
        );
    }

    #[test]
    fn a_project_json_file_is_created_or_extended_and_comments_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (project, home) = (dir.path().join("p"), dir.path().join("h"));
        let created = super::plan(
            Client::ClaudeCode,
            Scope::Project,
            &entry(None),
            &project,
            &home,
            false,
        )
        .unwrap();
        assert_eq!(
            created.file.as_deref(),
            Some(project.join(".mcp.json").as_path())
        );
        assert!(apply(&created).unwrap().is_none(), "no old file, no backup");
        let value: Value =
            serde_json::from_str(&std::fs::read_to_string(project.join(".mcp.json")).unwrap())
                .unwrap();
        assert_eq!(
            value["mcpServers"]["twaco"]["args"],
            serde_json::json!(["mcp"])
        );
        assert!(value["mcpServers"]["twaco"].get("env").is_none());

        let cursor = project.join(".cursor/mcp.json");
        std::fs::create_dir_all(cursor.parent().unwrap()).unwrap();
        std::fs::write(&cursor, "{\r\n  \"mcpServers\": {\r\n    \"other\": {\"command\": \"x\"}\r\n  },\r\n  \"keep\": 1\r\n}\r\n").unwrap();
        let extended = super::plan(
            Client::Cursor,
            Scope::Project,
            &entry(None),
            &project,
            &home,
            false,
        )
        .unwrap();
        apply(&extended).unwrap();
        let text = std::fs::read_to_string(&cursor).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["mcpServers"]["other"]["command"], "x");
        assert_eq!(value["keep"], 1);
        assert!(
            !text.replace("\r\n", "").contains('\n'),
            "CRLF kept: {text:?}"
        );

        std::fs::write(&cursor, "{ // mine\n}\n").unwrap();
        let error = super::plan(
            Client::Cursor,
            Scope::Project,
            &entry(None),
            &project,
            &home,
            false,
        )
        .unwrap_err();
        assert!(error.contains("not plain JSON"), "{error}");
    }

    #[test]
    fn claude_code_user_scope_is_a_command_not_an_edit() {
        let dir = tempfile::tempdir().unwrap();
        let plan = super::plan(
            Client::ClaudeCode,
            Scope::User,
            &entry(Some("C:/my sol")),
            dir.path(),
            dir.path(),
            false,
        )
        .unwrap();
        assert!(plan.file.is_none() && !plan.writes());
        assert_eq!(
            plan.command.as_deref(),
            Some("claude mcp add --scope user -e TWACO_ROOT=\"C:/my sol\" twaco -- C:/tools/twaco.exe mcp")
        );
    }
}
