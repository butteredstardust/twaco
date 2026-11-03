//! Comment measurement and the deliberately small house prose checks.

use regex::Regex;
use std::collections::BTreeSet;

const IMPERATIVES: &[&str] = &[
    "use", "call", "pass", "keep", "do", "avoid", "prefer", "set", "add", "remove", "check",
    "read", "write", "return", "declare", "treat", "note", "delete", "split", "cap", "put",
    "leave", "make", "give", "take", "run", "start", "stop", "fail", "throw", "log", "store",
];
const HEDGES: &[&str] = &[
    "it is important to note",
    "it should be noted",
    "it is worth noting",
    "note that",
    "basically",
    "essentially",
    "actually",
    "obviously",
    "of course",
    "simply",
    "just",
    "robust",
    "very",
    "quite",
    "really",
    "arguably",
    "somewhat",
    "fairly",
    "pretty much",
    "tends to",
    "in order to",
    "for what it is worth",
    "needless to say",
];
const HISTORY: &[&str] = &[
    "used to",
    "previously",
    "no longer",
    "formerly",
    "originally",
    "in the past",
    "legacy",
    "back then",
    "historically",
    "was renamed",
    "was dropped",
    "we used",
    "old behaviour",
    "old behavior",
    "before the migration",
    "since the migration",
    "used be",
    "at one point",
];
const SYNONYMS: &[&[&str]] = &[
    &["fetch", "retrieve", "download"],
    &["invoke", "execute"],
    &["delete", "erase"],
    &["build", "construct"],
    &["pick", "choose", "select"],
];
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "if", "so", "than", "then", "that", "this", "these",
    "those", "is", "are", "was", "were", "be", "been", "being", "it", "its", "as", "at", "by",
    "for", "from", "in", "into", "of", "on", "to", "with", "not", "no", "never", "always", "which",
    "what", "when", "where", "who", "whom", "because", "since", "while", "each", "every", "any",
    "all", "both", "one", "two", "three", "more", "most", "less", "least", "own", "same", "other",
    "another", "such", "only", "up", "down", "out", "over", "under", "has", "have", "had", "does",
    "did", "can", "may", "must", "will", "would", "should",
];

#[derive(Debug, Clone)]
pub(crate) struct CommentPolicy {
    pub(crate) jsdoc_max_lines: usize,
    pub(crate) line_run_max_lines: usize,
    pub(crate) ratio_max: f64,
    pub(crate) ratio_min_code_lines: usize,
    pub(crate) instruction_max_words: usize,
    pub(crate) description_max_words: usize,
    pub(crate) imperatives: Vec<String>,
    pub(crate) history: Vec<String>,
    pub(crate) hedges: Vec<String>,
    pub(crate) synonyms: Vec<Vec<String>>,
    pub(crate) noun_max_run: usize,
    compiled: Compiled,
}

impl Default for CommentPolicy {
    fn default() -> Self {
        let mut policy = Self::uncompiled();
        policy.compile();
        policy
    }
}

impl CommentPolicy {
    pub(crate) fn uncompiled() -> Self {
        Self {
            jsdoc_max_lines: 2,
            line_run_max_lines: 2,
            ratio_max: 0.40,
            ratio_min_code_lines: 15,
            instruction_max_words: 20,
            description_max_words: 25,
            imperatives: IMPERATIVES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            history: HISTORY.iter().map(|value| (*value).to_string()).collect(),
            hedges: HEDGES.iter().map(|value| (*value).to_string()).collect(),
            synonyms: SYNONYMS
                .iter()
                .map(|set| set.iter().map(|value| (*value).to_string()).collect())
                .collect(),
            noun_max_run: 3,
            compiled: Compiled::empty(),
        }
    }
    pub(crate) fn compile(&mut self) {
        self.compiled = Compiled::new(self);
    }
}

#[derive(Debug, Clone)]
struct Prose {
    line: usize,
    text: String,
    starts_tag: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Measure {
    code_lines: usize,
    comment_lines: usize,
    jsdoc_prose: usize,
    header_line: usize,
    inline_runs: Vec<(usize, usize)>,
    prose: Vec<Prose>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommentFinding {
    pub(crate) rule: &'static str,
    pub(crate) line: usize,
    pub(crate) message: String,
}

/// Scripts have already had CRLF normalised by callers. Splitting on LF therefore preserves the
/// reference checker's line numbers without treating exotic Unicode separators as source lines.
pub(crate) fn measure(text: &str) -> Measure {
    let mut out = Measure {
        code_lines: 0,
        comment_lines: 0,
        jsdoc_prose: 0,
        header_line: 0,
        inline_runs: Vec::new(),
        prose: Vec::new(),
    };
    let mut in_block = false;
    let mut first_block_done = false;
    let mut in_tags = false;
    let mut run = 0;
    let mut run_start = 0;
    for (index, raw) in text.split('\n').enumerate() {
        let number = index + 1;
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        if in_block {
            out.comment_lines += 1;
            let mut body = line.trim_start_matches('*').trim().to_string();
            if body.starts_with('@') {
                in_tags = true;
            } else if !first_block_done && !in_tags && !body.is_empty() {
                out.jsdoc_prose += 1;
            }
            if line.contains("*/") {
                in_block = false;
                first_block_done = true;
                in_tags = false;
                body = body.replace("*/", "").trim().to_string();
            }
            if !body.is_empty() {
                let starts_tag = body.starts_with('@');
                out.prose.push(Prose {
                    line: number,
                    text: body,
                    starts_tag,
                });
            }
            continue;
        }
        if line.starts_with("//")
            && line
                .trim_start_matches('/')
                .trim_start()
                .starts_with("twaco-lint-")
        {
            continue;
        }
        if line.starts_with("/*") {
            out.comment_lines += 1;
            if out.header_line == 0 {
                out.header_line = number;
            }
            in_block = !line.contains("*/");
            let mut body = line.trim_start_matches(['/', '*']).trim().to_string();
            if !in_block {
                first_block_done = true;
                body = body.replace("*/", "").trim().to_string();
            }
            if !body.is_empty() {
                out.prose.push(Prose {
                    line: number,
                    text: body,
                    starts_tag: false,
                });
            }
            continue;
        }
        if line.starts_with("//") {
            out.comment_lines += 1;
            if run == 0 {
                run_start = number;
            }
            run += 1;
            let body = line.trim_start_matches('/').trim();
            if !body.is_empty() {
                out.prose.push(Prose {
                    line: number,
                    text: body.to_string(),
                    starts_tag: false,
                });
            }
            continue;
        }
        if run != 0 {
            out.inline_runs.push((run_start, run));
        }
        run = 0;
        out.code_lines += 1;
    }
    if run != 0 {
        out.inline_runs.push((run_start, run));
    }
    out
}

pub(crate) fn findings(measure: &Measure, policy: &CommentPolicy) -> Vec<CommentFinding> {
    let compiled = &policy.compiled;
    let mut out = Vec::new();
    if measure.jsdoc_prose > policy.jsdoc_max_lines {
        out.push(CommentFinding { rule: "jsdoc-prose", line: measure.header_line, message: format!("the header comment has {} lines of prose (max {}); keep @param/@returns and cut the prose", measure.jsdoc_prose, policy.jsdoc_max_lines) });
    }
    for &(line, length) in &measure.inline_runs {
        if length > policy.line_run_max_lines {
            out.push(CommentFinding { rule: "line-run", line, message: format!("a run of {length} // lines (max {}); keep the comment beside the line it explains", policy.line_run_max_lines) });
        }
    }
    let total = measure.code_lines + measure.comment_lines;
    let ratio = if total == 0 {
        0.0
    } else {
        measure.comment_lines as f64 / total as f64
    };
    if measure.code_lines >= policy.ratio_min_code_lines && ratio > policy.ratio_max {
        out.push(CommentFinding {
            rule: "ratio",
            line: 0,
            message: format!(
                "{}% of the script is comments (max {}%)",
                rounded_percent(ratio),
                rounded_percent(policy.ratio_max)
            ),
        });
    }
    let mut seen_terms: Vec<Option<(String, usize)>> = vec![None; policy.synonyms.len()];
    for (start, text) in paragraphs(&measure.prose, &compiled.tag_prefix) {
        let short = truncate(&text);
        if text.contains("--") {
            out.push(CommentFinding {
                rule: "em-dash",
                line: start,
                message: format!("a \"--\" aside; split the sentence: {short}"),
            });
        }
        if let Some(phrase) = compiled
            .history
            .iter()
            .find(|(_, regex)| regex.is_match(&text))
        {
            out.push(CommentFinding {
                rule: "history",
                line: start,
                message: format!(
                    "'{}' describes history; say what the code does now: {short}",
                    phrase.0
                ),
            });
        }
        if let Some(phrase) = compiled
            .hedges
            .iter()
            .find(|(_, regex)| regex.is_match(&text))
        {
            out.push(CommentFinding {
                rule: "hedge",
                line: start,
                message: format!("'{}' hedges; state what the code does: {short}", phrase.0),
            });
        }
        for sentence in sentences(&text) {
            let words: Vec<&str> = compiled
                .words
                .find_iter(&sentence)
                .map(|found| found.as_str())
                .collect();
            let cap = if words
                .first()
                .is_some_and(|word| compiled.imperatives.contains(&word.to_ascii_lowercase()))
            {
                policy.instruction_max_words
            } else {
                policy.description_max_words
            };
            if words.len() > cap {
                out.push(CommentFinding {
                    rule: "length",
                    line: start,
                    message: format!("{} words (max {cap}): {}", words.len(), truncate(&sentence)),
                });
            }
            if sentence.contains(';') {
                out.push(CommentFinding {
                    rule: "bundled",
                    line: start,
                    message: format!(
                        "one sentence, two instructions; split at the ';': {}",
                        truncate(&sentence)
                    ),
                });
            }
            if let Some(found) = compiled.passive.captures(&sentence) {
                out.push(CommentFinding {
                    rule: "passive",
                    line: start,
                    message: format!(
                        "passive '{} ... by'; name the actor: {}",
                        &found[1],
                        truncate(&sentence)
                    ),
                });
            }
            if compiled.trailing_if.is_match(&sentence) {
                out.push(CommentFinding {
                    rule: "condition",
                    line: start,
                    message: format!("put the condition first: {}", truncate(&sentence)),
                });
            }
            if let Some(cluster) = noun_cluster(&words, policy.noun_max_run) {
                out.push(CommentFinding {
                    rule: "nouns",
                    line: start,
                    message: format!(
                        "{} nouns in a row ('{}'): {}",
                        policy.noun_max_run + 1,
                        cluster,
                        truncate(&sentence)
                    ),
                });
            }
        }
        // Python's set order is arbitrary. The written order makes this port deterministic.
        let lower = text.to_ascii_lowercase();
        for (index, terms) in compiled.synonyms.iter().enumerate() {
            if let Some(term) = terms
                .iter()
                .find(|(_, regex)| regex.is_match(&lower))
                .map(|(term, _)| term)
            {
                // As the reference does: a repeat of the remembered term moves the memory to
                // this line, so a later clash names the most recent use.
                match &seen_terms[index] {
                    Some((other, line)) if other != term => out.push(CommentFinding {
                        rule: "synonym",
                        line: start,
                        message: format!(
                            "'{term}' here, '{other}' at line {line}; pick one word for the concept"
                        ),
                    }),
                    _ => seen_terms[index] = Some((term.clone(), start)),
                }
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
struct Compiled {
    words: Regex,
    tag_prefix: Regex,
    passive: Regex,
    trailing_if: Regex,
    imperatives: BTreeSet<String>,
    history: Vec<(String, Regex)>,
    hedges: Vec<(String, Regex)>,
    synonyms: Vec<Vec<(String, Regex)>>,
}

impl Compiled {
    fn empty() -> Self {
        Self {
            words: Regex::new(".").unwrap(),
            tag_prefix: Regex::new(".").unwrap(),
            passive: Regex::new(".").unwrap(),
            trailing_if: Regex::new(".").unwrap(),
            imperatives: BTreeSet::new(),
            history: Vec::new(),
            hedges: Vec::new(),
            synonyms: Vec::new(),
        }
    }

    fn new(policy: &CommentPolicy) -> Self {
        let phrases = |values: &[String]| {
            values
                .iter()
                .map(|value| {
                    (
                        value.clone(),
                        Regex::new(&format!(r"(?i)\b{}\b", regex::escape(value)))
                            .expect("escaped phrase regex"),
                    )
                })
                .collect()
        };
        Self {
            words: Regex::new(r"[A-Za-z][A-Za-z'._-]*").unwrap(),
            tag_prefix: Regex::new(r"^@\w+\s*(?:\{[^}]*\}\s*)?(?:[\w.\[\]]+\s*-\s*)?").unwrap(),
            passive: Regex::new(
                r"(?i)\b(?:is|are|was|were|be|been|being)\s+(\w+(?:ed|en))\b\s+by\b",
            )
            .unwrap(),
            trailing_if: Regex::new(r#"(?i)[a-z0-9)\"']\s+(?:if|when|unless)\s+\w"#).unwrap(),
            imperatives: policy
                .imperatives
                .iter()
                .map(|value| value.to_ascii_lowercase())
                .collect(),
            history: phrases(&policy.history),
            hedges: phrases(&policy.hedges),
            // A prefix, as the reference checker matches it: `fetch` also finds "fetches".
            synonyms: policy
                .synonyms
                .iter()
                .map(|terms| {
                    terms
                        .iter()
                        .map(|term| {
                            let pattern = format!(r"(?i)\b{}", regex::escape(term));
                            (
                                term.clone(),
                                Regex::new(&pattern).expect("escaped term regex"),
                            )
                        })
                        .collect()
                })
                .collect(),
        }
    }
}

fn paragraphs(items: &[Prose], tag_prefix: &Regex) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut start = None;
    let mut buffer = Vec::new();
    let mut previous = None;
    for item in items {
        if previous.is_some_and(|line| item.line != line + 1 || item.starts_tag) {
            out.push((start.expect("paragraph start"), buffer.join(" ")));
            buffer.clear();
            start = None;
        }
        if start.is_none() {
            start = Some(item.line);
        }
        buffer.push(if item.starts_tag {
            tag_prefix.replace(&item.text, "").into_owned()
        } else {
            item.text.clone()
        });
        previous = Some(item.line);
    }
    if let Some(start) = start {
        out.push((start, buffer.join(" ")));
    }
    out
}

fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut whitespace = None;
    let mut previous = '\0';
    for (index, character) in text.char_indices() {
        if character.is_whitespace() && whitespace.is_none() && matches!(previous, '.' | '!' | '?')
        {
            whitespace = Some(index);
        }
        if !character.is_whitespace() {
            if let Some(split) = whitespace.take() {
                let sentence = text[start..split].trim();
                if !sentence.is_empty() {
                    out.push(sentence.to_string());
                }
                start = index;
            }
        }
        previous = character;
    }
    let sentence = text[start..].trim();
    if !sentence.is_empty() {
        out.push(sentence.to_string());
    }
    out
}

fn noun_cluster(words: &[&str], max_run: usize) -> Option<String> {
    let mut run = Vec::new();
    for word in words {
        let plain = word.to_ascii_lowercase().trim_matches('.').to_string();
        if STOPWORDS.contains(&plain.as_str())
            || word.contains('_')
            || word.contains('.')
            || word.chars().next().is_some_and(char::is_uppercase)
        {
            run.clear();
            continue;
        }
        run.push(*word);
        if run.len() > max_run {
            return Some(run.join(" "));
        }
    }
    None
}

fn truncate(text: &str) -> String {
    let mut chars = text.chars();
    let short: String = chars.by_ref().take(150).collect();
    if chars.next().is_some() {
        format!("{short}...")
    } else {
        short
    }
}

fn rounded_percent(value: f64) -> String {
    format!("{:.0}", (value * 100.0).round_ties_even())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(source: &str, policy: &CommentPolicy) -> Vec<(&'static str, usize)> {
        findings(&measure(source), policy)
            .into_iter()
            .map(|finding| (finding.rule, finding.line))
            .collect()
    }

    #[test]
    fn budgets_keep_the_reference_checkers_blank_line_and_header_quirks() {
        let policy = CommentPolicy::default();
        let source = "/* opening is not prose\n * first\n * second\n * third\n * @param {string} value - explanation\n */\n\n// one\n\n// two\n// three\nlet x = 1;";
        let found = rules(source, &policy);
        assert!(found.contains(&("jsdoc-prose", 1)));
        assert!(
            found.contains(&("line-run", 8)),
            "blank lines do not break a run"
        );
        assert!(!found.iter().any(|(rule, _)| *rule == "ratio"));
        assert!(
            !rules("/* x */\n/*\n * one\n * two\n * three\n */", &policy)
                .iter()
                .any(|(rule, _)| *rule == "jsdoc-prose")
        );
    }

    #[test]
    fn ratio_needs_enough_code_and_directives_are_blank() {
        let mut policy = CommentPolicy {
            ratio_min_code_lines: 2,
            ..CommentPolicy::uncompiled()
        };
        policy.compile();
        assert!(!rules("// comment\nlet a;", &policy)
            .iter()
            .any(|(rule, _)| *rule == "ratio"));
        // Two comments to two code lines is 50%; a directive in place of one is not a comment.
        assert!(rules("// one\n// two\nlet a;\nlet b;", &policy).contains(&("ratio", 0)));
        let source = "// twaco-lint-disable-next-line traps.for-in: no\n// two\nlet a;\nlet b;";
        assert!(!rules(source, &policy)
            .iter()
            .any(|(rule, _)| *rule == "ratio"));
    }

    #[test]
    fn prose_rules_follow_the_small_reference_grammar() {
        let mut policy = CommentPolicy {
            instruction_max_words: 2,
            description_max_words: 3,
            noun_max_run: 2,
            ..CommentPolicy::uncompiled()
        };
        policy.compile();
        let source = "// Previously, it is very important to note -- use one two three.\n// This sentence has four words. Do this now later; it was repaired by a person if ready. alpha beta gamma.\n// @param {string} name - text\n// fetch it\n\n// retrieve it\n// every item is fine";
        let found = rules(source, &policy);
        for rule in [
            "em-dash",
            "history",
            "hedge",
            "length",
            "bundled",
            "passive",
            "condition",
            "nouns",
            "synonym",
        ] {
            assert!(
                found.iter().any(|(found_rule, _)| *found_rule == rule),
                "{rule}: {found:?}"
            );
        }
        assert!(!findings(&measure("// every item"), &policy)
            .iter()
            .any(|finding| finding.rule == "hedge"));
        assert!(rules("// London.alpha item_one plain words", &policy)
            .iter()
            .all(|(rule, _)| *rule != "nouns"));
    }

    #[test]
    fn synonym_memory_is_per_measurement() {
        let policy = CommentPolicy::default();
        assert!(rules("// fetch it\n\n// retrieve it", &policy)
            .iter()
            .any(|(rule, _)| *rule == "synonym"));
        assert!(!rules("// retrieve it", &policy)
            .iter()
            .any(|(rule, _)| *rule == "synonym"));
    }
}
