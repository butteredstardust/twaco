//! The stable names of configurable script checks.

use super::config::Severity;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Rule {
    pub(crate) id: &'static str,
    pub(crate) gate: &'static str,
    pub(crate) rule: &'static str,
    pub(crate) summary: &'static str,
}

pub(crate) const RULES: &[Rule] = &[
    Rule {
        id: "format.unformatted",
        gate: "formatting",
        rule: "unformatted",
        summary: "The script is not laid out as `twaco fmt` would write it.",
    },
    Rule {
        id: "format.will-not-parse",
        gate: "formatting",
        rule: "will-not-parse",
        summary: "The script does not parse as JavaScript.",
    },
    Rule {
        id: "traps.const-in-loop",
        gate: "script traps",
        rule: "const-in-loop",
        summary: "A const declared in a loop body keeps its first value on every later iteration.",
    },
    Rule {
        id: "traps.const-redeclared",
        gate: "script traps",
        rule: "const-redeclared",
        summary: "A name declared twice in one function fails the whole service to compile; const and let are function-scoped here.",
    },
    Rule {
        id: "traps.for-in",
        gate: "script traps",
        rule: "for-in",
        summary: "for..in is a syntax error in the ThingWorx script engine.",
    },
    Rule {
        id: "traps.indexed-rows",
        gate: "script traps",
        rule: "indexed-rows",
        summary: "Indexing an InfoTable's rows.toArray() by a loop variable re-reads the first row.",
    },
    Rule {
        id: "traps.nan-risk",
        gate: "script traps",
        rule: "nan-risk",
        summary: "Number(x) written straight into a row is NaN when x is absent, which fails the whole service.",
    },
    Rule {
        id: "traps.read-before-assign",
        gate: "script traps",
        rule: "read-before-assign",
        summary: "A top-level statement reads a name that only a later statement fills in.",
    },
    Rule {
        id: "traps.unbounded-loop",
        gate: "script traps",
        rule: "unbounded-loop",
        summary: "A while loop can pin a platform thread if its body ever fails to advance.",
    },
    Rule {
        id: "traps.suppression",
        gate: "script traps",
        rule: "suppression",
        summary: "A twaco-lint directive is malformed or suppresses nothing.",
    },
    Rule {
        id: "order.main-code-below-a-helper",
        gate: "code order",
        rule: "main-code-below-a-helper",
        summary: "Main code sits below the first helper function, so the service reads out of order.",
    },
    Rule {
        id: "order.declaration-between-helpers",
        gate: "code order",
        rule: "declaration-between-helpers",
        summary: "A declaration between helper functions is not hoisted, so code above it reads undefined.",
    },
    Rule {
        id: "order.suppression",
        gate: "code order",
        rule: "suppression",
        summary: "A twaco-lint directive is malformed or suppresses nothing.",
    },
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
        "checks-v1" => Some(Severity::Error),
        "traps-only-v1" => Some(if rule.id.starts_with("traps.") {
            Severity::Error
        } else {
            Severity::Off
        }),
        // Comment rules will make this stricter than checks-v1 in a later phase.
        "house-v1" => Some(Severity::Error),
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
