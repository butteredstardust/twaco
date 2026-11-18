//! `menu nest`: a root Menu of sub-Menu links and one leaf Menu per group, written together.
//!
//! The server shows a sub-Menu link as the sub-Menu's label and Menu-level icon, so a group is a
//! Menu of its own. Each Menu nest names is written whole from the groups: its items are the
//! groups' (a leaf keeps the image and default of an item it already had under the same title)
//! and its description is kept. A Menu whose file already matches is left alone, so a rerun
//! changes nothing and a changed group changes only its own file.

use super::{
    icon_reference, invalid, menu_entries, menu_item_insert, menu_reference, remove_child,
    set_attribute, splice_all, with_rows, Doc, Edit, EditError, Entry, FileChange, FileChangeKind,
    MenuReferences, Plan,
};
use crate::core::entity::new::{self, Kind, MenuEntry};
use crate::core::scan;
use std::path::PathBuf;

/// One group of a nested menu: a label, a Menu-level icon and its Mashup items in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NestGroup {
    pub label: String,
    pub icon: String,
    pub items: Vec<(String, String)>,
}

impl NestGroup {
    /// `Label=<icon>:Title=Mashup,Title=Mashup`; the icon is a `cds:` name, a MediaEntity or empty.
    pub fn parse(text: &str) -> Result<NestGroup, EditError> {
        let usage = || {
            invalid(format!(
                "--group {text:?} must be Label=<icon>:Title=Mashup,Title=Mashup (icon: cds:icon_<name>, a MediaEntity, or empty)"
            ))
        };
        let (label, rest) = text.split_once('=').ok_or_else(usage)?;
        let split = if let Some(name) = rest.strip_prefix("cds:") {
            name.find(':').map(|at| at + 4)
        } else {
            rest.find(':')
        }
        .ok_or_else(usage)?;
        let (icon, list) = (&rest[..split], &rest[split + 1..]);
        let items = list
            .split(',')
            .map(|item| {
                item.split_once('=')
                    .filter(|(title, mashup)| !title.is_empty() && !mashup.is_empty())
                    .map(|(title, mashup)| (title.to_string(), mashup.to_string()))
                    .ok_or_else(usage)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if label.is_empty() {
            return Err(usage());
        }
        Ok(NestGroup {
            label: label.into(),
            icon: icon.into(),
            items,
        })
    }

    /// The group's Menu name: the root's without `_MN`, the label's words joined, then `_MN`.
    fn menu_name(&self, root: &str) -> Result<String, EditError> {
        let slug: String = self
            .label
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(|word| {
                let (first, rest) = word.split_at(1);
                format!("{}{rest}", first.to_uppercase())
            })
            .collect();
        if slug.is_empty() {
            return Err(invalid(format!(
                "group {:?} has no ASCII letter or digit to name its Menu from; use a label with one",
                self.label
            )));
        }
        Ok(format!(
            "{}_{slug}_MN",
            root.strip_suffix("_MN").unwrap_or(root)
        ))
    }
}

/// A Menu nest writes: where it is or goes, and what of it already exists.
struct Target {
    name: String,
    path: PathBuf,
    project: String,
    existing: Option<Existing>,
}

/// An existing Menu's attributes and items: title, destination, image, default, sub-Menu link.
struct Existing {
    doc: Doc,
    description: String,
    label: String,
    image: String,
    items: Vec<(String, String, String, bool, bool)>,
}

fn locate(
    found: &MenuReferences<'_>,
    name: &str,
    project: &str,
    root: &std::path::Path,
) -> Result<Target, EditError> {
    let file = found
        .discovered
        .entities
        .iter()
        .find(|file| file.info.collection == "Menus" && file.info.name == name);
    let Some(file) = file else {
        return Ok(Target {
            name: name.into(),
            path: root.join("Menus").join(format!("{name}.xml")),
            project: project.into(),
            existing: None,
        });
    };
    let doc = Doc::read(file)?;
    let (_, items) = menu_entries(&doc)?;
    Ok(Target {
        name: name.into(),
        path: file.path.clone(),
        project: file.info.project.clone(),
        existing: Some(Existing {
            description: doc.attr(doc.entity, "description").unwrap_or_default(),
            label: doc.attr(doc.entity, "menuLabel").unwrap_or_default(),
            image: doc.attr(doc.entity, "imageURL").unwrap_or_default(),
            items: items
                .into_iter()
                .map(|item| {
                    (
                        item.title,
                        item.target,
                        item.image_url,
                        item.is_default,
                        item.link_type == "Menu",
                    )
                })
                .collect(),
            doc,
        }),
    })
}

pub(super) fn plan(
    found: &MenuReferences<'_>,
    root: &str,
    project: Option<&str>,
    groups: &[NestGroup],
) -> Result<Plan, EditError> {
    let root = root.strip_prefix("Menus/").unwrap_or(root);
    crate::core::refs::validate_new_name(root).map_err(invalid)?;
    if groups.is_empty() {
        return Err(invalid("menu nest needs at least one --group"));
    }
    let mut plan = Plan::new(format!("Menus/{root}"));
    let solution = found.solution;
    let (project_name, project_root) = match found
        .discovered
        .entities
        .iter()
        .find(|file| file.info.collection == "Menus" && file.info.name == root)
    {
        Some(file) => {
            let declared = solution.project(&file.info.project).ok_or_else(|| {
                EditError::Unknown(format!(
                    "twaco.toml declares no project {}",
                    file.info.project
                ))
            })?;
            (declared.name.clone(), solution.project_root(declared))
        }
        None => {
            let chosen = new::project(solution, &project.map(String::from), Kind::Menu)?;
            (chosen.name.clone(), solution.project_root(chosen))
        }
    };
    let mut names: Vec<String> = Vec::new();
    for group in groups {
        let name = group.menu_name(root)?;
        crate::core::refs::validate_new_name(&name).map_err(invalid)?;
        if name == root || names.contains(&name) {
            return Err(invalid(format!(
                "group {:?} gives the Menu name {name} twice; name the groups apart",
                group.label
            )));
        }
        if group.items.is_empty() {
            return Err(invalid(format!("group {:?} has no items", group.label)));
        }
        icon_reference(found, &group.icon, &mut plan)?;
        for (index, (title, mashup)) in group.items.iter().enumerate() {
            if group.items[..index].iter().any(|(other, _)| other == title) {
                return Err(invalid(format!(
                    "group {:?} lists the item {title} twice",
                    group.label
                )));
            }
            menu_reference(found, "Mashups", mashup, &mut plan)?;
        }
        names.push(name);
    }
    let mut written = 0;
    for (group, name) in groups.iter().zip(&names) {
        let target = locate(found, name, &project_name, &project_root)?;
        let kept = target.existing.as_ref().map(|existing| &existing.items[..]);
        if let Some(link) = kept.into_iter().flatten().find(|item| item.4) {
            return Err(invalid(format!(
                "Menus/{name} already links the sub-Menu {}; menu nest writes a leaf group, so remove the link first",
                link.1
            )));
        }
        let entries: Vec<MenuEntry> = group
            .items
            .iter()
            .map(|(title, mashup)| {
                let old = kept
                    .into_iter()
                    .flatten()
                    .find(|item| item.0 == *title && item.1 == *mashup);
                MenuEntry {
                    title: title.clone(),
                    target: mashup.clone(),
                    sub_menu: false,
                    image: old.map(|item| item.2.clone()).unwrap_or_default(),
                    is_default: old.is_some_and(|item| item.3),
                }
            })
            .collect();
        let description = target
            .existing
            .as_ref()
            .map(|existing| existing.description.clone())
            .unwrap_or_default();
        written += write(
            found,
            &target,
            &entries,
            (&group.label, &group.icon, &description),
            &mut plan,
        )?;
    }
    let target = locate(found, root, &project_name, &project_root)?;
    if let Some(existing) = &target.existing {
        if let Some(item) = existing.items.iter().find(|item| !item.4) {
            return Err(invalid(format!(
                "Menus/{root} has the item {}; menu nest keeps only sub-Menu links in the root, so remove it with menu item remove first",
                item.0
            )));
        }
        for item in existing
            .items
            .iter()
            .filter(|item| !names.contains(&item.1))
        {
            plan.warnings
                .push(format!("Menus/{root}: the link to {} is dropped", item.1));
        }
    }
    let entries: Vec<MenuEntry> = names
        .iter()
        .map(|name| MenuEntry {
            title: String::new(),
            target: name.clone(),
            sub_menu: true,
            image: String::new(),
            is_default: false,
        })
        .collect();
    let (label, image, description) = target
        .existing
        .as_ref()
        .map(|e| (e.label.clone(), e.image.clone(), e.description.clone()))
        .unwrap_or_default();
    written += write(
        found,
        &target,
        &entries,
        (&label, &image, &description),
        &mut plan,
    )?;
    plan.summary = if written == 0 {
        format!(
            "nest {} groups under {root}: already as given",
            groups.len()
        )
    } else {
        format!(
            "nest {} groups under {root}: {written} Menu files",
            groups.len()
        )
    };
    Ok(plan)
}

/// Add the change that makes `target` hold `entries`, unless it does already. Returns how many
/// files it adds. An existing Menu is edited in place: only the items, the label and the icon
/// change, and a kept item keeps every attribute.
fn write(
    found: &MenuReferences<'_>,
    target: &Target,
    entries: &[MenuEntry],
    (label, image, description): (&str, &str, &str),
    plan: &mut Plan,
) -> Result<usize, EditError> {
    match &target.existing {
        Some(existing) => {
            let after = in_place(existing, &target.name, entries, label, image, plan)?;
            if after == existing.doc.src {
                return Ok(0);
            }
            plan.changes.push(existing.doc.change(after));
        }
        None => {
            let xml = new::menu_entity_xml(
                &target.name,
                &target.project,
                description,
                label,
                image,
                entries,
            )?;
            crate::core::transaction::check_inside(&found.solution.root, &target.path)
                .map_err(invalid)?;
            plan.changes.push(FileChange {
                path: target.path.clone(),
                entity: format!("Menus/{}", target.name),
                before: Vec::new(),
                after: xml.into_bytes(),
                kind: FileChangeKind::Create,
            });
        }
    }
    Ok(1)
}

/// Whether `item` is one of `entries`: a sub-Menu link by its target, an item by title and target.
fn wanted(entries: &[MenuEntry], item: &Entry) -> bool {
    entries.iter().any(|entry| {
        if entry.sub_menu {
            item.link_type == "Menu" && item.link_target == entry.target
        } else {
            item.title == entry.title && item.target == entry.target
        }
    })
}

fn in_place(
    existing: &Existing,
    name: &str,
    entries: &[MenuEntry],
    label: &str,
    image: &str,
    plan: &mut Plan,
) -> Result<Vec<u8>, EditError> {
    let file = existing.doc.file.clone();
    let mut doc = Doc::parse(file.clone(), existing.doc.src.clone())?;
    let mut edits = Vec::new();
    set_attribute(&doc, doc.entity, "menuLabel", label, &mut edits)?;
    set_attribute(&doc, doc.entity, "imageURL", image, &mut edits)?;
    if !edits.is_empty() {
        let effective = doc.only_child(doc.entity, "effectiveMenu")?;
        let rows = doc.only_child(effective, "Rows")?;
        let root = *scan::child_tags(&doc.tokens, &doc.src, "Row", rows)
            .first()
            .ok_or_else(|| invalid(format!("{} effectiveMenu has no root row", doc.label())))?;
        for (tag, text) in [("title", label), ("imageURL", image)] {
            let element = doc.only_child(root, tag)?;
            let xml = if text.is_empty() {
                String::new()
            } else {
                new::cdata(text)
            };
            edits.push(Edit::new(doc.inner(element)?, xml.into_bytes()));
        }
        doc = Doc::parse(file.clone(), splice_all(&doc.src, edits)?)?;
    }
    loop {
        let (list, items) = menu_entries(&doc)?;
        let Some(item) = items.iter().find(|item| !wanted(entries, item)) else {
            break;
        };
        if item.link_type != "Menu" {
            plan.warnings
                .push(format!("Menus/{name}: item '{}' is dropped", item.title));
        }
        let removed = remove_child(&doc, list, item.element)?;
        doc = Doc::parse(file.clone(), with_rows(&doc, removed)?)?;
    }
    for (at, entry) in entries.iter().enumerate() {
        let (_, items) = menu_entries(&doc)?;
        if items.iter().any(|item| {
            if entry.sub_menu {
                item.link_type == "Menu" && item.link_target == entry.target
            } else {
                item.title == entry.title && item.target == entry.target
            }
        }) {
            continue;
        }
        let before = entries[at + 1..]
            .iter()
            .find(|next| !next.sub_menu && items.iter().any(|item| item.title == next.title))
            .map(|next| next.title.as_str());
        let mut scratch = Plan::new(String::new());
        let (title, mashup, submenu) = if entry.sub_menu {
            (String::new(), String::new(), Some(entry.target.as_str()))
        } else {
            (entry.title.clone(), entry.target.clone(), None)
        };
        menu_item_insert(
            &doc,
            (title, mashup, entry.is_default),
            before,
            submenu,
            (!entry.image.is_empty()).then_some(entry.image.as_str()),
            &mut scratch,
        )?;
        let change = scratch
            .changes
            .pop()
            .expect("an added item changes the Menu");
        doc = Doc::parse(file.clone(), change.after)?;
    }
    Ok(doc.src)
}
