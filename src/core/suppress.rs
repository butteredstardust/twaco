//! Inline, per-service exceptions to script lint rules.

use super::rules;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) line: usize,
    pub(crate) message: String,
}

#[derive(Debug)]
struct Scope {
    id: String,
    directive_line: usize,
    start: usize,
    end: Option<usize>,
    used: bool,
}

/// Parsed directives. A scope is marked used only when it hides a raw finding.
#[derive(Debug, Default)]
pub(crate) struct Directives {
    scopes: Vec<Scope>,
    malformed: Vec<Problem>,
}

impl Directives {
    pub(crate) fn suppresses(&mut self, id: &str, line: usize) -> bool {
        let mut suppressed = false;
        for scope in &mut self.scopes {
            if scope.id == id && line >= scope.start && scope.end.is_none_or(|end| line <= end) {
                scope.used = true;
                suppressed = true;
            }
        }
        suppressed
    }

    pub(crate) fn malformed(&self) -> &[Problem] {
        &self.malformed
    }

    pub(crate) fn unused(&self, gate_prefix: &str) -> Vec<Problem> {
        self.scopes
            .iter()
            .filter(|scope| !scope.used && scope.id.starts_with(gate_prefix))
            .map(|scope| Problem {
                line: scope.directive_line,
                message: format!("suppression of {} suppresses nothing", scope.id),
            })
            .collect()
    }
}

/// Parse complete-line `twaco-lint` comments. Bad directives do not create scopes.
pub(crate) fn parse(source: &str) -> Directives {
    let lines: Vec<&str> = source.split('\n').collect();
    let mut directives = Directives::default();
    let mut open: Vec<usize> = Vec::new();

    for (index, raw) in lines.iter().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        let Some(comment) = trimmed.strip_prefix("//") else {
            continue;
        };
        let comment = comment.trim_start();
        let Some(rest) = comment.strip_prefix("twaco-lint-") else {
            continue;
        };

        let (kind, payload) = ["disable-next-line", "disable", "enable"]
            .into_iter()
            .find_map(|kind| {
                (rest == kind
                    || rest
                        .strip_prefix(kind)
                        .and_then(|suffix| suffix.chars().next())
                        .is_some_and(char::is_whitespace))
                .then(|| (kind, rest[kind.len()..].trim_start()))
            })
            .unwrap_or_else(|| {
                directives.malformed.push(Problem {
                    line,
                    message: "unknown twaco-lint directive word".to_string(),
                });
                ("", "")
            });
        if kind.is_empty() {
            continue;
        }

        let Some(ids) = ids(payload, kind != "enable", line, &mut directives) else {
            continue;
        };
        if kind == "disable-next-line" {
            let start = lines
                .iter()
                .enumerate()
                .skip(index + 1)
                .find_map(|(next, text)| {
                    let trimmed = text.trim();
                    (!trimmed.is_empty() && !trimmed.starts_with("//")).then_some(next + 1)
                });
            for id in ids {
                directives.scopes.push(Scope {
                    id,
                    directive_line: line,
                    start: start.unwrap_or(lines.len() + 1),
                    end: start,
                    used: false,
                });
            }
        } else if kind == "disable" {
            for id in ids {
                open.push(directives.scopes.len());
                directives.scopes.push(Scope {
                    id,
                    directive_line: line,
                    start: line + 1,
                    end: None,
                    used: false,
                });
            }
        } else {
            for id in ids {
                let matching = open
                    .iter()
                    .rposition(|&scope| directives.scopes[scope].id == id);
                let Some(position) = matching else {
                    directives.malformed.push(Problem {
                        line,
                        message: format!("twaco-lint-enable has no open disable for {id}"),
                    });
                    continue;
                };
                let scope = open.remove(position);
                directives.scopes[scope].end = Some(line.saturating_sub(1));
            }
        }
    }
    directives
}

fn ids(
    payload: &str,
    needs_reason: bool,
    line: usize,
    directives: &mut Directives,
) -> Option<Vec<String>> {
    let (ids, reason) = payload.split_once(':').unwrap_or((payload, ""));
    if ids.trim().is_empty() {
        directives.malformed.push(Problem {
            line,
            message: "twaco-lint directive has an empty id list".to_string(),
        });
        return None;
    }
    if needs_reason && reason.trim().is_empty() {
        directives.malformed.push(Problem {
            line,
            message: "twaco-lint-disable needs a reason after the colon".to_string(),
        });
        return None;
    }
    let ids: Vec<String> = ids.split(',').map(|id| id.trim().to_string()).collect();
    for id in &ids {
        let message = if id.is_empty() {
            "twaco-lint directive has an empty id list".to_string()
        } else if id == "all" {
            "suppress rules by name; `all` is refused".to_string()
        } else if id.starts_with("format.") {
            "formatting cannot be suppressed inline; use [lint.rules] or [[lint.override]]"
                .to_string()
        } else if id.starts_with("comments.") {
            "comment rules cannot be suppressed inline; reword the comment or use [[lint.override]]"
                .to_string()
        } else if matches!(id.as_str(), "traps.suppression" | "order.suppression") {
            format!("{id} cannot be suppressed")
        } else if rules::by_id(id).is_none() {
            format!("twaco-lint directive names no rule {id}")
        } else {
            continue;
        };
        directives.malformed.push(Problem { line, message });
        return None;
    }
    Some(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_line_skips_blank_and_line_comments() {
        let mut directives = parse("// twaco-lint-disable-next-line traps.for-in: legacy\n\n// why\nfor (var key in map) {}\n");
        assert!(directives.suppresses("traps.for-in", 4));
        assert!(!directives.suppresses("traps.for-in", 3));
    }

    #[test]
    fn ranges_close_each_id_independently_or_at_end() {
        let mut directives = parse("//twaco-lint-disable traps.for-in, order.main-code-below-a-helper: legacy\nfirst();\n// twaco-lint-enable traps.for-in\nsecond();\n");
        assert!(directives.suppresses("traps.for-in", 2));
        assert!(!directives.suppresses("traps.for-in", 4));
        assert!(directives.suppresses("order.main-code-below-a-helper", 4));
    }

    #[test]
    fn an_enable_closes_its_other_ids_when_one_has_no_disable() {
        let mut directives = parse(
            "// twaco-lint-disable traps.for-in: legacy
first();
// twaco-lint-enable traps.nan-risk, traps.for-in
second();
",
        );
        assert_eq!(directives.malformed.len(), 1);
        assert!(!directives.suppresses("traps.for-in", 4));
    }

    #[test]
    fn spaces_and_multiple_ids_are_accepted() {
        let mut directives = parse("// twaco-lint-disable-next-line traps.for-in, traps.indexed-rows: legacy\nfor (var key in rows) { rows[i]; }\n");
        assert!(directives.suppresses("traps.for-in", 2));
        assert!(directives.suppresses("traps.indexed-rows", 2));
    }

    #[test]
    fn malformed_directives_are_one_problem_on_the_directive_line() {
        for (source, expected) in [
            ("// twaco-lint-disable nope: reason", "names no rule nope"),
            ("// twaco-lint-disable all: reason", "`all` is refused"),
            (
                "// twaco-lint-disable format.unformatted: reason",
                "formatting cannot",
            ),
            ("// twaco-lint-disable traps.for-in", "needs a reason"),
            ("// twaco-lint-enable traps.for-in", "no open disable"),
            (
                "// twaco-lint-wat traps.for-in: reason",
                "unknown twaco-lint",
            ),
            ("// twaco-lint-disable : reason", "empty id list"),
            (
                "// twaco-lint-disable traps.suppression: reason",
                "cannot be suppressed",
            ),
            (
                "// twaco-lint-disable order.suppression: reason",
                "cannot be suppressed",
            ),
            (
                "// twaco-lint-disable comments.prose.hedge: reason",
                "comment rules cannot be suppressed inline",
            ),
            (
                "// twaco-lint-disable comments.header.contract: reason",
                "comment rules cannot be suppressed inline",
            ),
        ] {
            let directives = parse(source);
            assert_eq!(directives.malformed.len(), 1, "{source}");
            assert_eq!(directives.malformed[0].line, 1, "{source}");
            assert!(
                directives.malformed[0].message.contains(expected),
                "{source}"
            );
        }
    }

    #[test]
    fn unused_scopes_are_grouped_by_owner() {
        let directives = parse("// twaco-lint-disable-next-line traps.for-in, order.main-code-below-a-helper: legacy\nresult = 1;\n");
        assert_eq!(directives.unused("traps.").len(), 1);
        assert_eq!(directives.unused("order.").len(), 1);
    }
}
