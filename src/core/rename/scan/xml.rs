use super::findings::{excerpt, span_text, Finding, Place, XmlPass};
use crate::core::{refs, scan, splice};

/// Finds and plans a name replacement in one UTF-8 XML document.
///
/// Attributes and plain text are values and use `refs::find`; CDATA is a blob and uses
/// `refs::find_in_text`. Only Exact and Embedded hits become edits. The scanner's malformed or
/// unsupported-input errors are returned unchanged. This function refuses to decode entities,
/// inspect comments or processing instructions, perform file I/O, or rebuild markup.
pub fn scan_xml(
    src: &[u8],
    name: &str,
    mode: refs::Mode,
    new: &str,
) -> Result<XmlPass, scan::ScanError> {
    scan_xml_with_identity(src, name, mode, new, true, None)
}

/// The same structural pass, leaving another entity's own identity untouched.
pub(crate) fn scan_xml_with_identity(
    src: &[u8],
    name: &str,
    mode: refs::Mode,
    new: &str,
    rename_identity: bool,
    collection: Option<&str>,
) -> Result<XmlPass, scan::ScanError> {
    debug_assert!(
        refs::validate_entity_name(new).is_ok(),
        "scan_xml requires a validated replacement name"
    );
    let tokens = scan::tokenize(src)?;
    let mut elements = Vec::<String>::new();
    let mut pass = XmlPass::new(src);

    for token in &tokens {
        match token.kind {
            scan::Kind::Start | scan::Kind::Empty => {
                let element = span_text(src, token.name).to_owned();
                for attribute in scan::attributes(src, token)? {
                    let attribute_name = span_text(src, attribute.name).to_owned();
                    if mode == refs::Mode::Entity
                        && collection.is_some_and(|target| {
                            attribute_collection(&element, &attribute_name)
                                .is_some_and(|owner| owner != target)
                        })
                    {
                        continue;
                    }
                    // `Entities > <Collection> > <Entity name=...>`: the root element's name is the
                    // entity's own name, wherever the entity is of a kind this list has never heard of.
                    let place = if attribute_name == "name"
                        && elements.len() == 2
                        && elements.first().is_some_and(|root| root == "Entities")
                    {
                        if !rename_identity {
                            continue;
                        }
                        Place::EntityName {
                            element: element.clone(),
                        }
                    } else {
                        Place::Attribute {
                            element: element.clone(),
                            attribute: attribute_name,
                        }
                    };
                    inspect(
                        src,
                        attribute.value,
                        name,
                        mode,
                        new,
                        place,
                        false,
                        collection,
                        &mut pass,
                    );
                }
                if token.kind == scan::Kind::Start {
                    elements.push(element);
                }
            }
            scan::Kind::End => {
                // The scanner tokenises tags without pairing them; a text node's owner is only
                // trustworthy when every end tag closes the element that is open.
                match elements.pop() {
                    Some(open) if open == span_text(src, token.name) => {}
                    _ => {
                        return Err(scan::ScanError::Malformed {
                            what: "end tag that does not close the open element",
                            at: token.span.start,
                        })
                    }
                }
            }
            scan::Kind::Text => {
                let value = span_text(src, token.span);
                if !value.trim().is_empty() {
                    if let Some(element) = elements.last() {
                        inspect(
                            src,
                            token.span,
                            name,
                            mode,
                            new,
                            Place::Text {
                                element: element.clone(),
                            },
                            false,
                            collection,
                            &mut pass,
                        );
                    }
                }
            }
            scan::Kind::Cdata => {
                if let Some(element) = elements.last() {
                    inspect(
                        src,
                        token.inner,
                        name,
                        mode,
                        new,
                        Place::Cdata {
                            element: element.clone(),
                        },
                        true,
                        collection,
                        &mut pass,
                    );
                }
            }
            scan::Kind::Pi | scan::Kind::Comment | scan::Kind::DocType => {}
        }
    }
    Ok(pass)
}

/// Finds and plans a name replacement in one non-XML UTF-8 file.
///
/// The whole file is treated as a text blob, so quoted values and qualified embedded names use
/// the same rules as script CDATA. Non-UTF-8 input is refused and no file I/O is performed.
#[cfg(test)]
pub(crate) fn scan_text(
    src: &[u8],
    name: &str,
    mode: refs::Mode,
    new: &str,
) -> Result<XmlPass, std::str::Utf8Error> {
    debug_assert!(
        refs::validate_entity_name(new).is_ok(),
        "scan_text requires a validated replacement name"
    );
    scan_text_with(src, name, mode, new, None)
}

/// What an entity rename knows about the names that continue past the entity's own:
/// `Old.Child` that is another entity is that entity, and `Old.Member` names a member of the
/// renamed one.
#[derive(Debug, Default, Clone)]
pub(crate) struct Qualified {
    /// Every entity of the solution.
    pub(crate) entities: std::collections::BTreeSet<String>,
    /// The renamed entity's services, inherited ones included, and its own properties and events.
    pub(crate) members: std::collections::BTreeSet<String>,
    pub(crate) collection: Option<String>,
}

/// `scan_text`, and with `qualified` also `Old.Member`: an entity mode hit that the name
/// boundary refuses because more of a dotted name follows. That is another entity's name when
/// one has it, which is left alone. Otherwise a qualified name followed by a member of the
/// renamed entity, as in `Acme.Orders.Manager.GetOrder`, is a reference and is renamed; any other
/// continuation is left for a person. An unqualified name is only reported, and only before one
/// of its members, since `Node.js` is no reference to an entity named `Node`.
pub(crate) fn scan_text_with(
    src: &[u8],
    name: &str,
    mode: refs::Mode,
    new: &str,
    qualified: Option<&Qualified>,
) -> Result<XmlPass, std::str::Utf8Error> {
    debug_assert!(
        refs::validate_entity_name(new).is_ok(),
        "scan_text requires a validated replacement name"
    );
    let text = std::str::from_utf8(src)?;
    let mut pass = XmlPass::new(src);
    inspect(
        src,
        scan::Span::new(0, src.len()),
        name,
        mode,
        new,
        Place::File,
        true,
        None,
        &mut pass,
    );
    if let (refs::Mode::Entity, Some(qualified), false) = (mode, qualified, name.is_empty()) {
        for (start, _) in text.match_indices(name) {
            let end = start + name.len();
            if !refs::before_is_boundary(&text[..start]) {
                continue;
            }
            let Some(tail) = text[end..].strip_prefix('.') else {
                continue;
            };
            let length = tail
                .find(|c: char| !refs::is_name_char(c) && c != '.')
                .unwrap_or(tail.len());
            let tail = tail[..length].trim_end_matches('.');
            if tail.is_empty() || !tail.starts_with(refs::is_name_char) {
                continue;
            }
            let full = format!("{name}.{tail}");
            // Only an entity whose name extends the renamed one can be what the text names.
            let another_entity = qualified.entities.iter().any(|entity| {
                entity
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('.'))
                    && (full == *entity
                        || full
                            .strip_prefix(entity.as_str())
                            .is_some_and(|rest| rest.starts_with('.')))
            });
            if another_entity {
                continue;
            }
            let member = tail.split('.').next().unwrap_or(tail);
            let known = qualified.members.contains(member);
            let tier = match (refs::is_qualified(name), known) {
                (true, true) => refs::Tier::Embedded,
                (true, false) | (false, true) => refs::Tier::Review,
                (false, false) => continue,
            };
            let applied = tier == refs::Tier::Embedded;
            pass.findings.push(Finding {
                place: Place::File,
                tier,
                line: pass.line_of(start),
                excerpt: excerpt(text, start, true),
                applied,
            });
            if applied {
                pass.edits.push(splice::Edit::new(
                    scan::Span::new(start, end),
                    new.as_bytes().to_vec(),
                ));
            }
        }
        pass.findings.sort_by_key(|finding| finding.line);
    }
    Ok(pass)
}

#[allow(clippy::too_many_arguments)]
fn inspect(
    src: &[u8],
    span: scan::Span,
    name: &str,
    mode: refs::Mode,
    new: &str,
    place: Place,
    blob: bool,
    collection: Option<&str>,
    pass: &mut XmlPass,
) {
    if mode == refs::Mode::Entity
        && (matches!(&place, Place::Script)
            || matches!(&place, Place::Cdata { element } if element == "code"))
    {
        let dbinfo = crate::core::dbinfo::scan_entity(src)
            .is_ok_and(|scripts| scripts.iter().any(|(offset, _)| *offset == span.start));
        inspect_script(src, span, name, new, dbinfo, collection, pass);
        return;
    }
    let value = span_text(src, span);
    let hits = if blob {
        refs::find_in_text(value, name, mode)
    } else {
        refs::find(value, name, mode)
    };
    for hit in hits {
        let absolute = scan::Span::new(span.start + hit.start, span.start + hit.end);
        let mut tier = hit.tier;
        // An unqualified name can be a common word (`Node`, `Network`): a value or a quoted string
        // that merely equals it is not evidence of a reference. Only the places that are known to
        // name an entity apply; everything else is a finding for a person.
        if mode == refs::Mode::Entity && !refs::is_qualified(name) && !name.contains(' ') {
            if follows_derived_prefix(value, hit.start) {
                // `Things_<name>` is an id built from the entity's name, inside a larger blob or not.
                tier = refs::Tier::Exact;
            } else if tier == refs::Tier::Exact && !names_an_entity_here(&place, value, hit.start) {
                tier = refs::Tier::Review;
            }
        }
        let applied = matches!(tier, refs::Tier::Exact | refs::Tier::Embedded);
        pass.findings.push(Finding {
            place: place.clone(),
            tier,
            line: pass.line_of(absolute.start),
            excerpt: excerpt(value, hit.start, blob),
            applied,
        });
        if applied {
            pass.edits
                .push(splice::Edit::new(absolute, new.as_bytes().to_vec()));
        }
    }
}

pub(crate) fn scan_entity_script(
    src: &[u8],
    name: &str,
    mode: refs::Mode,
    new: &str,
    dbinfo: bool,
    collection: Option<&str>,
) -> Result<XmlPass, std::str::Utf8Error> {
    std::str::from_utf8(src)?;
    let mut pass = XmlPass::new(src);
    if mode == refs::Mode::Entity {
        inspect_script(
            src,
            scan::Span::new(0, src.len()),
            name,
            new,
            dbinfo,
            collection,
            &mut pass,
        );
    } else {
        inspect(
            src,
            scan::Span::new(0, src.len()),
            name,
            mode,
            new,
            Place::Script,
            true,
            collection,
            &mut pass,
        );
    }
    Ok(pass)
}

/// The parser supplies spans only: collection lookups and DataShape API arguments are
/// references; comments, logging strings and unrelated object values remain for review.
fn inspect_script(
    src: &[u8],
    span: scan::Span,
    name: &str,
    new: &str,
    dbinfo: bool,
    collection: Option<&str>,
    pass: &mut XmlPass,
) {
    use crate::core::script;
    let bytes = &src[span.start..span.end];
    let mut structural = std::collections::BTreeSet::new();
    let mut dotted = std::collections::BTreeSet::new();
    if let Ok(parsed) = script::parse(bytes) {
        for member in &parsed.members {
            if let script::Receiver::Variable(receiver) = &member.receiver {
                if crate::core::bundle::COLLECTION_ORDER.contains(&receiver.as_str())
                    && collection.is_none_or(|target| target == receiver)
                    && &bytes[member.property.start..member.property.end] == name.as_bytes()
                {
                    structural.insert((member.property.start, member.property.end));
                    if !member.string_index {
                        dotted.insert((member.property.start, member.property.end));
                    }
                }
            }
        }
        for property in &parsed.call_object_strings {
            if property.key == "dataShapeName"
                && property.value == name
                && collection.is_none_or(|target| target == "DataShapes")
            {
                structural.insert((property.value_span.start, property.value_span.end));
            }
        }
        // DBConnection's literal table contract also names DataShapes outside call arguments.
        let shapes = if dbinfo && collection.is_none_or(|target| target == "DataShapes") {
            crate::core::dbinfo::scan_script(bytes).shapes
        } else {
            Vec::new()
        };
        for shape in shapes {
            for reference in std::iter::once(shape.data_shape).chain(
                shape
                    .foreign_keys
                    .into_iter()
                    .filter_map(|key| key.reference_shape),
            ) {
                if reference.value == name {
                    structural.insert((reference.span.start, reference.span.end));
                }
            }
        }
    }
    let mut mentions = structural.clone();
    mentions.extend(
        refs::find_in_text(span_text(src, span), name, refs::Mode::Entity)
            .into_iter()
            .map(|hit| (hit.start, hit.end)),
    );
    for (start, end) in mentions {
        let absolute = scan::Span::new(span.start + start, span.start + end);
        if structural.contains(&(start, end)) {
            super::findings::add_service_edit(src, absolute, new, Place::Script, pass);
            if dotted.contains(&(start, end)) && refs::validate_field_name(new, "member").is_err() {
                // A dotted access cannot express spaces or a dotted entity name. Replace its
                // dot and property with a string index, retaining every other script byte.
                if let Some(dot) = bytes[..start]
                    .iter()
                    .rposition(|byte| !byte.is_ascii_whitespace())
                {
                    if bytes[dot] == b'.' {
                        let edit = pass.edits.last_mut().unwrap();
                        edit.span.start = span.start + dot;
                        edit.replacement =
                            format!("[{}]", serde_json::to_string(new).unwrap()).into_bytes();
                    }
                }
            }
            if refs::is_qualified(name) {
                pass.findings.last_mut().unwrap().tier = refs::Tier::Embedded;
            }
        } else {
            super::findings::add_review(src, absolute, Place::Script, pass);
        }
    }
}

/// Whether the hit sits right after a mashup-derived id prefix such as `Things_`: that id is built
/// from an entity name, so it is a reference whatever the name looks like.
fn follows_derived_prefix(value: &str, start: usize) -> bool {
    refs::DERIVED_PREFIXES
        .iter()
        .any(|prefix| value[..start].ends_with(prefix))
}

/// Whether an exact, unqualified hit is in a place that holds an entity's name: an attribute that
/// is documented to, or a script literal used as
/// the index of an entity collection (`Things["Name"]`).
fn names_an_entity_here(place: &Place, value: &str, start: usize) -> bool {
    match place {
        Place::Attribute { element, attribute } => match attribute.as_str() {
            "projectName"
            | "thingTemplate"
            | "baseThingTemplate"
            | "valueStream"
            | "inheritedValueStream"
            | "aspect.dataShape"
            | "aspect.thingShape"
            | "aspect.thingTemplate"
            | "dataShapeName"
            | "source" => true,
            "name" => matches!(
                element.as_str(),
                "ImplementedShape" | "Principal" | "Member" | "OrganizationalUnit"
            ),
            "from" | "to" => element == "Connection",
            _ => false,
        },
        Place::EntityName { .. } => true,
        Place::Cdata { .. } | Place::File => is_entity_collection_index(value, start),
        _ => false,
    }
}

/// `Things["Name"]`, `ThingTemplates['Name']` and the other collection lookups: the literal that
/// starts at `start` (just after its opening quote) is an entity's name.
fn is_entity_collection_index(value: &str, start: usize) -> bool {
    let before = &value.as_bytes()[..start];
    let Some(quote) = before.last() else {
        return false;
    };
    if !matches!(quote, b'"' | b'\'' | b'`') {
        return false;
    }
    let mut at = before.len() - 1;
    while at > 0 && before[at - 1].is_ascii_whitespace() {
        at -= 1;
    }
    if at == 0 || before[at - 1] != b'[' {
        return false;
    }
    at -= 1;
    while at > 0 && before[at - 1].is_ascii_whitespace() {
        at -= 1;
    }
    let end = at;
    while at > 0 && (before[at - 1].is_ascii_alphanumeric() || before[at - 1] == b'_') {
        at -= 1;
    }
    matches!(
        &before[at..end],
        b"Things"
            | b"ThingTemplates"
            | b"ThingShapes"
            | b"DataShapes"
            | b"Mashups"
            | b"Groups"
            | b"Roles"
            | b"PermissionGroups"
            | b"Users"
            | b"Organizations"
            | b"Projects"
            | b"StyleThemes"
            | b"MediaEntities"
            | b"Networks"
    )
}

fn attribute_collection(element: &str, attribute: &str) -> Option<&'static str> {
    match attribute {
        "projectName" => Some("Projects"),
        "thingTemplate" | "baseThingTemplate" | "aspect.thingTemplate" => Some("ThingTemplates"),
        "aspect.thingShape" => Some("ThingShapes"),
        "aspect.dataShape" | "dataShapeName" => Some("DataShapes"),
        "valueStream" | "inheritedValueStream" => Some("Things"),
        "source" if matches!(element, "Subscription" | "Event") => Some("Things"),
        "name" if element == "ImplementedShape" => Some("ThingShapes"),
        _ => None,
    }
}

#[cfg(test)]
mod subscription_tests {
    use super::*;

    #[test]
    fn brief99_rename_thing_follows_sources_in_both_subscription_shapes() {
        let src = br#"<Entities><Things><Thing name="Acme.Watcher"><Subscriptions><Subscription name="Old" eventName="ThingStart" source="Clock" sourceType="Thing"/><Subscription name="New" version="2"><Events><Event eventName="Timer" alias="Clock_Timer" source="Clock" sourceType="Thing"/></Events></Subscription></Subscriptions></Thing></Things></Entities>"#;
        let plan = scan_xml_with_identity(
            src,
            "Clock",
            refs::Mode::Entity,
            "Acme.Clock",
            false,
            Some("Things"),
        )
        .unwrap();
        let changed = splice::splice(src, &plan.edits).unwrap();
        let xml = String::from_utf8(changed).unwrap();
        assert_eq!(xml.matches("source=\"Acme.Clock\"").count(), 2);
        assert!(xml.contains("alias=\"Clock_Timer\""));
    }
}
