//! What the runtime is known or likely not to honour in a widget's `--style` entry. The notes are
//! advisories: they come from comparing built mashups with the platform kits' exports, not from a
//! specification, and say so.

use serde_json::Value;

/// Properties the host part (`$`) cannot use to place the widget: Composer positions a widget in
/// its container from the widget's own properties, so the kits never put these in a style entry.
const PLACEMENT: &[&str] = &[
    "position",
    "top",
    "left",
    "right",
    "bottom",
    "inset",
    "transform",
    "flex",
    "flex-grow",
    "flex-shrink",
    "flex-basis",
    "order",
    "align-self",
    "justify-self",
];

/// Parts the kits' exports and twaco's own recipes style, per styler: a part outside this list may
/// not exist on the widget. A styler with no entry has no part known beyond `$`.
fn known_parts(styler: &str) -> &'static [&'static str] {
    match styler {
        "flex-container" => &["$"],
        "ptcs-button" => &["$", "icon", "label"],
        "ptcs-collection" => &["$", "cell"],
        "ptcs-divider" => &["line"],
        "ptcs-dynamic-panel" => &["container", "panel"],
        "ptcs-grid" => &["$", "core-grid", "header-cell", "cell"],
        "ptcs-image" => &["$"],
        "ptcs-kpi-dial" => &["$", "UOM", "value-tracker"],
        "ptcs-label" => &["$", "label"],
        "ptcs-tab-set" => &["pages", "tabs-header", "tabs-tab"],
        "ptcs-textfield" => &["$", "text-box", "icon"],
        _ => &["$"],
    }
}

/// One advisory: its rule and what was seen.
pub(crate) type Note = (&'static str, String);

/// The advisories for one widget's style entry (`{"parts": {part: {state: {property: value}}}}`).
pub(crate) fn entry_notes(id: &str, entry: &Value) -> Vec<Note> {
    let styler = entry["widget"].as_str().unwrap_or_default();
    let mut notes = Vec::new();
    let Some(parts) = entry["parts"].as_object() else {
        return notes;
    };
    for (part, states) in parts {
        if !known_parts(styler).contains(&part.as_str()) {
            let seen = known_parts(styler).join(", ");
            let hint = if styler.starts_with("ptcs-chart") {
                "; a chart's text, axes and bars are not styled through parts in any kit: set series colours with a state format (mashup state-format set) and copy a styled chart with --style-from if one exists"
            } else {
                ""
            };
            notes.push((
                "style-unknown-part",
                format!(
                    "{id}: part `{part}` is not one the kits style on {styler} (known: {seen}); the runtime ignores a part the widget lacks{hint}"
                ),
            ));
        }
        for (state, css) in states.as_object().into_iter().flatten() {
            for (property, value) in css.as_object().into_iter().flatten() {
                if value
                    .as_str()
                    .is_some_and(|text| text.contains("!important"))
                {
                    notes.push((
                        "style-important",
                        format!(
                            "{id}: {part}@{state} {property} ends in !important, which made the setting invisible at run time in a built page (the value is applied as a plain property); say it without !important"
                        ),
                    ));
                }
                if part == "$" && state == "$" && PLACEMENT.contains(&property.as_str()) {
                    let message = if styler == "flex-container" && property.starts_with("flex") {
                        format!(
                            "{id}: {property} in a container's style is applied inside its own box: a header with `flex: 0 0 74px` became 74px wide instead of spanning the page; set flex-grow, flex-basis or flex-min-height/flex-max-height with --prop"
                        )
                    } else {
                        format!(
                            "{id}: {property} on the widget itself did not move it in a built page, since Composer places a widget from its own properties; use Width/Height/flex-grow with --prop, or a wrapper row with align-items/justify-content"
                        )
                    };
                    notes.push(("style-placement-property", message));
                }
            }
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rules(entry: Value) -> Vec<&'static str> {
        entry_notes("ptcstextfield-3", &entry)
            .into_iter()
            .map(|(rule, _)| rule)
            .collect()
    }

    #[test]
    fn kit_parts_and_plain_values_raise_nothing() {
        let entry = json!({"widget": "ptcs-textfield", "parts": {
            "text-box": {"$": {"border-radius": "20px"}, "focus": {"border-color": "#009bd6"}},
            "icon": {"$": {"color": "red"}}, "$": {"$": {"background": "#fff"}}}});
        assert!(rules(entry).is_empty());
    }

    #[test]
    fn important_placement_and_unknown_parts_are_named() {
        let entry = json!({"widget": "ptcs-textfield", "parts": {
            "$": {"$": {"position": "absolute", "flex": "1 1 0% !important"}},
            "placeholder": {"$": {"color": "red"}}}});
        let mut found = rules(entry);
        found.sort_unstable();
        assert_eq!(
            found,
            [
                "style-important",
                "style-placement-property",
                "style-placement-property",
                "style-unknown-part"
            ]
        );
    }

    #[test]
    fn a_container_flex_style_says_the_header_collapses() {
        let entry =
            json!({"widget": "flex-container", "parts": {"$": {"$": {"flex": "0 0 74px"}}}});
        let notes = entry_notes("flexcontainer-3", &entry);
        assert!(notes[0].1.contains("74px wide"), "{}", notes[0].1);
    }

    #[test]
    fn a_chart_part_says_charts_are_not_styled_through_parts() {
        let entry =
            json!({"widget": "ptcs-chart-bar", "parts": {"xaxis": {"$": {"color": "#fff"}}}});
        let notes = entry_notes("ptcschartbar-4", &entry);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].1.contains("state-format"), "{}", notes[0].1);
    }
}
