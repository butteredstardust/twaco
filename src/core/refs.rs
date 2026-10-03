//! Token-aware matching for names referenced by ThingWorx source values.
//!
//! Matches carry byte ranges into the original UTF-8 text. Qualified embedded names are safe to
//! rewrite; ambiguous unqualified names are retained as review findings. This module deliberately
//! has no knowledge of files or markup, so every caller uses the same boundary rule.

/// Whether the sought name is a complete entity identity or a dotted-name building block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Refuses `A.B` inside another entity name such as `A.B.C`.
    Entity,
    /// Allows `A.B` at the head of a dotted name such as `A.B.C`.
    Prefix,
}

/// The certainty with which a name occurrence can be rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// The hit is the whole value, apart from whitespace or a derived-id prefix. Always applied.
    Exact,
    /// The hit is inside longer text and its context makes it safe to apply.
    Embedded,
    /// The hit is inside longer text and is left unchanged for a person to review.
    Review,
}

/// One non-overlapping name occurrence, addressed by byte offsets into the source value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The first byte of the name itself.
    pub start: usize,
    /// The byte immediately after the name itself.
    pub end: usize,
    /// Whether the occurrence is applied automatically or retained for review.
    pub tier: Tier,
}

impl Hit {
    /// Returns whether replacement is allowed without human review.
    pub fn applies(&self) -> bool {
        matches!(self.tier, Tier::Exact | Tier::Embedded)
    }
}

/// Prefixes used by mashup identifiers derived from an entity name.
pub const DERIVED_PREFIXES: [&str; 6] = [
    "Things_",
    "ThingTemplates_",
    "ThingShapes_",
    "DynamicThings_",
    "DynamicThingTemplates_",
    "DynamicThingShapes_",
];

/// Finds exact-case, whole-token occurrences of `name` in one source value.
///
/// Results are ordered and non-overlapping, and their ranges cover only `name`, never a derived-id
/// prefix. Entity mode refuses a name at the head of a longer dotted entity name. An empty name is
/// refused by returning no hits. Non-ASCII surrounding text is preserved through byte offsets that
/// always lie on UTF-8 character boundaries.
pub fn find(text: &str, name: &str, mode: Mode) -> Vec<Hit> {
    if name.is_empty() {
        return Vec::new();
    }

    text.match_indices(name)
        .filter_map(|(start, _)| {
            let end = start + name.len();
            let before = &text[..start];
            let after = &text[end..];
            if !before_is_boundary(before) || !after_is_boundary(after, mode) {
                return None;
            }

            let tier = if is_exact_value(text, name, start, end) {
                Tier::Exact
            } else if is_qualified(name) || (mode == Mode::Prefix && starts_with_dotted_name(after))
            {
                Tier::Embedded
            } else {
                Tier::Review
            };
            Some(Hit { start, end, tier })
        })
        .collect()
}

/// Finds name occurrences inside a script or other free-form text blob.
///
/// This has the same boundaries, ordering and refusal of empty names as [`find`]. A Review hit is
/// promoted to Exact only when the name is the entire content of a balanced ASCII string literal;
/// qualified Embedded hits and already-Exact hits retain their original tier. A quote preceded by
/// a backslash is part of a longer string, not the start of a literal, so it promotes nothing.
pub fn find_in_text(text: &str, name: &str, mode: Mode) -> Vec<Hit> {
    find(text, name, mode)
        .into_iter()
        .map(|mut hit| {
            if hit.tier == Tier::Review {
                let before = text[..hit.start].chars().next_back();
                let after = text[hit.end..].chars().next();
                // The quote is one byte, so `hit.start - 1` is only reached when `before` is one.
                if before == after
                    && matches!(before, Some('"' | '\'' | '`'))
                    && !text[..hit.start - 1].ends_with('\\')
                {
                    hit.tier = Tier::Exact;
                }
            }
            hit
        })
        .collect()
}

/// Replaces every applicable hit while copying all other bytes unchanged.
///
/// `hits` must be ordered, non-overlapping ranges on UTF-8 boundaries, as returned by [`find`].
/// Review hits are deliberately not replaced. With no applicable hits, the returned string has
/// exactly the same content as `text`.
pub fn replace(text: &str, hits: &[Hit], new: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for hit in hits.iter().filter(|hit| hit.applies()) {
        out.push_str(&text[cursor..hit.start]);
        out.push_str(new);
        cursor = hit.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Returns whether a name has at least one dotted qualifier.
pub fn is_qualified(name: &str) -> bool {
    name.contains('.')
}

/// Validates a proposed entity or prefix name.
///
/// Refuses empty names, non-ASCII or unsupported characters, leading or trailing dots, and empty
/// components introduced by consecutive dots. The error identifies the broken rule and quotes the
/// rejected name.
pub fn validate_new_name(new: &str) -> Result<(), String> {
    if new.is_empty() {
        return Err(format!("new name {new:?} must not be empty"));
    }
    if !new.chars().all(|c| is_name_char(c) || c == '.') {
        return Err(format!(
            "new name {new:?} may contain only ASCII letters, digits, '_', '-', and '.'"
        ));
    }
    if new.starts_with('.') {
        return Err(format!("new name {new:?} must not start with '.'"));
    }
    if new.ends_with('.') {
        return Err(format!("new name {new:?} must not end with '.'"));
    }
    if new.contains("..") {
        return Err(format!(
            "new name {new:?} must not contain consecutive dots ('..')"
        ));
    }
    // The entity file is named for the entity, and Windows refuses NUL.xml and CON.Thing.xml
    // however the part after the first dot reads.
    let head = new.split('.').next().unwrap_or(new).to_ascii_uppercase();
    let reserved = matches!(head.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (head.len() == 4
            && (head.starts_with("COM") || head.starts_with("LPT"))
            && matches!(head.as_bytes()[3], b'1'..=b'9'));
    if reserved {
        return Err(format!(
            "new name {new:?} starts with {head}, a reserved device name Windows will not use for a file"
        ));
    }
    Ok(())
}

/// Validates a field-like name as an XML element-compatible ThingWorx identifier.
pub fn validate_field_name(name: &str, noun: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{noun} name must not be empty"));
    }
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        return Err(format!(
            "{noun} name {name:?} must start with an ASCII letter or '_'"
        ));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!(
            "{noun} name {name:?} may contain only ASCII letters, digits, and '_'"
        ));
    }
    Ok(())
}

/// Words a service parameter must not be called: JavaScript's reserved words and the names a
/// ThingWorx script already has in scope. Renaming a parameter to one of them would shadow it or
/// not compile.
const RESERVED_IDENTIFIERS: &[&str] = &[
    "arguments",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "undefined",
    "NaN",
    "Infinity",
    // what a ThingWorx service script already has in scope
    "me",
    "result",
    "logger",
    "Things",
    "ThingTemplates",
    "ThingShapes",
    "DataShapes",
    "Resources",
    "Mashups",
    "Groups",
    "Users",
    "Organizations",
    "Projects",
    "Subsystems",
    "Collections",
    "Networks",
    "StyleDefinitions",
    "Authenticators",
    "ApplicationKeys",
];

/// A service parameter's name: an identifier that is not reserved.
pub fn validate_param_name(name: &str) -> Result<(), String> {
    validate_field_name(name, "parameter")?;
    if RESERVED_IDENTIFIERS.contains(&name) {
        return Err(format!("parameter name {name:?} is reserved: it is a JavaScript keyword or a name every ThingWorx script already has"));
    }
    Ok(())
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
}

fn before_is_boundary(before: &str) -> bool {
    let ordinary = match before.chars().next_back() {
        None => true,
        Some(c) => !is_name_char(c) && c != '.',
    };
    ordinary || has_derived_prefix(before)
}

fn has_derived_prefix(before: &str) -> bool {
    DERIVED_PREFIXES.iter().any(|prefix| {
        let Some(prefix_start) = before.len().checked_sub(prefix.len()) else {
            return false;
        };
        before.ends_with(prefix)
            && (prefix_start == 0
                || before[..prefix_start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| !is_name_char(c) && c != '.'))
    })
}

fn after_is_boundary(after: &str, mode: Mode) -> bool {
    if after.chars().next().is_some_and(is_name_char) {
        return false;
    }
    mode == Mode::Prefix || !starts_with_dotted_name(after)
}

fn starts_with_dotted_name(after: &str) -> bool {
    let mut chars = after.chars();
    chars.next() == Some('.') && chars.next().is_some_and(is_name_char)
}

fn is_exact_value(text: &str, name: &str, start: usize, end: usize) -> bool {
    let trimmed = text.trim();
    let trimmed_start = text.len() - text.trim_start().len();
    let trimmed_end = trimmed_start + trimmed.len();

    if start == trimmed_start && end == trimmed_end && trimmed == name {
        return true;
    }
    DERIVED_PREFIXES.iter().any(|prefix| {
        start == trimmed_start + prefix.len()
            && end == trimmed_end
            && trimmed.strip_prefix(prefix) == Some(name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn one(text: &str, name: &str, mode: Mode) -> Hit {
        let hits = find(text, name, mode);
        assert_eq!(hits.len(), 1, "unexpected hits for {text:?}");
        hits[0].clone()
    }

    #[test]
    fn whole_value_and_surrounding_whitespace_are_exact() {
        assert_eq!(
            one("Acme.App.Manager", "Acme.App.Manager", Mode::Entity),
            Hit {
                start: 0,
                end: 16,
                tier: Tier::Exact
            }
        );
        assert_eq!(
            one(" \tAcme.App.Manager\r\n", "Acme.App.Manager", Mode::Entity),
            Hit {
                start: 2,
                end: 18,
                tier: Tier::Exact
            }
        );
    }

    #[test]
    fn entity_refuses_longer_dotted_name_while_prefix_accepts_it() {
        assert!(find("Acme.App.Manager.Child", "Acme.App.Manager", Mode::Entity).is_empty());
        assert_eq!(
            one("Acme.App.Manager.Child", "Acme.App", Mode::Prefix).tier,
            Tier::Embedded
        );
    }

    #[test]
    fn prefix_still_requires_a_whole_token() {
        for text in ["Acme.AppX.Thing", "XAcme.App.Thing", "my.Acme.App.Thing"] {
            assert!(
                find(text, "Acme.App", Mode::Prefix).is_empty(),
                "matched {text:?}"
            );
        }
    }

    #[test]
    fn hyphen_and_underscore_continue_a_name() {
        assert!(find("Acme.App-2", "Acme.App", Mode::Entity).is_empty());
        assert!(find("Acme.App_TS", "Acme.App", Mode::Entity).is_empty());
    }

    #[test]
    fn terminal_dot_is_punctuation_but_dotted_suffix_is_an_entity() {
        assert_eq!(
            one("see Acme.App.", "Acme.App", Mode::Entity).tier,
            Tier::Embedded
        );
        assert!(find("see Acme.App.xml", "Acme.App", Mode::Entity).is_empty());
        assert_eq!(
            one("see Acme.App. now", "Acme.App", Mode::Entity).tier,
            Tier::Embedded
        );
    }

    #[test]
    fn compound_principals_match_each_qualified_name() {
        let text = "Acme.Default_OR:Acme.Admin_UG";
        let first = one(text, "Acme.Default_OR", Mode::Entity);
        let second = one(text, "Acme.Admin_UG", Mode::Entity);
        assert_eq!(first.tier, Tier::Embedded);
        assert_eq!(second.tier, Tier::Embedded);
        assert_eq!(&text[first.start..first.end], "Acme.Default_OR");
        assert_eq!(&text[second.start..second.end], "Acme.Admin_UG");
    }

    #[test]
    fn urls_and_localization_tokens_are_embedded() {
        assert_eq!(
            one(
                "/Thingworx/MediaEntities/Acme.App.Icon_MD",
                "Acme.App.Icon_MD",
                Mode::Entity
            )
            .tier,
            Tier::Embedded
        );
        assert_eq!(
            one("[[Acme.App.Save]]", "Acme.App", Mode::Prefix).tier,
            Tier::Embedded
        );
    }

    #[test]
    fn derived_ids_have_a_boundary_and_exact_hits_exclude_the_prefix() {
        let text = "DynamicThingShapes_Acme.App.Management_TS";
        let hit = one(text, "Acme.App.Management_TS", Mode::Entity);
        assert_eq!(hit.tier, Tier::Exact);
        assert_eq!(&text[hit.start..hit.end], "Acme.App.Management_TS");

        let unqualified = one("Things_T", "T", Mode::Entity);
        assert_eq!(
            unqualified,
            Hit {
                start: 7,
                end: 8,
                tier: Tier::Exact
            }
        );

        assert!(find("XThings_Acme.App.A", "Acme.App.A", Mode::Entity).is_empty());
        assert_eq!(
            one("see Things_Acme.App.A now", "Acme.App.A", Mode::Entity).tier,
            Tier::Embedded
        );
    }

    #[test]
    fn unqualified_embedded_names_are_review_unless_heading_a_prefix() {
        let exact = one("T", "T", Mode::Entity);
        assert_eq!(exact.tier, Tier::Exact);
        assert!(exact.applies());

        let topic = one("T/T1/A1", "T", Mode::Entity);
        assert_eq!(topic.tier, Tier::Review);
        assert!(!topic.applies());

        assert_eq!(
            one("T.Manager", "T", Mode::Prefix).tier,
            Tier::Embedded
        );
        assert_eq!(
            one("the T project", "T", Mode::Prefix).tier,
            Tier::Review
        );
    }

    #[test]
    fn a_whole_quoted_name_in_a_blob_is_exact() {
        for quote in ['"', '\'', '`'] {
            let text = format!("call({quote}T{quote});");
            assert_eq!(
                one_in_text(&text, "T", Mode::Entity).tier,
                Tier::Exact
            );
        }
    }

    #[test]
    fn blob_quote_promotion_requires_matching_balanced_quotes() {
        for text in [
            "call(\"T/T1\")",
            "call(\"T)",
            "call(\"T')",
        ] {
            assert_eq!(
                one_in_text(text, "T", Mode::Entity).tier,
                Tier::Review
            );
        }
    }

    #[test]
    fn quoted_qualified_blob_hits_keep_their_embedded_tier() {
        assert_eq!(
            one_in_text("const x = \"Acme.App.X\";", "Acme.App.X", Mode::Entity).tier,
            Tier::Embedded
        );
    }

    #[test]
    fn finds_several_non_overlapping_occurrences_in_order() {
        let text = "Acme.App.X Acme.App.X;Acme.App.X";
        let hits = find(text, "Acme.App.X", Mode::Entity);
        assert_eq!(
            hits.iter().map(|hit| hit.start).collect::<Vec<_>>(),
            [0, 11, 22]
        );
        assert!(hits.iter().all(|hit| hit.tier == Tier::Embedded));
    }

    #[test]
    fn matching_is_case_sensitive() {
        assert!(find("acme.app.x", "Acme.App.X", Mode::Entity).is_empty());
    }

    #[test]
    fn non_ascii_surroundings_keep_byte_offsets_and_replacement_intact() {
        let text = "Größe Acme.App.X ✓";
        let hit = one(text, "Acme.App.X", Mode::Entity);
        assert_eq!(&text[hit.start..hit.end], "Acme.App.X");
        assert_eq!(hit.start, "Größe ".len());
        assert_eq!(replace(text, &[hit], "Acme.New.X"), "Größe Acme.New.X ✓");
    }

    #[test]
    fn replace_applies_safe_tiers_and_skips_review() {
        let text = "Acme.App T";
        let hits = [
            Hit {
                start: 0,
                end: 8,
                tier: Tier::Embedded,
            },
            Hit {
                start: 9,
                end: 19,
                tier: Tier::Review,
            },
        ];
        assert_eq!(replace(text, &hits, "Changed"), "Changed T");

        let exact = find("T", "T", Mode::Entity);
        assert_eq!(replace("T", &exact, "U"), "U");
    }

    #[test]
    fn replace_is_identity_without_hits_and_handles_length_changes() {
        assert_eq!(replace("unchanged", &[], "anything"), "unchanged");

        let text = "Acme.App.X and Acme.App.X";
        let hits = find(text, "Acme.App.X", Mode::Entity);
        assert_eq!(
            replace(text, &hits, "Acme.Longer.Name"),
            "Acme.Longer.Name and Acme.Longer.Name"
        );
        assert_eq!(replace(text, &hits, "A.B"), "A.B and A.B");
    }

    #[test]
    fn empty_name_has_no_hits() {
        assert!(find("anything", "", Mode::Entity).is_empty());
    }

    #[test]
    fn qualified_names_are_recognized_by_their_dot() {
        assert!(is_qualified("Acme.App"));
        assert!(!is_qualified("T"));
    }

    #[test]
    fn validates_supported_new_names() {
        for name in ["Acme.New.Name_TS", "A", "a-b.c_d"] {
            assert_eq!(validate_new_name(name), Ok(()));
        }
    }

    #[test]
    fn invalid_new_names_report_the_distinct_broken_rule_and_quote_the_name() {
        let cases = [
            ("", "must not be empty"),
            (".A", "must not start"),
            ("A.", "must not end"),
            ("A..B", "consecutive dots"),
            ("A B", "only ASCII"),
            ("A/B", "only ASCII"),
            ("Ä", "only ASCII"),
        ];
        let mut messages = HashSet::new();
        for (name, rule) in cases {
            let message = validate_new_name(name).unwrap_err();
            assert!(message.contains(rule), "{message:?} does not name {rule:?}");
            assert!(
                message.contains(&format!("{name:?}")),
                "{message:?} does not quote the name"
            );
            messages.insert(message);
        }
        assert_eq!(messages.len(), cases.len());
    }

    #[test]
    fn field_names_follow_the_identifier_rule() {
        for name in ["Period", "_period2", "a0"] {
            assert_eq!(validate_field_name(name, "field"), Ok(()));
        }
        for name in ["", "1x", "a b", "a.b"] {
            assert!(validate_field_name(name, "field").is_err(), "{name:?}");
        }
    }

    #[test]
    fn qualified_rename_round_trip_restores_mixed_text() {
        // Precondition: the replacement name does not already occur at a matching position in the
        // original, so the reverse pass cannot mistake pre-existing text for an earlier edit.
        let text = "Acme.App.X, Things_Acme.App.X, and /Acme.App.X/end";
        let renamed = replace(text, &find(text, "Acme.App.X", Mode::Entity), "Acme.New.X");
        let restored = replace(
            &renamed,
            &find(&renamed, "Acme.New.X", Mode::Entity),
            "Acme.App.X",
        );
        assert_eq!(restored, text);
    }

    #[test]
    fn a_derived_prefix_is_a_token_of_its_own() {
        // `my.Things_` is the tail of a longer dotted name, not a mashup-derived id.
        assert!(find("my.Things_Acme.App", "Acme.App", Mode::Prefix).is_empty());
        assert_eq!(
            find("x Things_Acme.App y", "Acme.App", Mode::Prefix).len(),
            1
        );
    }

    #[test]
    fn an_escaped_quote_does_not_make_a_literal() {
        // The name sits inside a longer string; only a real literal of the name alone is exact.
        let escaped = r#"var s = "prefix \"T\" suffix";"#;
        assert_eq!(
            one_in_text(escaped, "T", Mode::Entity).tier,
            Tier::Review
        );
        let literal = r#"var s = "T";"#;
        assert_eq!(
            one_in_text(literal, "T", Mode::Entity).tier,
            Tier::Exact
        );
        // A name at the very start has no quote before it, and must not index before the text.
        assert_eq!(
            one_in_text("T\"", "T", Mode::Entity).tier,
            Tier::Review
        );
    }

    #[test]
    fn windows_device_names_are_refused_as_the_head_of_a_name() {
        for name in [
            "NUL",
            "con",
            "Com1",
            "LPT9.Thing",
            "AUX.Anything.Here",
            "PRN",
        ] {
            let message = validate_new_name(name).unwrap_err();
            assert!(
                message.contains("reserved device name"),
                "{name}: {message}"
            );
        }
        // Only the first component matters, and COM0, COM10 and CONSOLE are fine.
        for name in ["Acme.CON", "COM0", "COM10", "CONSOLE", "Nullable"] {
            assert_eq!(validate_new_name(name), Ok(()), "{name}");
        }
    }

    fn one_in_text(text: &str, name: &str, mode: Mode) -> Hit {
        let hits = find_in_text(text, name, mode);
        assert_eq!(hits.len(), 1, "unexpected hits for {text:?}");
        hits[0].clone()
    }
}
