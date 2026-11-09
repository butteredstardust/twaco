//! A collection's cell menu: its `CellMenu` property names a StateDefinition, and each state with
//! a `defaultValue` is a menu item that fires `CellMenu:<item>` (the value, with spaces made
//! hyphens and anything else but letters, digits, `:`, `_` and `-` left out), as the 10.2
//! collection widget builds them.

use serde_json::Value;

pub(crate) const PREFIX: &str = "CellMenu:";

/// The event a menu item with this value fires.
pub(crate) fn event_name(value: &str) -> String {
    let item: String = value
        .replace(' ', "-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
        .collect();
    format!("{PREFIX}{item}")
}

/// The events the repository's StateDefinition `menu` gives a cell menu; `None` when the
/// StateDefinition is not in the repository or does not read.
pub(crate) fn repository_events(
    discovered: &crate::core::workspace::Discovery,
    menu: &str,
) -> Option<Vec<String>> {
    let file = discovered
        .entities
        .iter()
        .find(|file| file.info.collection == "StateDefinitions" && file.info.name == menu)?;
    let bytes = std::fs::read(&file.path).ok()?;
    let definition = super::authoring::read_state_definition(&bytes).ok()?;
    Some(events(&definition))
}

/// The events a StateDefinition's content gives a collection's cell menu, in its order.
pub(crate) fn events(definition: &Value) -> Vec<String> {
    definition["stateDefinitions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|state| state["defaultValue"].as_str())
        .filter(|value| !value.is_empty())
        .map(event_name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_item_fires_its_value_with_spaces_hyphenated_and_punctuation_dropped() {
        assert_eq!(event_name("Delete"), "CellMenu:Delete");
        assert_eq!(event_name("More Details"), "CellMenu:More-Details");
        assert_eq!(event_name("Edit (v2)!"), "CellMenu:Edit-v2");
    }

    #[test]
    fn only_states_with_a_value_are_items() {
        let definition = json!({"stateType": "string", "stateDefinitions": [
            {"defaultValue": "Edit"}, {"defaultValue": "More Details"}, {"defaultValue": ""}, {}
        ]});
        assert_eq!(
            events(&definition),
            ["CellMenu:Edit", "CellMenu:More-Details"]
        );
    }
}
