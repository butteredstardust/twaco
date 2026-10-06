//! `twaco init`: propose a `twaco.toml` for a repository that has none.
//!
//! Everything is read from the repository rather than asked. Projects come from the entities'
//! own `projectName`. A project's root is where its collection folders sit. The
//! sidecar root is a `src` found beside them. The script layout is the one the existing payloads
//! already use, so adopting twaco does not relayout a project that has not decided to.
//!
//! An entity file counts only when a folder above it is named after its own collection, as
//! `Things/X.xml` is. That keeps out bundles in `dist`, designer exports, and sidecar fragments,
//! which are XML too but not a project's source.

use super::entity;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Folders never walked: tooling output, dependencies, and anything hidden.
const SKIPPED: [&str; 4] = ["target", "node_modules", "dist", "distribution-backend"];

#[derive(Debug, Default)]
pub struct Proposal {
    /// The proposed file.
    pub toml: String,
    /// What the proposal is based on, and what it could not decide.
    pub notes: Vec<String>,
    pub projects: usize,
}

#[derive(Default)]
struct Found {
    /// Entity count per candidate root. More than one root means the project is split.
    roots: BTreeMap<PathBuf, usize>,
    /// Payloads that start indented, and payloads that start flush left.
    indented: usize,
    flush: usize,
}

pub fn propose(root: &Path) -> Proposal {
    let mut projects: BTreeMap<String, Found> = BTreeMap::new();
    let mut undeclared = 0usize;
    let mut files = Vec::new();
    walk(root, &mut files);
    for path in files {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(info) = entity::parse(&bytes) else {
            continue;
        };
        let Some(collection_dir) = path.ancestors().skip(1).find(|dir| {
            dir.file_name()
                .is_some_and(|n| n == info.collection.as_str())
        }) else {
            continue;
        };
        let project_root = collection_dir.parent().unwrap_or(root).to_path_buf();
        if info.project.is_empty() {
            undeclared += 1;
            continue;
        }
        let found = projects.entry(info.project).or_default();
        *found.roots.entry(project_root).or_default() += 1;
        let (indented, flush) = payload_layout(&bytes);
        found.indented += indented;
        found.flush += flush;
    }

    let mut proposal = Proposal {
        projects: projects.len(),
        ..Proposal::default()
    };
    if projects.is_empty() {
        proposal.notes.push(format!(
            "no entity files found under {}: an entity must sit in a folder named after its collection, such as Things/",
            root.display()
        ));
        return proposal;
    }
    let relative = |path: &Path| -> String {
        let text = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/");
        if text.is_empty() {
            ".".to_string()
        } else {
            text
        }
    };

    // The sidecar root: one `src` beside a project's collections, shared by the solution.
    let mut src_roots: Vec<String> = projects
        .values()
        .filter_map(|found| {
            found
                .roots
                .iter()
                .max_by_key(|(_, n)| **n)
                .map(|(r, _)| r.join("src"))
        })
        .filter(|src| src.is_dir())
        .map(|src| relative(&src))
        .collect();
    src_roots.sort();
    src_roots.dedup();
    let src = match src_roots.as_slice() {
        [] => {
            proposal.notes.push(
                "no sidecar folder found; src is the default, `src` at the repository root"
                    .to_string(),
            );
            "src".to_string()
        }
        [one] => one.clone(),
        several => {
            proposal.notes.push(format!(
                "several sidecar folders ({}); twaco keeps one per solution, and the first is proposed",
                several.join(", ")
            ));
            several[0].clone()
        }
    };

    let (indented, flush) = projects.values().fold((0, 0), |(i, f), found| {
        (i + found.indented, f + found.flush)
    });
    let indent = indented > flush;
    proposal.notes.push(format!(
        "script payloads: {indented} indented, {flush} flush left, so indent_cdata_payload = {indent}{}",
        if indent { " (a compatibility layout)" } else { "" }
    ));
    if undeclared > 0 {
        proposal.notes.push(format!(
            "{undeclared} entity file(s) declare no projectName and belong to no project"
        ));
    }
    proposal.notes.push(
        "not proposed, because the files cannot say: which services legitimately override an inherited \
         definition. `twaco check` names each one it finds; list them in [validate] inherited_overrides"
            .to_string(),
    );

    let name = projects.keys().next().cloned().unwrap_or_default();
    let mut toml = format!(
        "# Proposed by `twaco init`. Read it before committing it.\n\n[solution]\nname = {}\nsrc = {}\n\n[format]\nindent_cdata_payload = {indent}\n",
        quote(&name),
        quote(&src)
    );
    for (project, found) in &projects {
        let (main_root, count) = found
            .roots
            .iter()
            .max_by_key(|(_, n)| **n)
            .expect("a project has a root");
        if found.roots.len() > 1 {
            let others: Vec<String> = found
                .roots
                .keys()
                .filter(|r| *r != main_root)
                .map(|r| relative(r))
                .collect();
            proposal.notes.push(format!(
                "{project}: {count} entities under {}, and more under {}; only the first root is proposed",
                relative(main_root),
                others.join(", ")
            ));
        }
        toml.push_str(&format!(
            "\n[[project]]\nname = {}\nroot = {}\n",
            quote(project),
            quote(&relative(main_root))
        ));
    }
    if projects.len() > 1 {
        toml.push_str("\n# Deploy order is not something the files can say. If a project imports after another,\n# give it depends_on = [\"<that project>\"].\n");
    }
    proposal.toml = toml;
    proposal
}

fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !name.starts_with('.') && !SKIPPED.contains(&name.as_str()) {
                walk(&path, out);
            }
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("xml"))
        {
            out.push(path);
        }
    }
}

/// How many service script payloads in a document start indented, and how many flush left, by
/// their first non-blank line.
fn payload_layout(bytes: &[u8]) -> (usize, usize) {
    let text = String::from_utf8_lossy(bytes);
    let (mut indented, mut flush) = (0, 0);
    let mut rest = text.as_ref();
    while let Some(at) = rest.find("<code>") {
        rest = &rest[at + "<code>".len()..];
        let Some(open) = rest.find("<![CDATA[") else {
            break;
        };
        // The CDATA must belong to this <code>, not a later element.
        if rest[..open].contains('<') {
            continue;
        }
        let body = &rest[open + "<![CDATA[".len()..];
        let payload = &body[..body.find("]]>").unwrap_or(body.len())];
        if let Some(line) = payload.lines().find(|l| !l.trim().is_empty()) {
            if line.starts_with([' ', '\t']) {
                indented += 1;
            } else {
                flush += 1;
            }
        }
    }
    (indented, flush)
}

/// The files that route an agent to what it needs, for a solution of these projects: an
/// AGENTS.md whose first section is for people to fill in, and a CLAUDE.md pointing at it.
/// What twaco knows is in `twaco guide` and `twaco catalog`, so these stay short.
pub fn agent_files(solution_name: &str, projects: &[String]) -> [(&'static str, String); 2] {
    let name = if solution_name.is_empty() {
        projects.join(", ")
    } else {
        solution_name.to_string()
    };
    let listed: String = projects.iter().map(|p| format!("- `{p}`\n")).collect();
    let agents = format!(
        r#"# AGENTS.md

Guidance for coding agents working on {name}, a ThingWorx solution kept in source control
and managed with twaco. Read this file in full before changing anything.

## Project context

<!-- For people to fill in: what the solution is for, who uses it, where its data lives
     (DataTables, a database through a Thing, streams), the invariants that must hold, and
     which parts are fragile. An agent cannot derive these from the entities. -->

Projects, in deploy order (`twaco projects`):

{listed}
## How to work here

- `twaco guide workflow`: how to change a service, deploy it, call it, and take a designer's
  drop. Follow it.
- `twaco guide --search <words>`: the ThingWorx platform's verified-live quirks, the
  service-code reference, and this repository's own documents. Search it before a live import,
  a hand-written mashup binding, a configuration-table change, or a service that touches JSON.
- `twaco catalog [<entity>]`: every service, with its signature and description, derived from
  the entities.
- `twaco check` before reporting a change complete. Then deploy only what changed and call
  the service; the gates check structure, never behaviour.
- A command that writes to the server only plans unless given `--apply`.

## What we learned

<!-- Write here, or in docs/, what was expensive to find out about this solution: the next
     agent starts from these files, not from your conversation. Platform behaviour that is not
     specific to this solution belongs in a quirk instead. -->
"#
    );
    let claude = "# CLAUDE.md\n\n`AGENTS.md` is the source of truth for this repository: read it in full before making\nchanges. `twaco guide --search <words>` finds the platform's quirks and this repository's\ndocuments; `twaco mcp` serves every twaco command as a tool.\n".to_string();
    [("AGENTS.md", agents), ("CLAUDE.md", claude)]
}

/// Writes each agent file that does not exist yet, never replacing one. Returns what it wrote
/// and what it left alone.
pub fn write_agent_files(
    root: &Path,
    solution_name: &str,
    projects: &[String],
) -> std::io::Result<(Vec<String>, Vec<String>)> {
    let mut wrote = Vec::new();
    let mut kept = Vec::new();
    for (file, text) in agent_files(solution_name, projects) {
        let path = root.join(file);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut handle) => {
                std::io::Write::write_all(&mut handle, text.as_bytes())?;
                wrote.push(file.to_string());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => kept.push(file.to_string()),
            Err(e) => return Err(e),
        }
    }
    Ok((wrote, kept))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        let nonce = crate::test_nonce();
        std::env::temp_dir().join(format!("twaco-init-{}-{nonce}", std::process::id()))
    }

    fn write(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn thing(name: &str, project: &str, code: &str) -> String {
        format!(
            "<Entities><Things><Thing name=\"{name}\" projectName=\"{project}\"><ThingShape><ServiceImplementations>\
             <ServiceImplementation name=\"S\" handlerName=\"Script\"><ConfigurationTables><ConfigurationTable name=\"Script\">\
             <Rows><Row><code><![CDATA[{code}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables>\
             </ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>"
        )
    }

    #[test]
    fn projects_roots_sidecars_and_layout_come_from_the_repository() {
        let root = repo();
        write(
            root.join("backend/Things/B.T.xml"),
            &thing("B.T", "Backend", "\nflush();\n"),
        );
        write(
            root.join("ui/Things/U.T.xml"),
            &thing("U.T", "UI", "\nalso();\n"),
        );
        write(
            root.join("backend/src/B.T/services/S/script.js"),
            "flush();",
        );
        // Not source: a bundle in dist and an export folder that is not a collection.
        write(
            root.join("dist/bundle.xml"),
            &thing("X.T", "Backend", "x();"),
        );
        write(
            root.join("exported/drop.xml"),
            &thing("Y.T", "Backend", "y();"),
        );

        let proposal = propose(&root);
        assert_eq!(proposal.projects, 2);
        assert!(
            proposal
                .toml
                .contains("name = \"Backend\"\nroot = \"backend\""),
            "{}",
            proposal.toml
        );
        assert!(proposal.toml.contains("name = \"UI\"\nroot = \"ui\""));
        assert!(proposal.toml.contains("src = \"backend/src\""));
        assert!(proposal.toml.contains("indent_cdata_payload = false"));
        assert!(
            proposal.toml.contains("depends_on"),
            "a multi-project solution is told about deploy order"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn agent_files_are_written_once_and_never_replace_one() {
        let nonce = crate::test_nonce();
        let root =
            std::env::temp_dir().join(format!("twaco-init-agents-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "mine").unwrap();
        let (wrote, kept) =
            write_agent_files(&root, "S", &["P.One".into(), "P.Two".into()]).unwrap();
        assert_eq!(
            (wrote, kept),
            (vec!["AGENTS.md".to_string()], vec!["CLAUDE.md".to_string()])
        );
        let agents = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
        assert!(
            agents.contains("working on S,") && agents.contains("- `P.One`\n- `P.Two`\n"),
            "{agents}"
        );
        assert!(agents.contains("twaco guide workflow") && agents.contains("twaco catalog"));
        assert_eq!(
            std::fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "mine",
            "never replaced"
        );
        let (wrote, _) = write_agent_files(&root, "S", &[]).unwrap();
        assert!(wrote.is_empty(), "a second run writes nothing");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn indented_payloads_propose_the_compatibility_layout() {
        let root = repo();
        write(
            root.join("Things/P.A.xml"),
            &thing("P.A", "P", "\n            a();\n            "),
        );
        write(
            root.join("Things/P.B.xml"),
            &thing("P.B", "P", "\n            b();\n            "),
        );
        let proposal = propose(&root);
        assert!(proposal.toml.contains("root = \".\""));
        assert!(proposal.toml.contains("indent_cdata_payload = true"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_repository_without_entities_proposes_nothing_and_says_why() {
        let root = repo();
        std::fs::create_dir_all(&root).unwrap();
        let proposal = propose(&root);
        assert_eq!(proposal.projects, 0);
        assert!(proposal.toml.is_empty());
        assert!(proposal.notes[0].contains("no entity files"));
        let _ = std::fs::remove_dir_all(root);
    }
}
