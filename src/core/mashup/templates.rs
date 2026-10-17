//! The ThingWorx 10.2 files a new mashup is made from.
//!
//! `mashup.xml` is a Mashup entity as the 10.2 server exports one, with nothing in it: no
//! permissions, no parameters, and an empty content document that `mashup::sync` fills in, so a new
//! mashup's content is written exactly as a synced one is. `blank.content.json` is the content of
//! a responsive mashup holding one `center` container; `master_root.json` is the root widget's
//! properties for a master mashup.

use crate::core::scan::escape_attribute;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

const ENTITY_XML: &str = include_str!("templates/mashup.xml");
pub const BLANK: &str = include_str!("templates/blank.content.json");
const MASTER_ROOT: &str = include_str!("templates/master_root.json");

const LAYOUTS: &[&str] = &[
    include_str!("templates/layouts/blank.json"),
    include_str!("templates/layouts/sidebar.json"),
    include_str!("templates/layouts/header-footer.json"),
    include_str!("templates/layouts/header-sidebar.json"),
    include_str!("templates/layouts/sidebar-header.json"),
    include_str!("templates/layouts/form-dialog.json"),
    include_str!("templates/layouts/confirm-dialog.json"),
    include_str!("templates/layouts/list-page.json"),
    include_str!("templates/layouts/card.json"),
    include_str!("templates/layouts/master-detail.json"),
    include_str!("templates/layouts/kpi-tile.json"),
    include_str!("templates/layouts/filter-panel.json"),
    include_str!("templates/layouts/tabs.json"),
    include_str!("templates/layouts/dashboard.json"),
];

/// An embedded responsive layout template.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LayoutTemplate {
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub page: Map<String, Value>,
    #[serde(default)]
    pub master_slot: Option<String>,
    #[serde(default)]
    pub children: Vec<LayoutNode>,
}

/// A slot container in a layout template. Properties not part of the layout vocabulary are
/// Composer properties and are copied without interpretation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LayoutNode {
    #[serde(default)]
    pub slot: Option<String>,
    #[serde(default)]
    pub children: Vec<LayoutNode>,
    #[serde(default)]
    pub widget: Option<String>,
    #[serde(default)]
    pub fixed: Option<String>,
    #[serde(default)]
    pub divider: bool,
    #[serde(flatten)]
    pub properties: Map<String, Value>,
}

/// Parse the fixed, ordered set of templates embedded in this binary.
pub fn layouts() -> Result<Vec<LayoutTemplate>, String> {
    LAYOUTS
        .iter()
        .map(|text| {
            serde_json::from_str(text).map_err(|error| format!("embedded mashup layout: {error}"))
        })
        .collect()
}

/// One template by canonical name or alias.
pub fn layout(word: &str) -> Result<Option<LayoutTemplate>, String> {
    Ok(layouts()?.into_iter().find(|template| {
        template.name == word || template.aliases.iter().any(|alias| alias == word)
    }))
}

/// What kind of mashup the entity says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MashupKind {
    Mashup,
    Master,
}

impl MashupKind {
    /// The entity's `aspect.mashupType`.
    pub const fn aspect(self) -> &'static str {
        match self {
            Self::Mashup => "mashup",
            Self::Master => "target",
        }
    }

    /// The content's `mashupType` and the root widget's `Type`.
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Mashup => "mashup",
            Self::Master => "targetmashup",
        }
    }
}

/// A principal a new mashup is visible to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Visibility {
    /// `Organization` or `OrganizationalUnit`.
    pub kind: String,
    /// `<organization>` or `<organization>:<unit>`.
    pub name: String,
}

/// The entity file for a new mashup, its content still empty.
pub fn entity_xml(
    name: &str,
    project: &str,
    kind: MashupKind,
    visibility: &[Visibility],
) -> String {
    let principals: String = visibility
        .iter()
        .map(|principal| {
            format!(
                "\n                    <Principal\n                     isPermitted=\"true\"\n                     name=\"{}\"\n                     type=\"{}\"></Principal>",
                escape_attribute(&principal.name),
                escape_attribute(&principal.kind)
            )
        })
        .collect();
    let visibility = if principals.is_empty() {
        String::new()
    } else {
        format!("{principals}\n                ")
    };
    let values = BTreeMap::from([
        ("NAME", escape_attribute(name)),
        ("PROJECT", escape_attribute(project)),
        ("MASHUP_TYPE", kind.aspect().to_string()),
        ("VISIBILITY", visibility),
    ]);
    crate::core::newblock::fill(ENTITY_XML, &values)
}

/// The root widget's properties for a master mashup.
pub fn master_root() -> Value {
    serde_json::from_str(MASTER_ROOT).expect("the embedded master root is JSON; a test checks it")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mashup;

    #[test]
    fn a_filled_entity_takes_its_content_from_sync_and_extracts_back_to_it() {
        let xml = entity_xml("Acme.Orders_MU", "Acme", MashupKind::Mashup, &[]);
        assert!(!xml.contains("@@"), "{xml}");
        assert!(xml.contains("name=\"Acme.Orders_MU\""));
        assert!(xml.contains("aspect.mashupType=\"mashup\""));
        assert!(
            xml.contains("<Visibility></Visibility>"),
            "no visibility unless asked"
        );
        let assets = mashup::assets_from_payload(BLANK).unwrap();
        let (written, changed) = mashup::sync(xml.as_bytes(), &assets).unwrap();
        assert_eq!(changed, ["content.json"]);
        assert_eq!(mashup::extract(&written).unwrap(), assets);
        // Nothing outside the content changed.
        let content = |text: &str| {
            let start = text.find("<mashupContent>").unwrap();
            let end = text.find("</mashupContent>").unwrap();
            (text[..start].to_string(), text[end..].to_string())
        };
        assert_eq!(content(&String::from_utf8(written).unwrap()), content(&xml));
    }

    #[test]
    fn visibility_is_written_only_for_the_principals_given_and_escaped() {
        let xml = entity_xml(
            "Acme.Orders_MU",
            "Acme",
            MashupKind::Master,
            &[
                Visibility {
                    kind: "Organization".into(),
                    name: "Acme.Org".into(),
                },
                Visibility {
                    kind: "OrganizationalUnit".into(),
                    name: "Acme.Org:R&D".into(),
                },
            ],
        );
        assert!(xml.contains("aspect.mashupType=\"target\""));
        assert!(xml.contains("name=\"Acme.Org:R&amp;D\"\n                     type=\"OrganizationalUnit\"></Principal>\n                </Visibility>"), "{xml}");
        assert_eq!(xml.matches("<Principal").count(), 2);
        // A value is never searched for placeholders itself.
        let odd = entity_xml("Acme.@@PROJECT@@", "Acme", MashupKind::Mashup, &[]);
        assert!(odd.contains("name=\"Acme.@@PROJECT@@\""));
    }

    #[test]
    fn the_master_root_is_a_responsive_column() {
        let root = master_root();
        assert_eq!(root["Type"], MashupKind::Master.content_type());
        assert_eq!(root["Id"], crate::core::mashup::edit::ROOT_ID);
        assert_eq!(root["flex-direction"], "column");
        assert_eq!(root["StyleTheme"], "PTC Convergence Theme");
    }
}
