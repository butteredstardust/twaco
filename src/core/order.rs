//! Where the main code sits relative to its helpers.
//!
//! A ThingWorx service script has no entry point: it runs top to bottom. So the main code
//! belongs at the top, where a reader meets it first, and the helpers below it — function
//! declarations hoist, so a helper can sit anywhere.
//!
//! The rule is not taste. A declaration moved below the main code stops hoisting its
//! *initialiser*: a service can read a `const` whose value is assigned further down and return
//! `undefined` without failing.

/// What a top-level line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Function,
    Declaration,
    Statement,
    /// Nested, blank, or a comment: not the rule's business.
    Skip,
}

/// One thing in the wrong place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Misplaced {
    pub line: usize,
    pub rule: &'static str,
    pub message: String,
}

/// What appears below the first top-level function that should be above it.
pub fn check(text: &str) -> Vec<Misplaced> {
    let classified = classify(text);
    let Some(first) = classified.iter().find(|(_, kind)| *kind == Kind::Function).map(|(n, _)| *n)
    else {
        return Vec::new();
    };

    classified
        .iter()
        .filter(|(number, _)| *number > first)
        .filter_map(|(number, kind)| match kind {
            Kind::Statement => Some(Misplaced {
                line: *number,
                rule: "main-code-below-a-helper",
                message: format!(
                    "main code below the first function (line {first}); a service runs top to \
                     bottom, so this reads out of order"
                ),
            }),
            Kind::Declaration => Some(Misplaced {
                line: *number,
                rule: "declaration-between-helpers",
                message: "a declaration between functions belongs above the main code; its \
                          initialiser does not hoist, so anything above reads undefined"
                    .to_string(),
            }),
            _ => None,
        })
        .collect()
}

/// Label every line by what it is at the top level.
///
/// Brace depth is counted with string and comment contents removed, so a brace inside a message
/// does not open a block.
fn classify(text: &str) -> Vec<(usize, Kind)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_block_comment = false;

    for (index, raw) in text.lines().enumerate() {
        let stripped = raw.trim();
        let mut kind = Kind::Skip;
        let mut counts_toward_depth = true;

        if in_block_comment {
            // A brace inside a block comment is prose. Counting it left the classifier inside a
            // block for the rest of the file, and the rule then reported nothing at all.
            counts_toward_depth = false;
            if stripped.contains("*/") {
                in_block_comment = false;
            }
        } else if stripped.starts_with("/*") {
            counts_toward_depth = false;
            if !stripped.contains("*/") {
                in_block_comment = true;
            }
        } else if depth == 0 && !stripped.is_empty() && !stripped.starts_with("//") {
            kind = if is_function(stripped) {
                Kind::Function
            } else if is_declaration(stripped) {
                Kind::Declaration
            } else {
                Kind::Statement
            };
        }
        out.push((index + 1, kind));

        if counts_toward_depth {
            let code = strip_literals(raw);
            depth += code.matches('{').count() as i32 - code.matches('}').count() as i32;
            depth = depth.max(0);
        }
    }
    out
}

fn is_function(stripped: &str) -> bool {
    let Some(rest) = stripped.strip_prefix("function ") else { return false };
    let name: String =
        rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect();
    !name.is_empty() && rest.trim_start()[name.len()..].trim_start().starts_with('(')
}

fn is_declaration(stripped: &str) -> bool {
    ["const ", "let ", "var "].iter().any(|k| stripped.starts_with(k))
}

/// Remove string literals and trailing comments, so their braces are not counted.
fn strip_literals(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Some(open) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == open {
                    quote = None;
                }
            }
            None => {
                if c == '"' || c == '\'' || c == '`' {
                    quote = Some(c);
                } else if c == '/' && chars.peek() == Some(&'/') {
                    break;
                } else {
                    out.push(c);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(source: &str) -> Vec<&'static str> {
        check(source).into_iter().map(|m| m.rule).collect()
    }

    #[test]
    fn main_code_above_its_helpers_is_correct() {
        let source = "const a = helper();\nresult = a;\nfunction helper() {\n    return 1;\n}\n";
        assert!(rules(source).is_empty(), "got {:?}", check(source));
    }

    #[test]
    fn a_statement_below_a_helper_is_reported() {
        let source = "function helper() {\n    return 1;\n}\nresult = helper();\n";
        assert_eq!(rules(source), vec!["main-code-below-a-helper"]);
    }

    #[test]
    fn a_declaration_between_helpers_is_reported() {
        let source = "function a() {\n    return 1;\n}\nconst mid = 2;\nfunction b() {\n    return mid;\n}\n";
        assert_eq!(rules(source), vec!["declaration-between-helpers"]);
    }

    #[test]
    fn a_file_with_no_helpers_has_nothing_to_order() {
        assert!(rules("const a = 1;\nresult = a;\n").is_empty());
    }

    #[test]
    fn a_brace_inside_a_string_does_not_open_a_block() {
        // Without stripping literals, this brace leaves the classifier inside a block forever
        // and the rule silently stops reporting anything.
        let source = "function a() {\n    logger.warn(\"{} - oops\", me.name);\n}\nresult = a();\n";
        assert_eq!(rules(source), vec!["main-code-below-a-helper"]);
    }

    #[test]
    fn a_comment_below_a_helper_is_not_main_code() {
        let source = "function a() {\n    return 1;\n}\n// just explaining\n/* and a block\n   comment */\n";
        assert!(rules(source).is_empty(), "got {:?}", check(source));
    }

    #[test]
    fn a_jsdoc_block_above_a_helper_is_skipped() {
        let source = "result = 1;\n/**\n * Does a thing.\n */\nfunction a() {\n    return 1;\n}\n";
        assert!(rules(source).is_empty(), "got {:?}", check(source));
    }

    #[test]
    fn a_brace_inside_a_block_comment_is_prose() {
        // Counting it left the classifier inside a block forever, and the rule silently stopped
        // reporting anything at all for the rest of the file.
        let source = "function f() {\n    return 1;\n}\n/* { */\nresult = f();\n";
        assert_eq!(rules(source), vec!["main-code-below-a-helper"]);
    }

    #[test]
    fn a_misplaced_line_reports_where_it_is_and_what_it_is_below() {
        let found = check("function a() {\n    return 1;\n}\nresult = a();\n");
        assert_eq!(found[0].line, 4);
        assert!(found[0].message.contains("line 1"), "got {}", found[0].message);
    }
}
