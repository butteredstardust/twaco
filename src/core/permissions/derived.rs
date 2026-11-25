//! PermissionGroup rows explain target principals, but never become target writes.

use super::{attribute, elements, error, Grant, KindKey, PermissionsError, Sets};
use crate::core::{config::Solution, entity::carry::Kind, normalise, scan, splice, workspace};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub(crate) struct Derived {
    pub(crate) targets: BTreeMap<(String, String), Sets>,
    pub(crate) repository: BTreeSet<(String, String)>,
    /// Rows that could not be used; each is skipped, never an error.
    pub(crate) notes: Vec<String>,
}

impl Derived {
    pub(crate) fn load(solution: &Solution) -> Result<Self, PermissionsError> {
        Self::read(&workspace::discover(solution).entities)
    }

    pub(crate) fn read(files: &[workspace::EntityFile]) -> Result<Self, PermissionsError> {
        let mut result = Self::default();
        result.repository.extend(
            files
                .iter()
                .map(|f| (f.info.collection.clone(), f.info.name.clone())),
        );
        for file in files
            .iter()
            .filter(|f| f.info.collection == "PermissionGroups")
        {
            let bytes =
                std::fs::read(&file.path).map_err(|e| error(format!("{}: {e}", file.info.name)))?;
            result.add(&bytes);
        }
        Ok(result)
    }

    /// One line for commands that have no place for the notes themselves.
    pub(crate) fn notice(&self) -> Option<String> {
        (!self.notes.is_empty()).then(|| {
            format!(
                "{} PermissionGroup row(s) were ignored, so their grants are not expected on targets; run `twaco permissions diff --all` for details",
                self.notes.len()
            )
        })
    }

    pub(crate) fn add(&mut self, bytes: &[u8]) {
        let group = match normalise::entity_of(bytes) {
            Ok(group) => group,
            Err(e) => {
                self.notes
                    .push(format!("a PermissionGroup is unreadable: {e}"));
                return;
            }
        };
        let Some(principal) = attribute(&group, "name") else {
            self.notes.push("a PermissionGroup has no name".into());
            return;
        };
        let mut seen: BTreeMap<(String, String, KindKey, Grant), bool> = BTreeMap::new();
        let mut conflicted = BTreeSet::new();
        for list in elements(&group).filter(|e| e.name == b"EntityPermissions") {
            for row in elements(list).filter(|e| e.name == b"EntityPermission") {
                let required = |key| attribute(row, key).filter(|v| !v.is_empty());
                let (Some(entity_type), Some(name), Some(category)) = (
                    required("entityType"),
                    required("entityName"),
                    required("permissionCategory"),
                ) else {
                    self.notes.push(format!(
                        "PermissionGroup {principal}: a row lacks entityType, entityName or permissionCategory"
                    ));
                    continue;
                };
                let collection = collection(entity_type);
                let instances = attribute(row, "onInstances") == Some("true")
                    && matches!(collection.as_str(), "ThingTemplates" | "ThingShapes");
                let kind = match (category, instances) {
                    ("RunTime", false) => Kind::RunTime,
                    ("RunTime", true) => Kind::InstanceRunTime,
                    ("DesignTime", false) => Kind::DesignTime,
                    ("DesignTime", true) => Kind::InstanceDesignTime,
                    ("Visibility", false) => Kind::Visibility,
                    ("Visibility", true) => Kind::InstanceVisibility,
                    (category, _) => {
                        self.notes.push(format!(
                            "PermissionGroup {principal}: unknown permissionCategory {category}"
                        ));
                        continue;
                    }
                };
                let action = if matches!(kind, Kind::Visibility | Kind::InstanceVisibility) {
                    "Visibility"
                } else if let Some(action) = required("permissionName") {
                    action
                } else {
                    self.notes.push(format!(
                        "PermissionGroup {principal}: a row for {name} lacks permissionName"
                    ));
                    continue;
                };
                let resource = attribute(row, "resourceName").unwrap_or_default();
                let grant = Grant {
                    resource: if kind.is_run_time() {
                        if resource.is_empty() {
                            "*"
                        } else {
                            resource
                        }
                    } else {
                        ""
                    }
                    .into(),
                    action: action.into(),
                    principal: principal.into(),
                    principal_type: "PermissionGroup".into(),
                };
                let permitted = attribute(row, "isPermitted") != Some("false");
                let key = (collection, name.to_string(), KindKey::of(kind), grant);
                if seen
                    .insert(key.clone(), permitted)
                    .is_some_and(|old| old != permitted)
                {
                    self.notes.push(format!(
                        "PermissionGroup {principal}: conflicting rows for {}; both ignored",
                        key.3
                    ));
                    conflicted.insert(key);
                }
            }
        }
        for ((collection, name, kind, grant), permitted) in seen {
            if conflicted.contains(&(collection.clone(), name.clone(), kind, grant.clone())) {
                continue;
            }
            self.targets
                .entry((collection, name))
                .or_default()
                .entry(kind)
                .or_default()
                .insert(grant, permitted);
        }
    }

    pub(crate) fn target(&self, collection: &str, name: &str) -> Sets {
        self.targets
            .get(&(collection.into(), name.into()))
            .cloned()
            .unwrap_or_default()
    }
}

fn collection(kind: &str) -> String {
    match kind {
        "MediaEntity" => "MediaEntities".into(),
        kind if workspace::KNOWN_COLLECTIONS.contains(&kind) => kind.into(),
        kind => format!("{kind}s"),
    }
}

/// Comparison bytes only. Exact explained principals are omitted on both sides; caller retains
/// the original bytes for every import and permission write. Unexpected allow/deny remains drift.
pub(crate) fn comparison(
    bytes: &[u8],
    expected: &Sets,
) -> Result<Vec<u8>, normalise::NormaliseError> {
    strip(bytes, expected, false)
}

/// Server bytes for the repository: the principals the PermissionGroups explain are dropped
/// with their line, every other principal stays, so a file never holds a derived grant.
pub(crate) fn without_explained(
    bytes: &[u8],
    derived: &Derived,
    collection: &str,
    name: &str,
) -> Result<Vec<u8>, normalise::NormaliseError> {
    strip(bytes, &derived.target(collection, name), true)
}

fn strip(
    bytes: &[u8],
    expected: &Sets,
    whole_line: bool,
) -> Result<Vec<u8>, normalise::NormaliseError> {
    if expected.is_empty() {
        return Ok(bytes.to_vec());
    }
    let entity = normalise::entity_of(bytes)?;
    let tokens = scan::tokenize(bytes)?;
    let mut edits = Vec::new();
    for block in elements(&entity) {
        let Some(kind) = Kind::of_element(&block.name) else {
            continue;
        };
        let Some(grants) = expected.get(&KindKey::of(kind)) else {
            continue;
        };
        let resources: Vec<_> = if kind.is_run_time() {
            elements(block)
                .map(|e| (attribute(e, "resourceName").unwrap_or("*"), e))
                .collect()
        } else {
            vec![("", block)]
        };
        for (resource, container) in resources {
            for action in elements(container) {
                for principal in elements(action).filter(|e| e.name == b"Principal") {
                    let grant = Grant {
                        resource: resource.into(),
                        action: String::from_utf8_lossy(&action.name).into_owned(),
                        principal: attribute(principal, "name").unwrap_or_default().into(),
                        principal_type: attribute(principal, "type").unwrap_or_default().into(),
                    };
                    if grants.get(&grant)
                        == Some(&(attribute(principal, "isPermitted") != Some("false")))
                    {
                        if let Some(index) = tokens
                            .iter()
                            .position(|t| t.span.start == principal.source_start)
                        {
                            if let Some(mut span) = scan::element_span(&tokens, index) {
                                if whole_line {
                                    let mut start = span.start;
                                    while start > 0 && matches!(bytes[start - 1], b' ' | 9) {
                                        start -= 1;
                                    }
                                    if start > 0 && bytes[start - 1] == 10 {
                                        start -= 1;
                                        if start > 0 && bytes[start - 1] == 13 {
                                            start -= 1;
                                        }
                                        span = scan::Span {
                                            start,
                                            end: span.end,
                                        };
                                    }
                                }
                                edits.push(splice::Edit::new(span, Vec::new()));
                            }
                        }
                    }
                }
            }
        }
    }
    splice::splice(bytes, &edits).map_err(|e| normalise::NormaliseError::Malformed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_map_collections_resources_denials_and_instance_sets() {
        let mut derived = Derived::default();
        derived.add(br#"<PermissionGroup name="Acme.Access"><EntityPermissions>
            <EntityPermission entityType="Thing" entityName="Acme.Target" permissionCategory="RunTime" permissionName="PropertyRead" resourceName="" isPermitted="false" onInstances="false"/>
            <EntityPermission entityType="Subsystem" entityName="PlatformSubsystem" permissionCategory="Visibility" permissionName="Visibility" resourceName="*" isPermitted="true" onInstances="false"/>
            <EntityPermission entityType="ThingTemplate" entityName="Acme.Template" permissionCategory="RunTime" permissionName="ServiceInvoke" resourceName="Run" isPermitted="true" onInstances="true"/>
            <EntityPermission entityType="ThingTemplate" entityName="Acme.Template" permissionCategory="RunTime" permissionName="PropertyRead" resourceName="*" isPermitted="true" onInstances="false"/>
            <EntityPermission entityType="ThingShape" entityName="Acme.Shape" permissionCategory="RunTime" permissionName="ServiceInvoke" resourceName="Run" isPermitted="true" onInstances="true"/>
        </EntityPermissions></PermissionGroup>"#);
        let thing = derived.target("Things", "Acme.Target");
        let (grant, permitted) = thing[&KindKey::of(Kind::RunTime)]
            .first_key_value()
            .unwrap();
        assert_eq!(grant.resource, "*");
        assert!(!permitted);
        assert_eq!(grant.principal, "Acme.Access");
        let subsystem = derived.target("Subsystems", "PlatformSubsystem");
        assert_eq!(
            subsystem[&KindKey::of(Kind::Visibility)]
                .first_key_value()
                .unwrap()
                .0
                .resource,
            ""
        );
        let template = derived.target("ThingTemplates", "Acme.Template");
        assert!(template.contains_key(&KindKey::of(Kind::RunTime)));
        assert!(template.contains_key(&KindKey::of(Kind::InstanceRunTime)));
        assert!(derived
            .target("ThingShapes", "Acme.Shape")
            .contains_key(&KindKey::of(Kind::InstanceRunTime)));
    }

    #[test]
    fn comparison_omits_only_exact_explained_principals_and_never_changes_input() {
        let mut derived = Derived::default();
        derived.add(br#"<PermissionGroup name="Acme.Access"><EntityPermissions><EntityPermission entityType="Thing" entityName="Acme.Target" permissionCategory="RunTime" permissionName="ServiceInvoke" resourceName="Run" isPermitted="true" onInstances="false"/></EntityPermissions></PermissionGroup>"#);
        let grants = derived.target("Things", "Acme.Target");
        let local = br#"<Thing name="Acme.Target"><RunTimePermissions/></Thing>"#;
        let server = br#"<Thing name="Acme.Target"><RunTimePermissions><Permissions resourceName="Run"><ServiceInvoke><Principal type="PermissionGroup" name="Acme.Access" isPermitted="true"></Principal></ServiceInvoke></Permissions></RunTimePermissions></Thing>"#;
        let original = server.to_vec();
        assert_eq!(
            normalise::hash_with_grants(local, &[], &grants).unwrap(),
            normalise::hash_with_grants(server, &[], &grants).unwrap()
        );
        for changed in [
            String::from_utf8_lossy(server).replace("Acme.Access", "Acme.Other"),
            String::from_utf8_lossy(server).replace("true", "false"),
        ] {
            assert_ne!(
                normalise::hash_with_grants(local, &[], &grants).unwrap(),
                normalise::hash_with_grants(changed.as_bytes(), &[], &grants).unwrap()
            );
        }
        assert_eq!(original, server);
        assert_eq!(comparison(local, &grants).unwrap(), local);
    }

    #[test]
    fn malformed_and_conflicting_rows_are_skipped_with_notes() {
        let mut derived = Derived::default();
        derived.add(br#"<PermissionGroup name="Acme.Access"><EntityPermissions>
            <EntityPermission entityType="Thing" entityName="Acme.Target" permissionCategory="RunTime" resourceName="*" isPermitted="true"/>
            <EntityPermission entityType="Thing" permissionCategory="RunTime" permissionName="PropertyRead" isPermitted="true"/>
            <EntityPermission entityType="Thing" entityName="Acme.Target" permissionCategory="Other" permissionName="Read" isPermitted="true"/>
            <EntityPermission entityType="Thing" entityName="Acme.Target" permissionCategory="DesignTime" permissionName="Read" isPermitted="true"/>
            <EntityPermission entityType="Thing" entityName="Acme.Target" permissionCategory="DesignTime" permissionName="Update" isPermitted="true"/>
            <EntityPermission entityType="Thing" entityName="Acme.Target" permissionCategory="DesignTime" permissionName="Update" isPermitted="false"/>
        </EntityPermissions></PermissionGroup>"#);
        assert_eq!(derived.notes.len(), 4, "{:?}", derived.notes);
        let grants = &derived.target("Things", "Acme.Target")[&KindKey::of(Kind::DesignTime)];
        assert_eq!(grants.len(), 1);
        assert_eq!(grants.keys().next().unwrap().action, "Read");
        assert!(derived.notice().unwrap().contains("4 PermissionGroup row"));
    }

    #[test]
    fn without_explained_drops_only_explained_principals_and_their_lines() {
        let mut derived = Derived::default();
        derived.add(br#"<PermissionGroup name="Acme.Access"><EntityPermissions><EntityPermission entityType="Thing" entityName="Acme.Target" permissionCategory="Visibility" permissionName="Visibility" resourceName="" isPermitted="true"/></EntityPermissions></PermissionGroup>"#);
        let server = b"<Thing name=\"Acme.Target\">
  <VisibilityPermissions>
    <Visibility>
      <Principal isPermitted=\"true\" name=\"Acme.Access\" type=\"PermissionGroup\"/>
      <Principal isPermitted=\"true\" name=\"Acme.Other\" type=\"PermissionGroup\"/>
    </Visibility>
  </VisibilityPermissions>
</Thing>
";
        let kept = without_explained(server, &derived, "Things", "Acme.Target").unwrap();
        let kept = String::from_utf8(kept).unwrap();
        assert!(!kept.contains("Acme.Access") && kept.contains("Acme.Other"));
        assert!(
            !kept.contains(
                "

"
            ) && !kept.contains(
                "
      
"
            )
        );
        assert_eq!(
            without_explained(server, &derived, "Things", "Acme.Else").unwrap(),
            server
        );
    }
}
