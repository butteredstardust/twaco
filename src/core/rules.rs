//! The stable names of configurable script checks.

use super::config::Severity;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Rule {
    pub(crate) id: &'static str,
    pub(crate) gate: &'static str,
    pub(crate) rule: &'static str,
    pub(crate) summary: &'static str,
    pub(crate) options: &'static [RuleOption],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuleOption {
    pub(crate) name: &'static str,
    pub(crate) kind: OptionKind,
    #[allow(dead_code)] // The registry carries defaults for configuration documentation.
    pub(crate) default: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum OptionKind {
    Int { minimum: i64 },
    Float { minimum: f64, maximum: f64 },
    Phrases,
    Sets,
}

const NO_OPTIONS: &[RuleOption] = &[];
const JSDOC_OPTIONS: &[RuleOption] = &[RuleOption {
    name: "max_lines",
    kind: OptionKind::Int { minimum: 0 },
    default: "2",
}];
const LINE_RUN_OPTIONS: &[RuleOption] = &[RuleOption {
    name: "max_lines",
    kind: OptionKind::Int { minimum: 1 },
    default: "2",
}];
const RATIO_OPTIONS: &[RuleOption] = &[
    RuleOption {
        name: "max_ratio",
        kind: OptionKind::Float {
            minimum: 0.0,
            maximum: 1.0,
        },
        default: "0.40",
    },
    RuleOption {
        name: "min_code_lines",
        kind: OptionKind::Int { minimum: 0 },
        default: "15",
    },
];
const LENGTH_OPTIONS: &[RuleOption] = &[
    RuleOption {
        name: "instruction_max_words",
        kind: OptionKind::Int { minimum: 1 },
        default: "20",
    },
    RuleOption {
        name: "description_max_words",
        kind: OptionKind::Int { minimum: 1 },
        default: "25",
    },
    RuleOption {
        name: "imperatives_extend",
        kind: OptionKind::Phrases,
        default: "IMPERATIVES",
    },
    RuleOption {
        name: "imperatives_replace",
        kind: OptionKind::Phrases,
        default: "IMPERATIVES",
    },
];
const PHRASE_OPTIONS: &[RuleOption] = &[
    RuleOption {
        name: "phrases_extend",
        kind: OptionKind::Phrases,
        default: "built-in list",
    },
    RuleOption {
        name: "phrases_replace",
        kind: OptionKind::Phrases,
        default: "built-in list",
    },
];
const SYNONYM_OPTIONS: &[RuleOption] = &[
    RuleOption {
        name: "sets_extend",
        kind: OptionKind::Sets,
        default: "built-in sets",
    },
    RuleOption {
        name: "sets_replace",
        kind: OptionKind::Sets,
        default: "built-in sets",
    },
];
const NOUN_OPTIONS: &[RuleOption] = &[RuleOption {
    name: "max_run",
    kind: OptionKind::Int { minimum: 1 },
    default: "3",
}];

pub(crate) const RULES: &[Rule] = &[
    Rule {
        id: "format.unformatted",
        gate: "formatting",
        rule: "unformatted",
        summary: "The script is not laid out as `twaco fmt` would write it.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "format.will-not-parse",
        gate: "formatting",
        rule: "will-not-parse",
        summary: "The script does not parse as JavaScript.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "traps.const-in-loop",
        gate: "script traps",
        rule: "const-in-loop",
        summary: "A const declared in a loop body keeps its first value on every later iteration.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "traps.const-redeclared",
        gate: "script traps",
        rule: "const-redeclared",
        summary: "A name declared twice in one function fails the whole service to compile; const and let are function-scoped here.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "traps.for-in",
        gate: "script traps",
        rule: "for-in",
        summary: "for..in is a syntax error in the ThingWorx script engine.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "traps.indexed-rows",
        gate: "script traps",
        rule: "indexed-rows",
        summary: "Indexing an InfoTable's rows.toArray() by a loop variable re-reads the first row.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "traps.nan-risk",
        gate: "script traps",
        rule: "nan-risk",
        summary: "Number(x) written straight into a row is NaN when x is absent, which fails the whole service.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "traps.read-before-assign",
        gate: "script traps",
        rule: "read-before-assign",
        summary: "A top-level statement reads a name that only a later statement fills in.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "traps.unbounded-loop",
        gate: "script traps",
        rule: "unbounded-loop",
        summary: "A while loop can pin a platform thread if its body ever fails to advance.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "traps.suppression",
        gate: "script traps",
        rule: "suppression",
        summary: "A twaco-lint directive is malformed or suppresses nothing.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "order.main-code-below-a-helper",
        gate: "code order",
        rule: "main-code-below-a-helper",
        summary: "Main code sits below the first helper function, so the service reads out of order.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "order.declaration-between-helpers",
        gate: "code order",
        rule: "declaration-between-helpers",
        summary: "A declaration between helper functions is not hoisted, so code above it reads undefined.",
        options: NO_OPTIONS,
    },
    Rule {
        id: "order.suppression",
        gate: "code order",
        rule: "suppression",
        summary: "A twaco-lint directive is malformed or suppresses nothing.",
        options: NO_OPTIONS,
    },
    Rule { id: "comments.budget.jsdoc-prose", gate: "comment budget", rule: "jsdoc-prose", summary: "The header comment has more prose than its API tags need.", options: JSDOC_OPTIONS },
    Rule { id: "comments.budget.line-run", gate: "comment budget", rule: "line-run", summary: "A run of line comments is detached from the code it should explain.", options: LINE_RUN_OPTIONS },
    Rule { id: "comments.budget.ratio", gate: "comment budget", rule: "ratio", summary: "Comments occupy too much of a script that has enough code to judge.", options: RATIO_OPTIONS },
    Rule { id: "comments.prose.em-dash", gate: "comment prose", rule: "em-dash", summary: "A \"--\" aside is a second thought bolted on; split the sentence.", options: NO_OPTIONS },
    Rule { id: "comments.prose.history", gate: "comment prose", rule: "history", summary: "A comment describes what the code used to do instead of what it does.", options: PHRASE_OPTIONS },
    Rule { id: "comments.prose.length", gate: "comment prose", rule: "length", summary: "A comment sentence is too long to scan beside the code.", options: LENGTH_OPTIONS },
    Rule { id: "comments.prose.bundled", gate: "comment prose", rule: "bundled", summary: "A semicolon joins instructions that should be separate sentences.", options: NO_OPTIONS },
    Rule { id: "comments.prose.hedge", gate: "comment prose", rule: "hedge", summary: "A comment hedges instead of stating what the code does.", options: PHRASE_OPTIONS },
    Rule { id: "comments.prose.condition", gate: "comment prose", rule: "condition", summary: "A condition at the end of a comment makes its instruction harder to read.", options: NO_OPTIONS },
    Rule { id: "comments.prose.passive", gate: "comment prose", rule: "passive", summary: "Passive voice hides the actor responsible for the action.", options: NO_OPTIONS },
    Rule { id: "comments.prose.nouns", gate: "comment prose", rule: "nouns", summary: "A dense run of nouns makes a comment difficult to understand.", options: NOUN_OPTIONS },
    Rule { id: "comments.prose.synonym", gate: "comment prose", rule: "synonym", summary: "Different words describe the same concept in one script.", options: SYNONYM_OPTIONS },
];

/// Labels reserved when lint enables one of their rules. Future comment groups belong here.
pub(crate) const LINT_GATE_LABELS: &[&str] = &[
    "formatting",
    "script traps",
    "code order",
    "comment header",
    "comment budget",
    "comment prose",
];

pub(crate) fn by_id(id: &str) -> Option<&'static Rule> {
    RULES.iter().find(|rule| rule.id == id)
}

pub(crate) fn preset(name: &str, rule: &Rule) -> Option<Severity> {
    match name {
        // Today's gates and nothing more: the comment rules are only for those who opt in.
        "checks-v1" => Some(if rule.id.starts_with("comments.") {
            Severity::Off
        } else {
            Severity::Error
        }),
        "traps-only-v1" => Some(if rule.id.starts_with("traps.") {
            Severity::Error
        } else {
            Severity::Off
        }),
        "house-v1" => Some(if rule.id.starts_with("comments.") {
            match rule.id {
                "comments.prose.condition"
                | "comments.prose.passive"
                | "comments.prose.nouns"
                | "comments.prose.synonym" => Severity::Warn,
                _ => Severity::Error,
            }
        } else {
            Severity::Error
        }),
        _ => None,
    }
}

pub(crate) const PRESETS: &[&str] = &["checks-v1", "traps-only-v1", "house-v1"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_covers_every_rule_the_script_gates_emit() {
        let emitted = [
            ("formatting", "unformatted"),
            ("formatting", "will-not-parse"),
            ("script traps", "const-in-loop"),
            ("script traps", "const-redeclared"),
            ("script traps", "for-in"),
            ("script traps", "indexed-rows"),
            ("script traps", "nan-risk"),
            ("script traps", "read-before-assign"),
            ("script traps", "unbounded-loop"),
            ("script traps", "suppression"),
            ("code order", "main-code-below-a-helper"),
            ("code order", "declaration-between-helpers"),
            ("code order", "suppression"),
        ];
        for (gate, rule) in emitted {
            assert!(
                RULES
                    .iter()
                    .any(|known| known.gate == gate && known.rule == rule),
                "{gate}/{rule}"
            );
        }
    }
}
