//! Editing a DataShape field and its managed files together.

use super::{
    append_child, created_sidecar, invalid, remove_child, Edit2, EditError, FileChange,
    FileChangeKind, Plan,
};
use crate::core::index::Confidence;
use crate::core::{accumulated, datashape, datatable, entity, scan, workspace};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

impl Edit2<'_> {
    pub(super) fn field_add(
        &self,
        spec: &str,
        description: Option<&str>,
        data_shape: Option<&str>,
        plan: &mut Plan,
    ) -> Result<(), EditError> {
        let doc = self.doc;
        let fields = self.fields()?;
        let mut field = entity::new::parse_fields(&[spec.to_string()])?.remove(0);
        if fields.iter().any(|existing| existing.name == field.name) {
            return Err(EditError::Exists(format!(
                "{} already defines field {}",
                doc.label(),
                field.name
            )));
        }
        if let Some(shape) = data_shape {
            if field.base_type != "INFOTABLE"
                || shape.is_empty()
                || field.aspects.contains_key("dataShape")
            {
                return Err(invalid(
                    "--data-shape needs an INFOTABLE field without an inline DataShape",
                ));
            }
            field
                .aspects
                .insert("dataShape".into(), datashape::Aspect::Text(shape.into()));
        }
        if let Some(datashape::Aspect::Text(shape)) = field.aspects.get("dataShape") {
            if !self
                .found
                .iter()
                .any(|file| file.info.collection == "DataShapes" && file.info.name == *shape)
            {
                plan.warnings.push(format!(
                    "DataShape {shape} is not in the solution; it must exist on the server"
                ));
            }
        }
        field.description = description.unwrap_or_default().into();
        field.ordinal = fields
            .iter()
            .filter_map(|field| field.ordinal.trim().parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| {
                invalid(format!(
                    "{} cannot allocate another field ordinal",
                    doc.label()
                ))
            })?
            .to_string();
        let xml = entity::new::field_xml(&field)?;
        let after =
            if scan::child_tags(&doc.tokens, &doc.src, "FieldDefinitions", doc.entity).is_empty() {
                let section =
                    format!("            <FieldDefinitions>\n{xml}            </FieldDefinitions>");
                append_child(doc, doc.entity, &section, 12)?
            } else {
                let section = doc.only_child(doc.entity, "FieldDefinitions")?;
                append_child(doc, section, xml.trim_end_matches('\n'), 16)?
            };
        plan.summary = format!("add field {}:{}", field.name, field.base_type);
        let after_fields = datashape::extract(&after).map_err(invalid)?;
        self.field_changes(plan, after, &after_fields)?;
        self.update_accumulated(plan, &after_fields)
    }

    pub(super) fn field_remove(&self, name: &str, plan: &mut Plan) -> Result<(), EditError> {
        let doc = self.doc;
        let mut fields = self.fields()?;
        let Some(index) = fields.iter().position(|field| field.name == name) else {
            return Err(EditError::Unknown(format!(
                "{} has no field {name}",
                doc.label()
            )));
        };
        let removed = fields.remove(index);
        let primary = |field: &datashape::Field| {
            field.aspects.get("isPrimaryKey") == Some(&datashape::Aspect::Bool(true))
        };
        let last_primary = primary(&removed) && !fields.iter().any(primary);
        if last_primary {
            plan.warnings.push(format!("{name} is the last primary key of {}; applying leaves the DataShape with no declared primary key", doc.label()));
        }

        let impact = crate::core::impact::run(
            self.solution,
            &crate::core::impact::Request {
                entity: doc.label(),
                member: Some(name.into()),
                min: Confidence::Review,
                depth: None,
            },
        )
        .map_err(invalid)?;
        let mut references = BTreeSet::new();
        // Only actual storage Things have persisted rows. Other entities can also have a
        // DataShape-typed member named dataShape or a configuration table called Settings.
        let mut storage_shapes: BTreeSet<&str> = BTreeSet::from([doc.name()]);
        let mut stores = BTreeSet::new();
        for dependent in &impact.dependents {
            if dependent
                .path
                .iter()
                .all(|step| step.kind == crate::core::index::EdgeKind::BaseDataShape)
            {
                if let Some(name) = dependent.label.strip_prefix("DataShapes/") {
                    storage_shapes.insert(name);
                }
            }
        }
        for file in self
            .found
            .iter()
            .filter(|file| file.info.collection == "Things")
        {
            let before = std::fs::read(&file.path)
                .map_err(|error| invalid(format!("{}: {error}", file.path.display())))?;
            if datatable::storage_shape(&before)
                .is_some_and(|shape| storage_shapes.contains(shape.as_str()))
            {
                stores.insert(file.info.name.as_str());
            }
        }
        references.extend(
            impact
                .dependents
                .iter()
                .map(|dependent| dependent.label.as_str()),
        );
        if !stores.is_empty() {
            let primary = if last_primary {
                " It is also the last primary key."
            } else {
                ""
            };
            return Err(EditError::InUse(format!(
                "{} field {name} may still be used by rows in {}. Migrate these DataTables or Streams to a different DataShape before removing the field.{primary} References: {}",
                doc.label(), stores.into_iter().collect::<Vec<_>>().join(", "),
                references.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }
        if !references.is_empty() {
            plan.warnings.push(format!(
                "field {name} may be referenced by {}",
                references.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        if !impact.complete {
            plan.warnings
                .push("Some repository inputs could not be read; see impact for details.".into());
        }
        let section = doc.only_child(doc.entity, "FieldDefinitions")?;
        let element = doc
            .named(section, "FieldDefinition", name)
            .expect("field came from this section");
        let after = remove_child(doc, section, element)?;
        plan.summary = format!("remove field {name}");
        let after_fields = datashape::extract(&after).map_err(invalid)?;
        self.field_changes(plan, after, &after_fields)
    }

    fn fields(&self) -> Result<Vec<datashape::Field>, EditError> {
        self.doc
            .require(&["DataShapes"], "fields belong to a DataShape")?;
        let fields = datashape::extract(&self.doc.src).map_err(invalid)?;
        let mut names = BTreeSet::new();
        if fields.iter().any(|field| !names.insert(&field.name)) {
            return Err(invalid(format!(
                "{} has duplicate field definitions",
                self.doc.label()
            )));
        }
        Ok(fields)
    }

    fn field_changes(
        &self,
        plan: &mut Plan,
        after: Vec<u8>,
        fields: &[datashape::Field],
    ) -> Result<(), EditError> {
        let path = workspace::fields_path(self.solution, &self.doc.file);
        let before = read_optional(&path)?;
        if let Some(bytes) = &before {
            let text = sidecar_text(&path, bytes)?;
            let desired = datashape::from_sidecar(text)
                .map_err(|error| invalid(format!("{}: {error}", path.display())))?;
            if desired != datashape::extract(&self.doc.src).map_err(invalid)? {
                return Err(invalid(format!(
                    "{} has changes pending; sync the DataShape first",
                    path.display()
                )));
            }
        }
        plan.changes.push(self.doc.change(after));
        let after = datashape::to_sidecar(fields).into_bytes();
        plan.changes
            .push(sidecar_change(path, self.doc.label(), before, after));
        Ok(())
    }

    fn update_accumulated(
        &self,
        plan: &mut Plan,
        fields: &[datashape::Field],
    ) -> Result<(), EditError> {
        for file in self
            .found
            .iter()
            .filter(|file| file.info.collection == "Things")
        {
            let before = std::fs::read(&file.path)
                .map_err(|error| invalid(format!("{}: {error}", file.path.display())))?;
            let Some((shape, old)) = datatable::shape_and_accumulated(&before) else {
                continue;
            };
            if shape != self.doc.name() {
                continue;
            }
            let Some(merged) = accumulated::merge(&old, fields) else {
                continue;
            };
            let after = datatable::write_accumulated(&before, &merged).map_err(invalid)?;
            let label = format!("Things/{}", file.info.name);
            if datatable::is_data_table(&before) {
                let path = workspace::datatable_path(self.solution, file);
                if let Some(bytes) = read_optional(&path)? {
                    let desired = datatable::from_sidecar(sidecar_text(&path, &bytes)?)
                        .map_err(|error| invalid(format!("{}: {error}", path.display())))?;
                    if datatable::sync(&before, &desired).map_err(invalid)?.0 != before {
                        return Err(invalid(format!(
                            "{} has changes pending; sync the DataTable first",
                            path.display()
                        )));
                    }
                    let config = datatable::extract(&after).map_err(invalid)?;
                    let text = datatable::to_sidecar(&config).map_err(invalid)?;
                    plan.changes.push(sidecar_change(
                        path,
                        label.clone(),
                        Some(bytes),
                        text.into_bytes(),
                    ));
                }
            }
            plan.changes.push(FileChange {
                path: file.path.clone(),
                entity: label,
                before,
                after,
                kind: FileChangeKind::Replace,
            });
        }
        Ok(())
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, EditError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(invalid(format!("{}: {error}", path.display()))),
    }
}

fn sidecar_text<'a>(path: &Path, bytes: &'a [u8]) -> Result<&'a str, EditError> {
    std::str::from_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))
        .map_err(|error| invalid(format!("{}: {error}", path.display())))
}

fn sidecar_change(
    path: PathBuf,
    entity: String,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
) -> FileChange {
    match before {
        Some(before) => {
            let after = if before.starts_with(b"\xef\xbb\xbf") {
                [b"\xef\xbb\xbf".as_slice(), &after].concat()
            } else {
                after
            };
            FileChange {
                path,
                entity,
                before,
                after,
                kind: FileChangeKind::Replace,
            }
        }
        None => created_sidecar(path, entity, after),
    }
}
