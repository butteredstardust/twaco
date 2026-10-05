//! Traps in the ThingWorx script engine that a formatter and a parser both let through.
//!
//! These are not style rules. Each one is a thing that compiles, imports, and then behaves
//! differently from how it reads — silently, at runtime, usually in a way that looks like a data
//! problem rather than a code one. Every rule here was written after one of them cost a day.
//!
//! The analysis is line-based rather than syntactic, and deliberately so: every sidecar goes
//! through the formatter before it is synced, so indentation is reliable enough to stand in for
//! brace depth, and a rule that reads like the code it is about is easier to trust than an AST
//! walk nobody revisits.

use std::collections::{BTreeMap, BTreeSet};

/// One trap, in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trap {
    pub line: usize,
    pub rule: &'static str,
    pub message: String,
    /// The offending line, trimmed, so a report can show it without re-reading the file.
    pub source: String,
}

/// Every trap in one service script.
pub fn lint(text: &str) -> Vec<Trap> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut traps = Vec::new();
    traps.extend(const_in_loop(&lines));
    traps.extend(redeclared(&lines));
    traps.extend(per_line_rules(&lines, text));
    traps.extend(read_before_assign(&lines));
    traps.sort_by_key(|t| (t.line, t.rule));
    traps
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_comment(stripped: &str) -> bool {
    stripped.starts_with("//") || stripped.starts_with('*') || stripped.starts_with("/*")
}

/// `const` declared inside a loop body.
///
/// Every iteration after the first silently keeps the first value. The loop body is the run of
/// lines indented past the `for` or `while` that opened it; a callback resets that, because a
/// function body is its own scope and a `const` there is correct.
fn const_in_loop(lines: &[&str]) -> Vec<Trap> {
    let mut traps = Vec::new();
    let mut loop_indent: Option<usize> = None;
    for (index, line) in lines.iter().enumerate() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with("//") {
            continue;
        }
        if loop_indent.is_some_and(|open| indent_of(line) <= open) {
            loop_indent = None;
        }
        if starts_loop(stripped) {
            // A one-line loop has no body to trip over.
            loop_indent = if stripped.ends_with('}') {
                None
            } else {
                Some(indent_of(line))
            };
            continue;
        }
        if loop_indent.is_some() {
            if starts_function(stripped) || stripped.contains("=>") {
                loop_indent = None;
                continue;
            }
            if let Some(name) = declared(stripped, &["const"], false) {
                traps.push(Trap {
                    line: index + 1,
                    rule: "const-in-loop",
                    message: format!(
                        "`{name}` is declared const inside a loop body; every iteration after \
                         the first silently keeps the first value. Declare it with let outside \
                         the loop and assign inside."
                    ),
                    source: stripped.to_string(),
                });
            }
        }
    }
    traps
}

/// The same name declared twice in one function.
///
/// `const` and `let` are function-scoped in this engine, so two sibling blocks collide. The
/// whole service then fails to compile and callers see "No service handler defined", which
/// names neither the service nor the line.
fn redeclared(lines: &[&str]) -> Vec<Trap> {
    let mut traps = Vec::new();
    let mut scope: BTreeMap<String, usize> = BTreeMap::new();
    let mut scope_indent = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with("//") {
            continue;
        }
        if starts_function(stripped) {
            scope.clear();
            scope_indent = indent_of(line);
            continue;
        }
        if indent_of(line) <= scope_indent && stripped.starts_with('}') {
            scope.clear();
        }
        if let Some(name) = declared_name(stripped, &["const", "let"]) {
            match scope.get(&name) {
                Some(first) => traps.push(Trap {
                    line: index + 1,
                    rule: "const-redeclared",
                    message: format!(
                        "`{name}` is already declared at line {first} in this function. \
                         const and let are function-scoped here, so this fails the whole \
                         service to compile and callers see \"No service handler defined\"."
                    ),
                    source: stripped.to_string(),
                }),
                None => {
                    scope.insert(name, index + 1);
                }
            }
        }
    }
    traps
}

/// The rules that look at one line at a time.
fn per_line_rules(lines: &[&str], whole: &str) -> Vec<Trap> {
    let writes_rows = whole.contains("AddRow");
    let mut traps = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let stripped = line.trim();
        if stripped.starts_with("//") {
            continue;
        }
        let number = index + 1;

        if is_for_in(stripped) {
            traps.push(Trap {
                line: number,
                rule: "for-in",
                message: "for..in is a syntax error in the ThingWorx script engine; use \
                          Object.keys(x).forEach(...)."
                    .to_string(),
                source: stripped.to_string(),
            });
        }
        if indexes_rows(stripped) {
            traps.push(Trap {
                line: number,
                rule: "indexed-rows",
                message: "indexing into an InfoTable's rows.toArray() re-reads the first row on \
                          this platform, so every iteration sees row 0. Use .forEach(...) instead."
                    .to_string(),
                source: stripped.to_string(),
            });
        }
        if stripped.starts_with("while")
            && stripped[5..].trim_start().starts_with('(')
            && !stripped.contains("false")
        {
            traps.push(Trap {
                line: number,
                rule: "unbounded-loop",
                message: "a while loop in a service can pin a platform thread at 100% CPU if its \
                          body ever fails to advance. Use a for loop with a constant iteration \
                          cap alongside the condition."
                    .to_string(),
                source: stripped.to_string(),
            });
        }
        // Only where the result goes straight into a row, which is where a NaN takes the
        // service down rather than merely being wrong.
        if writes_rows {
            // Every one on the line: `a: Number(x + 1), b: Number(y)` has a real finding after
            // an expression that is not one.
            for argument in numbers_into_rows(stripped) {
                traps.push(Trap {
                    line: number,
                    rule: "nan-risk",
                    message: format!(
                        "Number({argument}) is written straight into a row; if it is ever absent \
                         this is NaN, which cannot be serialised and 500s the whole service."
                    ),
                    source: stripped.to_string(),
                });
            }
        }
    }
    traps
}

/// A top-level statement that reads a name only a later statement fills in.
///
/// Function declarations hoist, so a helper may sit below its caller. A declaration's
/// *initialiser* does not: `const index = requiredIndex(action)` reads `action` when that line
/// runs, whatever a later `action = ...` intends.
fn read_before_assign(lines: &[&str]) -> Vec<Trap> {
    let groups = top_level_statements(lines);
    let readers = helper_readers(lines);

    // A helper's body runs when it is called, not where it sits, so an assignment inside one
    // says nothing about order. Only the main code can be the statement that comes too late.
    let main: Vec<&(usize, Vec<String>)> = groups
        .iter()
        .filter(|(_, body)| top_function_name(&body[0]).is_none())
        .collect();

    let mut declared: BTreeMap<String, usize> = BTreeMap::new();
    for (number, body) in &main {
        if let Some(name) = empty_declaration(body[0].trim()) {
            declared.entry(name).or_insert(*number);
        }
    }

    let mut traps = Vec::new();
    for (index, (number, body)) in main.iter().enumerate() {
        let stripped: Vec<String> = body.iter().map(|l| strip_comment(l)).collect();
        // Comments are removed before anything is matched. A prose mention of a variable is not
        // a read of it, and a statement group picks up the comment lines that follow it.
        let text = stripped.join("\n");
        let called: Vec<&String> = readers.keys().filter(|name| calls(&text, name)).collect();

        for (name, declared_at) in &declared {
            if declared_at >= number || declares_same(&body[0], name) {
                continue;
            }
            if assigns(name, &stripped) {
                continue;
            }
            let direct = mentions(&text, name);
            let through: Vec<&str> = called
                .iter()
                .filter(|helper| readers[**helper].contains(name))
                .map(|h| h.as_str())
                .collect();
            if !direct && through.is_empty() {
                continue;
            }
            let Some(later) = main[index + 1..]
                .iter()
                .find(|(_, other)| {
                    assigns(
                        name,
                        &other
                            .iter()
                            .map(|l| strip_comment(l))
                            .collect::<Vec<String>>(),
                    )
                })
                .map(|(n, _)| *n)
            else {
                continue;
            };
            // A variable filled further up is settled: a later `payload.rowCount = ...` is not
            // the assignment this read was waiting for.
            let filled_above = main[..index].iter().any(|(n, other)| {
                n > declared_at
                    && assigns(
                        name,
                        &other
                            .iter()
                            .map(|l| strip_comment(l))
                            .collect::<Vec<String>>(),
                    )
            });
            if filled_above {
                continue;
            }
            let how = if direct {
                "directly".to_string()
            } else {
                format!("through {}()", through[0])
            };
            traps.push(Trap {
                line: *number,
                rule: "read-before-assign",
                message: format!(
                    "`{name}` is read here {how}, but nothing assigns it until line {later}. \
                     Declarations hoist and their initialisers do not, so this reads undefined. \
                     Move this statement below the assignment."
                ),
                source: body[0].trim().to_string(),
            });
        }
    }
    traps
}

/// Group the file into top-level statements: an unindented line plus the lines under it.
fn top_level_statements(lines: &[&str]) -> Vec<(usize, Vec<String>)> {
    let mut groups: Vec<(usize, Vec<String>)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        if indent_of(line) == 0 && !is_comment(stripped) {
            groups.push((index + 1, vec![line.to_string()]));
        } else if let Some(last) = groups.last_mut() {
            last.1.push(line.to_string());
        }
    }
    groups
}

/// Each top-level helper mapped to the outer names it reads, ignoring its own parameters.
fn helper_readers(lines: &[&str]) -> BTreeMap<String, BTreeSet<String>> {
    let mut readers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut shadowed: BTreeSet<String> = BTreeSet::new();

    for line in lines {
        if let Some((name, parameters)) = top_function(line) {
            current = Some(name.clone());
            shadowed = parameters;
            readers.entry(name).or_default();
            continue;
        }
        let Some(name) = current.clone() else {
            continue;
        };
        let stripped = line.trim();
        if indent_of(line) == 0
            && !stripped.is_empty()
            && !stripped.starts_with('}')
            && !is_comment(stripped)
        {
            current = None;
            continue;
        }
        for word in identifiers(&strip_comment(line)) {
            if !shadowed.contains(&word) {
                readers.entry(name.clone()).or_default().insert(word);
            }
        }
    }
    readers
}

/// Whether a statement gives `name` a value, by assignment or by mutating what it holds.
fn assigns(name: &str, body: &[String]) -> bool {
    body.iter().any(|line| assigns_in_line(name, line))
}

fn assigns_in_line(name: &str, line: &str) -> bool {
    let mut from = 0usize;
    while let Some(at) = line[from..].find(name) {
        let start = from + at;
        let end = start + name.len();
        from = end;
        if !is_word_boundary(line, start, end) {
            continue;
        }
        let rest = line[end..].trim_start();
        // `name = `, `name.x = `, `name[i] = `, but never `name ==`.
        let after_access = match rest.strip_prefix('.') {
            Some(tail) => tail
                .trim_start_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '$')
                .trim_start(),
            None => match rest.strip_prefix('[') {
                Some(tail) => tail
                    .split_once(']')
                    .map(|(_, t)| t.trim_start())
                    .unwrap_or(""),
                None => rest,
            },
        };
        if after_access.starts_with('=') && !after_access.starts_with("==") {
            return true;
        }
        // A mutating method call counts as filling it.
        for method in ["push", "unshift", "splice", "pop", "shift", "set", "add"] {
            if rest.starts_with(&format!(".{method}(")) {
                return true;
            }
        }
    }
    false
}

fn is_word_boundary(line: &str, start: usize, end: usize) -> bool {
    let before = line[..start].chars().next_back();
    let after = line[end..].chars().next();
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$');
    !is_word(before) && !is_word(after)
}

fn mentions(text: &str, name: &str) -> bool {
    identifiers(text).contains(&name.to_string())
}

fn calls(text: &str, name: &str) -> bool {
    let mut from = 0usize;
    while let Some(at) = text[from..].find(name) {
        let start = from + at;
        let end = start + name.len();
        from = end;
        if is_word_boundary(text, start, end) && text[end..].trim_start().starts_with('(') {
            return true;
        }
    }
    false
}

fn identifiers(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' || c == '$' {
            current.push(c);
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn strip_comment(line: &str) -> String {
    match line.find("//") {
        Some(at) => line[..at].to_string(),
        None => line.to_string(),
    }
}

// --- small syntactic predicates, kept together so the rules above read as rules --------------

fn starts_loop(stripped: &str) -> bool {
    for keyword in ["for", "while"] {
        if let Some(rest) = stripped.strip_prefix(keyword) {
            if rest.trim_start().starts_with('(') {
                return true;
            }
        }
    }
    false
}

fn starts_function(stripped: &str) -> bool {
    if let Some(rest) = stripped.strip_prefix("function") {
        return rest.starts_with(' ') || rest.starts_with('(');
    }
    for keyword in ["const ", "let ", "var "] {
        if let Some(rest) = stripped.strip_prefix(keyword) {
            if let Some((_, tail)) = rest.split_once('=') {
                if tail.trim_start().starts_with("function") {
                    return true;
                }
            }
        }
    }
    false
}

/// The name a `const`/`let`/`var` line declares, when it declares one.
fn declared_name(stripped: &str, keywords: &[&str]) -> Option<String> {
    declared(stripped, keywords, true)
}

/// As `declared_name`, but `require_value` says whether a bare `let x;` counts.
///
/// `const x;` is not a declaration this rule is about: it has no initialiser to be trapped by,
/// and the supported declaration pattern requires the `=`.
fn declared(stripped: &str, keywords: &[&str], allow_bare: bool) -> Option<String> {
    for keyword in keywords {
        let Some(rest) = stripped.strip_prefix(keyword) else {
            continue;
        };
        // Any whitespace, not just a space: a tab-indented script is still a declaration.
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        // A name starts with a letter, underscore or dollar, as JavaScript requires.
        if !rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '$') {
            continue;
        }
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        let after = rest[name.len()..].trim_start();
        if after.starts_with('=') || (allow_bare && after.starts_with(';')) {
            return Some(name);
        }
    }
    None
}

/// A declaration with no useful initialiser: `let x;`, `const x = {};`, `let x = [];`.
fn empty_declaration(stripped: &str) -> Option<String> {
    let name = declared_name(stripped, &["const", "let", "var"])?;
    let after = stripped.split_once(&name)?.1.trim_start();
    if after.starts_with(';') {
        return Some(name);
    }
    // The semicolon is required: `const x = {}` continuing onto the next line is a literal
    // being built, not an empty declaration waiting to be filled.
    let value = after.strip_prefix('=')?.trim();
    let value = value.strip_suffix(';')?.trim();
    if value == "{}" || value == "[]" {
        return Some(name);
    }
    None
}

fn declares_same(line: &str, name: &str) -> bool {
    declared_name(line.trim(), &["const", "let", "var"]).is_some_and(|n| n == name)
}

fn top_function_name(line: &str) -> Option<String> {
    top_function(line).map(|(name, _)| name)
}

/// A top-level `function name(a, b)` declaration: its name and its parameters.
fn top_function(line: &str) -> Option<(String, BTreeSet<String>)> {
    if indent_of(line) != 0 {
        return None;
    }
    let rest = line.strip_prefix("function ")?;
    let (name, rest) = rest.split_once('(')?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let parameters = rest
        .split_once(')')
        .map(|(inside, _)| {
            inside
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Some((name, parameters))
}

fn is_for_in(stripped: &str) -> bool {
    // Every `for` on the line: `if (before) for (let k in obj)` has one that is not the first
    // occurrence of the word, and looking only at the first missed it entirely.
    let mut from = 0usize;
    while let Some(at) = stripped[from..].find("for") {
        let start = from + at;
        from = start + 3;
        let rest = stripped[start + 3..].trim_start();
        let Some(inside) = rest.strip_prefix('(') else {
            continue;
        };
        let inside = inside.trim_start();
        for keyword in ["const", "let", "var"] {
            let Some(tail) = inside.strip_prefix(keyword) else {
                continue;
            };
            if !tail.starts_with(char::is_whitespace) {
                continue;
            }
            let after_name = tail
                .trim_start()
                .trim_start_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '$');
            let after_name = after_name.trim_start();
            if let Some(operand) = after_name.strip_prefix("in") {
                if operand.starts_with(char::is_whitespace) {
                    return true;
                }
            }
        }
    }
    false
}

/// `rows[i]` and friends: an indexed read of something whose name ends in `rows`.
fn indexes_rows(stripped: &str) -> bool {
    let mut from = 0usize;
    while let Some(at) = stripped[from..].find('[') {
        let open = from + at;
        from = open + 1;
        // Whitespace between the name and the bracket is still an indexed read.
        let head = stripped[..open].trim_end();
        let before: String = head
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        // `rows` or `Rows`, not `ROWS`: the reference matches the suffix case-sensitively, and
        // an all-caps constant is not an InfoTable.
        if !before.ends_with("rows") && !before.ends_with("Rows") {
            continue;
        }
        let Some((index, _)) = stripped[open + 1..].split_once(']') else {
            continue;
        };
        if matches!(index.trim(), "i" | "j" | "index" | "idx") {
            return true;
        }
    }
    false
}

/// `field: Number(x)` written straight into an object literal, where `x` is a bare path.
///
/// Only a bare path — `row.value`, `data[0]`, `count` — because that is the shape that can be
/// absent and become NaN. `Number(1 + f(x))` is an expression whose author already knows what
/// is in it, and flagging those buried the real findings in noise: 32 of them in the reference
/// project, every one a generated demo value.
fn numbers_into_rows(stripped: &str) -> Vec<String> {
    // The line must *start* with `key:`. That anchor is what confines the rule to a property on
    // its own line, which is what a row literal looks like once the formatter has been through
    // it. Without it, `manager.DeleteCard({ uid: Number(card.uid) })` reads as a row and is not.
    let Some(colon) = stripped.find(':') else {
        return Vec::new();
    };
    let key = stripped[..colon].trim_end();
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    {
        return Vec::new();
    }
    // And `Number(` must be what the property is assigned, not something further along.
    if !stripped[colon + 1..].trim_start().starts_with("Number(") {
        return Vec::new();
    }

    // Then the first `Number(path)` anywhere on the line, a bare path only: an expression is
    // something whose author already knows what is in it.
    let mut from = 0usize;
    while let Some(at) = stripped[from..].find("Number(") {
        let start = from + at;
        from = start + 7;
        // A word boundary, so `optionalFiniteNumber(x)` is not a coercion of x.
        if stripped[..start].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$') {
            continue;
        }
        let argument = stripped[start + 7..].trim_start();
        let path: String = argument
            .chars()
            .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '.' | '[' | ']'))
            .collect();
        if !path.is_empty() && argument[path.len()..].trim_start().starts_with(')') {
            return vec![path];
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(source: &str) -> Vec<&'static str> {
        lint(source).into_iter().map(|t| t.rule).collect()
    }

    #[test]
    fn a_const_inside_a_loop_is_reported() {
        let source = "for (var i = 0; i < 3; i++) {\n    const x = i;\n    use(x);\n}\n";
        assert_eq!(rules(source), vec!["const-in-loop"]);
    }

    #[test]
    fn a_const_inside_a_callback_is_fine() {
        // A function body is its own scope, so the const is correct there.
        let source = "rows.forEach(function (row) {\n    const x = row.a;\n    use(x);\n});\n";
        assert!(rules(source).is_empty(), "got {:?}", lint(source));
    }

    #[test]
    fn a_one_line_loop_has_no_body_to_trip_over() {
        assert!(rules("for (var i = 0; i < 3; i++) { use(i); }\n").is_empty());
    }

    #[test]
    fn the_same_name_declared_in_two_sibling_blocks_is_reported() {
        // Inside a function, where the scope really is shared. A closing brace at the scope's
        // own indentation ends it, so two top-level blocks are treated as separate -- which is
        // what the reference does, and what the corpus was linted under.
        let source = "function f() {\n    if (a) {\n        const x = 1;\n    }\n    if (b) {\n        const x = 2;\n    }\n}\n";
        assert_eq!(rules(source), vec!["const-redeclared"]);
    }

    #[test]
    fn the_same_name_in_two_functions_is_fine() {
        let source = "function a() {\n    const x = 1;\n}\nfunction b() {\n    const x = 2;\n}\n";
        assert!(rules(source).is_empty(), "got {:?}", lint(source));
    }

    #[test]
    fn for_in_is_reported() {
        assert_eq!(
            rules("for (const key in map) {\n    use(key);\n}\n"),
            vec!["for-in"]
        );
        // for..of is fine.
        assert!(rules("for (const item of list) {\n    use(item);\n}\n").is_empty());
    }

    #[test]
    fn indexing_into_rows_is_reported() {
        assert_eq!(rules("var r = result.rows[i];\n"), vec!["indexed-rows"]);
        assert!(
            rules("var r = rows[0];\n").is_empty(),
            "a constant index is not the trap"
        );
    }

    #[test]
    fn a_while_loop_is_reported_unless_it_is_the_idiom() {
        assert_eq!(
            rules("while (more()) {\n    step();\n}\n"),
            vec!["unbounded-loop"]
        );
        assert!(rules("while (false) {\n    never();\n}\n").is_empty());
    }

    #[test]
    fn a_number_written_into_a_row_is_reported() {
        let source = "out.AddRow({\n    Value: Number(row.value)\n});\n";
        assert_eq!(rules(source), vec!["nan-risk"]);
    }

    #[test]
    fn a_number_outside_a_row_is_not_reported() {
        assert!(rules("var v = Number(row.value);\n").is_empty());
    }

    #[test]
    fn a_statement_reading_a_name_assigned_later_is_reported() {
        let source = "let action;\nconst index = lookup(action);\naction = \"go\";\n";
        assert_eq!(rules(source), vec!["read-before-assign"]);
    }

    #[test]
    fn a_name_filled_above_is_settled() {
        // Regression: every statement below the first property write reported the one below it.
        let source = "var payload = {};\n\
                      payload = build();\n\
                      use(payload);\n\
                      payload.rowCount = 0;\n";
        assert!(rules(source).is_empty(), "got {:?}", lint(source));
    }

    #[test]
    fn a_helper_below_its_caller_is_fine_because_declarations_hoist() {
        let source = "const value = helper();\nfunction helper() {\n    return 1;\n}\n";
        assert!(rules(source).is_empty(), "got {:?}", lint(source));
    }

    #[test]
    fn a_tab_indented_declaration_is_still_a_declaration() {
        let source = "for (var i = 0; i < 3; i++) {\n\tconst\tx = i;\n\tuse(x);\n}\n";
        assert_eq!(rules(source), vec!["const-in-loop"]);
    }

    #[test]
    fn a_name_starting_with_a_digit_is_not_a_declaration() {
        assert!(rules("for (var i = 0; i < 3; i++) {\n    const 1x = i;\n}\n").is_empty());
    }

    #[test]
    fn a_for_in_that_is_not_first_on_the_line_is_found() {
        assert_eq!(
            rules("if (before) for (let k in obj) { use(k); }\n"),
            vec!["for-in"]
        );
    }

    #[test]
    fn a_second_number_on_a_property_line_is_still_checked() {
        // The line must be a property of its own, which is what a row literal looks like once
        // the formatter has been through it. Then any bare path on it counts.
        let source = "out.AddRow({\n    a: Number(x + 1), b: Number(row.value)\n});\n";
        let traps = lint(source);
        assert_eq!(traps.len(), 1, "got {traps:?}");
        assert!(
            traps[0].message.contains("Number(row.value)"),
            "got {}",
            traps[0].message
        );
    }

    #[test]
    fn a_helper_whose_name_ends_in_number_is_not_a_coercion() {
        // `optionalFiniteNumber(x)` contains `Number(`, but the helper call is not a coercion.
        let source = "out.AddRow({\n    width: optionalFiniteNumber(row.CardWidth)\n});\n";
        assert!(rules(source).is_empty(), "got {:?}", lint(source));
    }

    #[test]
    fn a_number_in_a_call_argument_is_not_a_row() {
        // A service call is not a row literal, whatever it looks like mid-line.
        let source = "out.AddRow({});\nmanager.DeleteCard({ uid: Number(card.uid) });\n";
        assert!(rules(source).is_empty(), "got {:?}", lint(source));
    }

    #[test]
    fn an_all_caps_constant_is_not_an_infotable() {
        assert!(rules("var r = ROWS[i];\n").is_empty());
        assert_eq!(rules("var r = myRows [i];\n"), vec!["indexed-rows"]);
    }

    #[test]
    fn a_trap_carries_its_line_and_the_offending_source() {
        let traps = lint("for (var i = 0; i < 3; i++) {\n    const x = i;\n}\n");
        assert_eq!(traps[0].line, 2);
        assert_eq!(traps[0].source, "const x = i;");
    }
}
