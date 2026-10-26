//! The `localization tokens` gate: every `[[token]]` an entity names under one of the solution's
//! token prefixes must be in a Default table file, and the table files must be ones the server
//! takes.
//!
//! ThingWorx imports a mashup or menu that names a token no table has, and the label then shows
//! the raw `[[name]]` text. Tokens outside the solution's prefixes (`[[showMore]]`, the widgets'
//! own `[[Widgets.Validation.…]]`) are the platform's or an extension's: they are counted as not
//! checked rather than guessed at.

use super::ops::project_for;
use super::{discover, problems, root, Problem, DEFAULT_TABLE};
use crate::core::config::Solution;
use crate::core::{scan, workspace};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(crate) const GATE: &str = "localization tokens";

/// One thing the gate found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TokenProblem {
    pub(crate) file: String,
    /// 1-based; zero for a finding about the whole file.
    pub(crate) line: usize,
    pub(crate) rule: &'static str,
    pub(crate) message: String,
}

#[derive(Debug, Default)]
pub(crate) struct Report {
    /// References to the solution's own tokens that were checked.
    pub(crate) checked: usize,
    /// Entity files that hold at least one `[[token]]`.
    pub(crate) files: usize,
    /// References to tokens outside every project's prefixes.
    pub(crate) not_checked: usize,
    pub(crate) problems: Vec<TokenProblem>,
    /// Per language table, the solution's Default tokens it lacks: they show the Default text.
    pub(crate) untranslated: BTreeMap<String, Vec<String>>,
    /// The solution's Default tokens that no entity names, as `[[name]]` or as the bare name a
    /// script passes to a localization function. A name built at run time is not seen.
    pub(crate) unreferenced: Vec<String>,
}

#[cfg(test)]
pub(crate) fn run(solution: &Solution) -> Report {
    let discovered = workspace::discover(solution);
    run_in(solution, &discovered)
}

/// [`run`] over a scan already made.
pub(crate) fn run_in(solution: &Solution, discovered: &workspace::Discovery) -> Report {
    let mut report = Report::default();
    let shown = |path: &Path| solution.relative(path);
    let found = match discover(&root(solution)) {
        Ok(found) => found,
        Err(error) => {
            report.problems.push(TokenProblem {
                file: shown(&root(solution)),
                line: 0,
                rule: "unreadable-table",
                message: error.to_string(),
            });
            return report;
        }
    };
    for (path, why) in &found.unreadable {
        report.problems.push(TokenProblem {
            file: shown(path),
            line: 0,
            rule: "unreadable-table",
            message: why.clone(),
        });
    }
    if let Err(error) = super::ops::check_prefixes(solution) {
        report.problems.push(TokenProblem {
            file: crate::core::config::CONFIG_FILE.to_string(),
            line: 0,
            rule: "prefix-clash",
            message: error.to_string(),
        });
    }
    let prefixes: Vec<String> = solution.projects.iter().flat_map(super::prefixes).collect();
    for problem in problems(&found.files, &prefixes) {
        match problem {
            Problem::Duplicate { table, name, files } => report.problems.push(TokenProblem {
                file: shown(&files[0]),
                line: 0,
                rule: "duplicate-token",
                message: format!(
                    "{name} is in table {table} more than once ({}); keep one row",
                    files
                        .iter()
                        .map(|file| shown(file))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }),
            Problem::NotInDefault { table, name, file } => report.problems.push(TokenProblem {
                file: shown(&file),
                line: 0,
                rule: "not-in-default",
                message: format!(
                    "{name} is in table {table} but not in {DEFAULT_TABLE}; the server refuses \
                     it. Add it with `twaco localization set {name} <value>`"
                ),
            }),
            // A missing translation shows the Default text: normal while a language is filled
            // in, so it is a note rather than a finding.
            Problem::Untranslated { table, name } => {
                report.untranslated.entry(table).or_default().push(name);
            }
        }
    }
    let defined: BTreeSet<&str> = found
        .files
        .iter()
        .filter(|file| file.table == DEFAULT_TABLE)
        .flat_map(|file| file.tokens.iter().map(|token| token.name.as_str()))
        .collect();
    let mut named: BTreeSet<String> = BTreeSet::new();
    // Every name-like word of every entity and sidecar script, for tokens a script names bare.
    let mut words: BTreeSet<Vec<u8>> = BTreeSet::new();
    for file in &discovered.entities {
        let Ok(bytes) = std::fs::read(&file.path) else {
            // The project gate reports a file it cannot read.
            continue;
        };
        let found = references(&bytes);
        named.extend(found.iter().map(|(_, name)| name.clone()));
        let mut reported: BTreeSet<&str> = BTreeSet::new();
        for (line, name) in &found {
            let (line, name) = (*line, name.as_str());
            if project_for(solution, name).is_none() {
                report.not_checked += 1;
                continue;
            }
            report.checked += 1;
            // One finding per token and file: a mashup names a label in many widgets' states.
            if !defined.contains(name) && reported.insert(name) {
                report.problems.push(TokenProblem {
                    file: shown(&file.path),
                    line,
                    rule: "missing-token",
                    message: format!(
                        "[[{name}]] is not in the {DEFAULT_TABLE} localization table, so the \
                         label shows the raw text; add it with `twaco localization set {name} \
                         <value>`"
                    ),
                });
            }
        }
        if !found.is_empty() {
            report.files += 1;
        }
        add_words(&bytes, &mut words);
    }
    // A script edited in its sidecar names a token before `sync` writes it into the entity.
    for script in crate::core::workspace::script_files(solution).files {
        if let Ok(bytes) = std::fs::read(&script) {
            add_words(&bytes, &mut words);
        }
    }
    report.unreferenced = defined
        .iter()
        .filter(|name| project_for(solution, name).is_some() && !named.contains(**name))
        .filter(|name| !words.contains(name.as_bytes()))
        .map(|name| (*name).to_string())
        .collect();
    report
}

/// The whole words of `text` that could be a token name: runs of the bytes a name is made of, so
/// `Acme.Title` is not found in `Acme.TitleShort` or `Acme:Title:Long`.
fn add_words(text: &[u8], words: &mut BTreeSet<Vec<u8>>) {
    let part = |byte: &u8| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':') || *byte >= 0x80
    };
    for word in text
        .split(|byte| !part(byte))
        .filter(|word| !word.is_empty())
    {
        if !words.contains(word) {
            words.insert(word.to_vec());
        }
    }
}

/// The longest token name looked for. Names are short keys; the bound keeps a file full of
/// unterminated `[[` linear to scan.
const LONGEST: usize = 256;

/// Every `[[name]]` an entity file means, with its 1-based line: in text, CDATA and attribute
/// values, entities decoded as a parser would, and never in a comment. A file the scanner cannot
/// read is scanned as plain text; the project gate reports it.
fn references(src: &[u8]) -> Vec<(usize, String)> {
    let Ok(tokens) = scan::tokenize(src) else {
        return bracketed(&String::from_utf8_lossy(src));
    };
    let mut out = Vec::new();
    // Lines counted once, front to back, as the tokens come in order.
    let (mut counted_to, mut line) = (0, 1);
    for token in &tokens {
        let raw = match token.kind {
            scan::Kind::Comment | scan::Kind::Pi | scan::Kind::DocType | scan::Kind::End => {
                continue
            }
            scan::Kind::Cdata => token.inner.of(src),
            _ => token.span.of(src),
        };
        // `&#91;&#91;` decodes to `[[`: a raw `[[` or an entity is needed either way.
        if !contains(raw, b"[[") && !contains(raw, b"&") {
            continue;
        }
        let text = scan::decode_entities(&String::from_utf8_lossy(raw));
        line += src[counted_to..token.span.start]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count();
        counted_to = token.span.start;
        // Lines come from the bytes as written: a decoded `&#10;` is no line of the file. A name
        // that only decoding reveals (`&#91;&#91;...`) is reported where its token starts.
        let written = bracketed(&String::from_utf8_lossy(raw));
        for (within, name) in &written {
            out.push((line + within - 1, name.clone()));
        }
        if token.kind != scan::Kind::Cdata && contains(raw, b"&") {
            let mut seen: Vec<&String> = written.iter().map(|(_, name)| name).collect();
            for (_, name) in bracketed(&text) {
                if let Some(at) = seen.iter().position(|known| **known == name) {
                    seen.remove(at);
                } else {
                    out.push((line, name));
                }
            }
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|part| part == needle)
}

/// Every `[[name]]` in `text`, with its 1-based line within it. A name is what ThingWorx resolves:
/// no brackets, quotes, spaces or markup inside, not empty, and at most `LONGEST` bytes.
fn bracketed(text: &str) -> Vec<(usize, String)> {
    let bytes = text.as_bytes();
    let allowed = |byte: u8| {
        !byte.is_ascii_whitespace()
            && !matches!(byte, b'[' | b']' | b'"' | b'\'' | b'<' | b'>' | b'&')
    };
    let mut out = Vec::new();
    let mut line = 1;
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\n' {
            line += 1;
        } else if bytes[at] == b'[' && bytes.get(at + 1) == Some(&b'[') {
            let start = at + 2;
            let mut end = start;
            while end < bytes.len() && end - start <= LONGEST && allowed(bytes[end]) {
                end += 1;
            }
            // `end` stops on an ASCII byte, so the slice is on a character boundary.
            if end > start
                && end - start <= LONGEST
                && bytes.get(end) == Some(&b']')
                && bytes.get(end + 1) == Some(&b']')
            {
                out.push((line, text[start..end].to_string()));
                at = end + 2;
                continue;
            }
        }
        at += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &[u8] = include_bytes!(
        "../../../tests/fixtures/localization/flat/Acme.App_LocalizationTable_Default.xml"
    );

    fn write(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn solution(mashup: &str) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "twaco.toml",
            b"[[project]]
name = \"Acme.App\"
collections = [\"Mashups\"]
",
        );
        write(
            root,
            "localization/Acme.App_LocalizationTable_Default.xml",
            DEFAULT,
        );
        write(
            root,
            "Mashups/Acme.App.View.xml",
            format!(
                "<Entities><Mashups><Mashup name=\"Acme.App.View\" projectName=\"Acme.App\">                 <mashupContent><![CDATA[{mashup}]]></mashupContent></Mashup></Mashups></Entities>"
            )
            .as_bytes(),
        );
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (dir, solution)
    }

    #[test]
    fn a_solution_token_missing_from_default_is_found_once_per_file_and_platform_tokens_are_not_checked(
    ) {
        let (_dir, solution) = solution(
            "{\"Text\":\"[[Acme.App.Cancel]]\",
\"Label\":\"[[Acme.App.Missing]]\",
             \"Other\":\"[[Acme.App.Missing]] [[showMore]]\"}",
        );
        let report = run(&solution);
        assert_eq!(
            (report.checked, report.not_checked, report.files),
            (3, 1, 1)
        );
        assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
        let problem = &report.problems[0];
        assert_eq!(problem.file, "Mashups/Acme.App.View.xml");
        assert_eq!(problem.rule, "missing-token");
        assert_eq!(problem.line, 2);
        assert!(
            problem.message.contains("[[Acme.App.Missing]]"),
            "{}",
            problem.message
        );
    }

    #[test]
    fn untranslated_and_unreferenced_solution_tokens_are_notes_not_findings() {
        // Cancel as a label, Save as the bare name a script passes, and a longer name that holds
        // Open but is not it.
        let (dir, solution) = solution(
            "{\"Text\":\"[[Acme.App.Cancel]]\",\"token\":\"Acme.App.Save\",\"x\":\"Acme.App.OpenAll\"}",
        );
        const GERMAN: &[u8] = include_bytes!(
            "../../../tests/fixtures/localization/flat/Acme.App_LocalizationTable_DE.xml"
        );
        let german = String::from_utf8(GERMAN.to_vec()).unwrap();
        let start = german.find("<![CDATA[Acme.App.Help]]>").unwrap();
        let row = german[..start].rfind("<Row>").unwrap();
        let end = german[start..].find("</Row>").unwrap() + start + "</Row>".len();
        write(
            dir.path(),
            "localization/Acme.App_LocalizationTable_DE.xml",
            format!("{}{}", &german[..row], &german[end..]).as_bytes(),
        );
        // A sidecar script names Open before `sync` writes it into its entity; a longer name
        // holding Close is not Close.
        write(
            dir.path(),
            "src/Acme.App.View/services/Load/script.js",
            b"result = LocalizeToken(\"Acme.App.Open\") + \"Acme.App.Close:Long\";",
        );
        let report = run(&solution);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(
            report.untranslated.values().collect::<Vec<_>>(),
            [&["Acme.App.Help".to_string()]]
        );
        assert_eq!(
            report.unreferenced,
            ["Acme.App.Close", "Acme.App.Delete", "Acme.App.Help"]
        );
    }

    #[test]
    fn two_projects_claiming_one_prefix_are_a_finding() {
        let (_dir, solution) = solution("");
        std::fs::write(
            solution.root.join("twaco.toml"),
            "[[project]]
name = \"Acme.App\"
collections = [\"Mashups\"]
             [project.localization]
prefixes = [\"Acme.\"]

             [[project]]
name = \"Acme.Other\"
[project.localization]
prefixes = [\"Acme.\"]
",
        )
        .unwrap();
        let solution = Solution::load(&solution.root.join("twaco.toml")).unwrap();
        let report = run(&solution);
        assert_eq!(
            report.problems[0].rule, "prefix-clash",
            "{:?}",
            report.problems
        );
        assert!(report.problems[0].message.contains("both claim"));
    }

    #[test]
    fn every_reference_in_default_passes() {
        let (_dir, solution) = solution("[[Acme.App.Save]] [[Acme.App.Cancel]]");
        let report = run(&solution);
        assert_eq!(report.checked, 2);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
    }

    #[test]
    fn a_language_token_default_lacks_and_an_unreadable_table_are_found() {
        let (_dir, solution) = solution("");
        let extra = String::from_utf8(DEFAULT.to_vec())
            .unwrap()
            .replace("name=\"Default\"", "name=\"de\"")
            .replace("Acme.App.Cancel", "Acme.App.OnlyInGerman");
        write(
            &solution.root,
            "localization/Acme.App_LocalizationTable_DE.xml",
            extra.as_bytes(),
        );
        write(
            &solution.root,
            "localization/broken.xml",
            b"<Entities><LocalizationTables><LocalizationTable name=\"x\"/><LocalizationTable name=\"y\"/></LocalizationTables></Entities>",
        );
        let report = run(&solution);
        let rules: Vec<&str> = report.problems.iter().map(|problem| problem.rule).collect();
        assert_eq!(
            rules,
            ["unreadable-table", "not-in-default"],
            "{:?}",
            report.problems
        );
        assert!(report.problems[1].message.contains("Acme.App.OnlyInGerman"));
    }

    #[test]
    fn a_token_is_a_bracketed_name_and_its_line_is_counted() {
        let text = "a [[One]]\nb [[Two.Three]] [[]] [[with space]]\n\n[[[Four]]] [[\"q\"]]";
        let found: Vec<(usize, String)> = bracketed(text);
        let expected = [(1, "One"), (2, "Two.Three"), (4, "Four")]
            .map(|(line, name)| (line, name.to_string()));
        assert_eq!(found, expected);
    }

    #[test]
    fn references_are_read_as_xml_means_them_and_never_in_a_comment() {
        let src = b"<Mashup a=\"[[In.Attribute]]\" b=\"&#91;&#91;En.Coded&#93;&#93;\">\n\
<!-- [[In.Comment]] -->\n<![CDATA[x [[In.Cdata]] &amp; [[Not&amp;Decoded]]]]>\n[[In.Text]]</Mashup>";
        let found: Vec<(usize, String)> = references(src);
        assert_eq!(
            found,
            [
                (1, "In.Attribute".to_string()),
                (1, "En.Coded".to_string()),
                (3, "In.Cdata".to_string()),
                (4, "In.Text".to_string()),
            ]
        );
    }

    #[test]
    fn a_decoded_line_feed_is_no_line_of_the_file() {
        let found = references(
            b"<Mashup
 label=\"&#10;&#10;[[On.Two]]\" other=\"&amp;
[[On.Three]]\"/>
[[On.Four]]",
        );
        assert_eq!(
            found,
            [
                (2, "On.Two".to_string()),
                (3, "On.Three".to_string()),
                (4, "On.Four".to_string())
            ]
        );
    }

    #[test]
    fn a_long_run_of_unterminated_brackets_is_scanned_in_linear_time() {
        let text = "[[a".repeat(200_000);
        let started = std::time::Instant::now();
        assert!(bracketed(&text).is_empty());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }
}
