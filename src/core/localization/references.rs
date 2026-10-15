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
use crate::core::workspace;
use std::collections::BTreeSet;
use std::path::Path;

pub(crate) const GATE: &str = "localization tokens";

/// One thing the gate found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct TokenProblem {
    pub file: String,
    /// 1-based; zero for a finding about the whole file.
    pub line: usize,
    pub rule: &'static str,
    pub message: String,
}

#[derive(Debug, Default)]
pub(crate) struct Report {
    /// References to the solution's own tokens that were checked.
    pub checked: usize,
    /// Entity files that hold at least one `[[token]]`.
    pub files: usize,
    /// References to tokens outside every project's prefixes.
    pub not_checked: usize,
    pub problems: Vec<TokenProblem>,
}

pub(crate) fn run(solution: &Solution) -> Report {
    let mut report = Report::default();
    let shown = |path: &Path| {
        path.strip_prefix(&solution.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
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
            // A missing translation shows the Default text: normal while a language is filled in.
            Problem::Untranslated { .. } => {}
        }
    }
    let defined: BTreeSet<&str> = found
        .files
        .iter()
        .filter(|file| file.table == DEFAULT_TABLE)
        .flat_map(|file| file.tokens.iter().map(|token| token.name.as_str()))
        .collect();
    for file in &workspace::discover(solution).entities {
        let Ok(bytes) = std::fs::read(&file.path) else {
            // The project gate reports a file it cannot read.
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut any = false;
        let mut reported: BTreeSet<&str> = BTreeSet::new();
        for (line, name) in tokens(&text) {
            any = true;
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
        if any {
            report.files += 1;
        }
    }
    report
}

/// Every `[[name]]` in `text`, with its 1-based line. A name is what ThingWorx resolves: no
/// brackets, quotes, spaces or markup inside, and not empty.
fn tokens(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut line = 1;
    let mut rest = text;
    let mut consumed_lines_to = 0;
    let mut offset = 0;
    while let Some(start) = rest.find("[[") {
        let name_start = start + 2;
        let candidate = &rest[name_start..];
        let end = candidate.find("]]");
        let advance = match end {
            Some(end)
                if end > 0
                    && !candidate[..end].contains(|c: char| {
                        c.is_whitespace() || matches!(c, '[' | ']' | '"' | '\'' | '<' | '>' | '&')
                    }) =>
            {
                let at = offset + start;
                line += text[consumed_lines_to..at].matches('\n').count();
                consumed_lines_to = at;
                out.push((line, &candidate[..end]));
                name_start + end + 2
            }
            _ => start + 1,
        };
        offset += advance;
        rest = &rest[advance..];
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
        assert_eq!(tokens(text), [(1, "One"), (2, "Two.Three"), (4, "Four")]);
    }
}
