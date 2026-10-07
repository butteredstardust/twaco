//! What a policy needs to know about one project's entities, read from their XML.

use super::{elements, from_xml, PermissionsError, Sets};
use crate::core::normalise::{self, Element};
use crate::core::workspace::EntityFile;
use std::collections::BTreeSet;

/// The template every Solution Framework permission helper Thing derives from. It ships in the
/// `PTCDTS.Base` project, which depends only on `PTC.Base`, so a server without the Solution
/// Framework can have one.
pub const HELPER_TEMPLATE: &str = "PTCDTS.Base.ComponentPermissionHelper_TT";

/// One entity of the project, with what its permission rules can name.
#[derive(Clone, Debug)]
pub struct ModelEntity {
    pub file: EntityFile,
    /// The XML element name: `Thing`, `ThingShape`, `Mashup`...
    pub entity_type: String,
    /// `thingTemplate` of a Thing; empty otherwise.
    pub thing_template: String,
    pub services: BTreeSet<String>,
    pub properties: BTreeSet<String>,
    pub events: BTreeSet<String>,
    pub sets: Sets,
    /// The organizational units an Organization declares; empty for anything else.
    pub units: BTreeSet<String>,
}

impl ModelEntity {
    pub fn read(file: &EntityFile) -> Result<ModelEntity, PermissionsError> {
        let bytes = std::fs::read(&file.path)
            .map_err(|e| PermissionsError(format!("{}: {e}", file.path.display())))?;
        let at = |e: PermissionsError| PermissionsError(format!("{}: {e}", file.path.display()));
        let sets = from_xml(&bytes).map_err(at)?;
        let entity = normalise::entity_of(&bytes)
            .map_err(|e| PermissionsError(format!("{}: {e}", file.path.display())))?;
        let mut model = ModelEntity {
            file: file.clone(),
            entity_type: String::from_utf8_lossy(&entity.name).into_owned(),
            thing_template: attribute(&entity, "thingTemplate").to_string(),
            services: BTreeSet::new(),
            properties: BTreeSet::new(),
            events: BTreeSet::new(),
            sets,
            units: BTreeSet::new(),
        };
        collect(&entity, &mut model);
        Ok(model)
    }

    /// `Collection/Name`, as every report names an entity.
    pub fn key(&self) -> String {
        format!("{}/{}", self.file.info.collection, self.file.info.name)
    }

    pub fn name(&self) -> &str {
        &self.file.info.name
    }

    /// The resources of one run-time action that the entity itself defines.
    pub fn defined(&self, action: &str) -> &BTreeSet<String> {
        match action {
            "ServiceInvoke" => &self.services,
            "EventInvoke" | "EventSubscribe" => &self.events,
            _ => &self.properties,
        }
    }

    pub fn is_helper(&self) -> bool {
        self.entity_type == "Thing" && self.thing_template == HELPER_TEMPLATE
    }
}

fn attribute<'a>(element: &'a Element, name: &str) -> &'a str {
    super::attribute(element, name).unwrap_or_default()
}

/// Definitions sit in their containers at any depth (a Thing's are under `ThingShape`); nothing
/// inside a permission block or a configuration table is a definition.
fn collect(element: &Element, model: &mut ModelEntity) {
    for child in elements(element) {
        let name = child.name.as_slice();
        if crate::core::entity_carry::Kind::of_element(name).is_some()
            || name == b"ConfigurationTables"
        {
            continue;
        }
        let list = match name {
            b"ServiceDefinitions" => Some((b"ServiceDefinition".as_slice(), 0)),
            b"PropertyDefinitions" => Some((b"PropertyDefinition".as_slice(), 1)),
            b"EventDefinitions" => Some((b"EventDefinition".as_slice(), 2)),
            b"OrganizationalUnits" => Some((b"OrganizationalUnit".as_slice(), 3)),
            _ => None,
        };
        match list {
            Some((item, slot)) => {
                for definition in elements(child).filter(|e| e.name == item) {
                    let value = attribute(definition, "name").to_string();
                    if value.is_empty() {
                        continue;
                    }
                    match slot {
                        0 => model.services.insert(value),
                        1 => model.properties.insert(value),
                        2 => model.events.insert(value),
                        _ => model.units.insert(value),
                    };
                }
            }
            None => collect(child, model),
        }
    }
}
