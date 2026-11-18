//! The CDS icon names ThingWorx 10.2 ships. A `cds:` value naming anything else renders as a
//! blank icon, so every value twaco writes is checked against this list.

use std::sync::LazyLock;

const NAMES: &str = include_str!("widgets/cds_icons.txt");

/// The prefix of a CDS icon value.
pub(crate) const PREFIX: &str = "cds:";

/// Icon names that do not exist, and the CDS icon that is meant by them (`icon_menu`: the menu
/// button is `icon_hamburger`).
const SYNONYMS: &[(&str, &str)] = &[("_menu", "icon_hamburger")];

/// The icon names, sorted, without the prefix.
pub fn names() -> &'static [&'static str] {
    static LIST: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
        NAMES
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect()
    });
    &LIST
}

/// The names containing `filter` (all of them when it is empty), as `cds:<name>`.
pub fn list(filter: Option<&str>) -> Vec<String> {
    let filter = filter.unwrap_or("").to_lowercase();
    names()
        .iter()
        .filter(|name| name.to_lowercase().contains(&filter))
        .map(|name| format!("{PREFIX}{name}"))
        .collect()
}

/// Refuse a `cds:` value that is not a 10.2 icon, naming the closest ones. Any other value (a
/// media entity, a URL) passes.
pub(crate) fn check(value: &str) -> Result<(), String> {
    let Some(name) = value.strip_prefix(PREFIX) else {
        return Ok(());
    };
    if names().binary_search(&name).is_ok() {
        return Ok(());
    }
    // Names other icon sets use for an icon CDS has under another name.
    let alias = SYNONYMS
        .iter()
        .find(|(word, _)| name.ends_with(word))
        .map(|(_, icon)| *icon);
    let mut close: Vec<(usize, &str)> = names()
        .iter()
        .map(|candidate| {
            let distance = strsim::levenshtein(name, candidate);
            (
                if Some(*candidate) == alias {
                    0
                } else {
                    distance
                },
                *candidate,
            )
        })
        .collect();
    close.sort();
    let close: Vec<String> = close
        .into_iter()
        .take(3)
        .map(|(_, candidate)| format!("{PREFIX}{candidate}"))
        .collect();
    Err(format!(
        "{value} is not a CDS icon in ThingWorx 10.2; closest: {}; `twaco widgets --icons` lists them",
        close.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_sorted_and_whole() {
        let names = names();
        assert_eq!(names.len(), 153);
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn a_known_name_and_other_values_pass() {
        // What the collapsible recipe writes.
        for icon in [
            "cds:icon_hamburger",
            "cds:icon_chevron_right",
            "cds:icon_chevron_left",
        ] {
            assert!(check(icon).is_ok(), "{icon}");
        }
        assert!(check("Acme.Media.Logo").is_ok());
        assert!(check("/Thingworx/MediaEntities/Acme.Logo").is_ok());
        assert!(check("").is_ok());
    }

    #[test]
    fn a_typo_names_the_closest_icons() {
        let error = check("cds:icon_chevron_rigth").unwrap_err();
        assert!(error.contains("cds:icon_chevron_right"), "{error}");
        let error = check("cds:icon_menu").unwrap_err();
        assert!(error.contains("not a CDS icon"), "{error}");
    }

    #[test]
    fn every_icon_the_catalogue_and_recipes_write_exists() {
        let catalogue = include_str!("widgets/catalog.json");
        for part in catalogue.split("cds:").skip(1) {
            let name: String = part
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            assert!(check(&format!("cds:{name}")).is_ok(), "{name}");
        }
    }
}
