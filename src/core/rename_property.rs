//! Where a property's name appears, and how a property rename edits it.
//!
//! In an entity's XML a property is named by its definition, the value element under
//! `ThingProperties`, an alert configuration, a property binding, a subscription's event
//! (`<Event sourceProperty="...">`) and a run-time permission keyed by the member's name
//! (`<Permissions resourceName="...">`). In a script it is read as `me.Name`, `this.Name` and
//! `Things["T"].Name`. A call such as `me.Name(` is a service, never a property, and is left alone.
//! Everything else that is exactly the name (another receiver, a quoted string) is a review finding.

use super::rename_scan::{
    add_field_edit, add_review, add_service_edit, lexical_mentions, span_text, Place, XmlPass,
};
use super::{scan, script};
use std::collections::{BTreeMap, BTreeSet};

/// What one entity says about the property, and the edits for it.
pub struct PropertyPass {
    pub pass: XmlPass,
    /// The entity declares a property with the old name.
    pub declared: bool,
    /// The entity declares a property with the new name.
    pub new_declared: bool,
}

/// Scan one entity document. `affected` are the scope and the entities that inherit it (they carry
/// the property); `script_scope` says whether this entity's own scripts read `me.<name>`.
pub fn scan_property_entity(
    src: &[u8],
    old: &str,
    new: &str,
    entity: &str,
    affected: &BTreeSet<String>,
    script_scope: bool,
) -> Result<PropertyPass, scan::ScanError> {
    let tokens = scan::tokenize(src)?;
    let mut pass = XmlPass::new_for(src);
    let mut declared = false;
    let mut new_declared = false;
    let is_affected = affected.contains(entity);
    let element = |name: &str| Place::Service { element: name.to_string() };
    let value_of = |token: &scan::Token, attribute: &str| -> Result<Option<scan::Span>, scan::ScanError> {
        scan::attribute(src, token, attribute)
    };
    let is = |span: Option<scan::Span>, text: &str| span.is_some_and(|span| span.of(src) == text.as_bytes());

    for (index, token) in tokens.iter().enumerate() {
        if !matches!(token.kind, scan::Kind::Start | scan::Kind::Empty) {
            continue;
        }
        match token.name.of(src) {
            b"PropertyDefinition" => {
                if let Some(name) = value_of(token, "name")? {
                    if name.of(src) == old.as_bytes() {
                        declared = true;
                        if is_affected {
                            add_field_edit(src, name, new, element("PropertyDefinition"), &mut pass);
                        }
                    } else if name.of(src) == new.as_bytes() {
                        new_declared = true;
                    }
                }
            }
            b"AlertDefinitions" | b"RemotePropertyBinding" if is_affected => {
                if let Some(name) = value_of(token, "name")?.filter(|span| span.of(src) == old.as_bytes()) {
                    add_field_edit(src, name, new, element("AlertDefinitions or RemotePropertyBinding"), &mut pass);
                }
            }
            b"PropertyBinding" => {
                // Its own name, and a binding elsewhere that reads this property as its source.
                if is_affected {
                    if let Some(name) = value_of(token, "name")?.filter(|span| span.of(src) == old.as_bytes()) {
                        add_field_edit(src, name, new, element("PropertyBinding"), &mut pass);
                    }
                }
                let source_thing = value_of(token, "sourceThingName")?
                    .map(|span| String::from_utf8_lossy(span.of(src)).into_owned());
                if let (Some(thing), Some(property)) = (source_thing, value_of(token, "sourcePropertyName")?) {
                    if property.of(src) == old.as_bytes() && affected.contains(&thing) {
                        add_field_edit(src, property, new, element("PropertyBinding source"), &mut pass);
                    }
                }
            }
            b"Event" => {
                // A subscription's data-change event: `source=""` means this entity itself.
                let source = value_of(token, "source")?;
                let names_affected = match source {
                    Some(span) if span.of(src).is_empty() => is_affected,
                    Some(span) => affected.contains(String::from_utf8_lossy(span.of(src)).as_ref()),
                    None => false,
                };
                if let Some(property) = value_of(token, "sourceProperty")?.filter(|span| span.of(src) == old.as_bytes()) {
                    if names_affected {
                        add_field_edit(src, property, new, element("Event"), &mut pass);
                    }
                }
            }
            b"Permissions" if is_affected => {
                if is(value_of(token, "resourceName")?, old) {
                    let span = value_of(token, "resourceName")?.expect("checked above");
                    add_field_edit(src, span, new, element("Permissions"), &mut pass);
                }
            }
            b"ThingProperties" if token.kind == scan::Kind::Start && is_affected => {
                for child in super::rename_scan::child_elements(&tokens, index) {
                    if tokens[child].name.of(src) == old.as_bytes() {
                        add_field_edit(src, tokens[child].name, new, element("ThingProperties"), &mut pass);
                        if let Some(end) = scan::element_end_in(&tokens, src, child) {
                            add_field_edit(src, tokens[end].name, new, element("ThingProperties"), &mut pass);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // The scripts of this entity, read as `me.<name>`.
    let mut elements = Vec::<String>::new();
    for token in &tokens {
        match token.kind {
            scan::Kind::Start => elements.push(span_text(src, token.name).to_string()),
            scan::Kind::End => {
                elements.pop();
            }
            scan::Kind::Cdata if elements.last().is_some_and(|name| name == "code") => {
                scan_property_script(src, token.inner, old, new, script_scope, affected, &mut pass);
            }
            _ => {}
        }
    }
    Ok(PropertyPass { pass, declared, new_declared })
}

/// Scan a sidecar script (the whole file is script text).
pub fn scan_property_script_file(
    src: &[u8],
    old: &str,
    new: &str,
    local: bool,
    affected: &BTreeSet<String>,
) -> Result<XmlPass, std::str::Utf8Error> {
    std::str::from_utf8(src)?;
    let mut pass = XmlPass::new_for(src);
    scan_property_script(src, scan::Span::new(0, src.len()), old, new, local, affected, &mut pass);
    Ok(pass)
}

/// The property reads in `span` of `src`. `local`: `me.<name>` and `this.<name>` are this entity's
/// own property. `affected`: the entities whose `Things["T"].<name>` is the property.
///
/// A script the parser refuses gets no edits; every whole-word mention of the name is left for
/// review.
fn scan_property_script(
    src: &[u8],
    span: scan::Span,
    old: &str,
    new: &str,
    local: bool,
    affected: &BTreeSet<String>,
    pass: &mut XmlPass,
) {
    let text = span.of(src);
    let shift = |inner: scan::Span| scan::Span::new(span.start + inner.start, span.start + inner.end);
    let Ok(script) = script::parse(text) else {
        for mention in lexical_mentions(text, old) {
            add_review(src, shift(mention), Place::Property, pass);
        }
        return;
    };
    // Each occurrence, in source order, with whether it is the entity's property.
    let mut found = BTreeMap::<(usize, usize), bool>::new();
    for member in &script.members {
        // `me.Name(` is a service call, never a property.
        if member.string_index || member.is_callee || member.property.of(text) != old.as_bytes() {
            continue;
        }
        let owns = match &member.receiver {
            script::Receiver::Me | script::Receiver::This => local,
            other => script
                .thing_of(other)
                .is_some_and(|entity| affected.contains(entity)),
        };
        found.insert((member.property.start, member.property.end), owns);
    }
    for string in script.strings.iter().filter(|string| string.value == old) {
        // `me["Name"]` is a read of the property; any other literal is only a mention.
        let own_index = script.members.iter().any(|member| {
            member.string_index
                && member.property == string.span
                && matches!(member.receiver, script::Receiver::Me | script::Receiver::This)
        });
        found.insert((string.span.start, string.span.end), own_index && local);
    }
    for ((start, end), owns) in found {
        let hit = shift(scan::Span::new(start, end));
        if owns {
            add_service_edit(src, hit, new, Place::Property, pass);
        } else {
            add_review(src, hit, Place::Property, pass);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::splice;

    fn affected() -> BTreeSet<String> {
        BTreeSet::from(["P.T".to_string(), "P.Child".to_string()])
    }

    fn apply(src: &[u8], pass: &XmlPass) -> String {
        String::from_utf8(splice::splice(src, &pass.edits).unwrap()).unwrap()
    }

    const ENTITY: &str = r#"<Entities><Things><Thing name="P.Child" thingTemplate="P.T"><Permissions/><RunTimePermissions><Permissions resourceName="Temperature"/><Permissions resourceName="Other"/></RunTimePermissions><PropertyBindings><PropertyBinding name="Temperature" sourceThingName="P.T" sourcePropertyName="Temperature"/><PropertyBinding name="Mine" sourceThingName="Elsewhere" sourcePropertyName="Temperature"/></PropertyBindings><AlertConfigurations><AlertDefinitions name="Temperature"/></AlertConfigurations><ThingProperties><Temperature><Value>5</Value></Temperature><Other><Value>1</Value></Other></ThingProperties><ThingShape><PropertyDefinitions><PropertyDefinition name="Temperature" baseType="NUMBER"/></PropertyDefinitions><Subscriptions><Subscription name="s"><Event eventName="DataChange" source="" sourceProperty="Temperature" sourceType="Thing"/><Event eventName="DataChange" source="Elsewhere" sourceProperty="Temperature" sourceType="Thing"/></Subscription></Subscriptions><ServiceImplementations><ServiceImplementation name="Run"><ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[var v = me.Temperature; me.Temperature = v; me["Temperature"]; me.Temperature(); Things["P.T"].Temperature; other.Temperature; "Temperature"]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations></ThingShape></Thing></Things></Entities>"#;

    #[test]
    fn an_entity_follows_a_property_through_every_place_it_is_named() {
        let scanned = scan_property_entity(ENTITY.as_bytes(), "Temperature", "Heat", "P.Child", &affected(), true).unwrap();
        let out = apply(ENTITY.as_bytes(), &scanned.pass);
        assert!(scanned.declared && !scanned.new_declared);
        assert!(out.contains(r#"<PropertyDefinition name="Heat""#), "{out}");
        assert!(out.contains("<Heat><Value>5</Value></Heat>") && out.contains("<Other><Value>1</Value></Other>"), "{out}");
        assert!(out.contains(r#"<AlertDefinitions name="Heat"/>"#), "{out}");
        assert!(out.contains(r#"<Permissions resourceName="Heat"/>"#) && out.contains(r#"resourceName="Other""#), "{out}");
        // A binding of its own, and a binding that reads it from an affected Thing; not one that reads another Thing's.
        assert!(out.contains(r#"<PropertyBinding name="Heat" sourceThingName="P.T" sourcePropertyName="Heat"/>"#), "{out}");
        assert!(out.contains(r#"sourceThingName="Elsewhere" sourcePropertyName="Temperature""#), "{out}");
        // The subscription on this entity follows; one on another Thing does not.
        assert!(out.contains(r#"source="" sourceProperty="Heat""#) && out.contains(r#"source="Elsewhere" sourceProperty="Temperature""#), "{out}");
        // Scripts: reads and writes through me, an index, and Things["P.T"]; a call, another receiver and a plain string are not edited.
        assert!(out.contains("var v = me.Heat; me.Heat = v; me[\"Heat\"]; me.Temperature(); Things[\"P.T\"].Heat; other.Temperature;"), "{out}");
        assert!(out.contains("\"Temperature\"]]>"), "a bare string is only a mention: {out}");
        let reviews = scanned.pass.findings.iter().filter(|finding| !finding.applied).count();
        assert_eq!(reviews, 2, "other.Temperature and the quoted string: {:?}", scanned.pass.findings);
    }

    #[test]
    fn an_entity_that_does_not_carry_the_property_is_only_read_for_what_points_at_it() {
        let outside = BTreeSet::from(["P.T".to_string()]);
        let scanned = scan_property_entity(ENTITY.as_bytes(), "Temperature", "Heat", "P.Child", &outside, false).unwrap();
        let out = apply(ENTITY.as_bytes(), &scanned.pass);
        // Not its own: the definition, value element, alert and permission stay; a binding whose source is P.T follows.
        assert!(out.contains("<Temperature><Value>5</Value></Temperature>"), "{out}");
        assert!(out.contains(r#"sourceThingName="P.T" sourcePropertyName="Heat""#), "{out}");
        assert!(out.contains(r#"<PropertyDefinition name="Temperature""#), "{out}");
    }

    #[test]
    fn a_script_file_resolves_a_variable_assigned_from_things_and_leaves_others() {
        let script = b"const t = Things[\"P.T\"]; const o = Things[\"Else\"]; t.Temperature; o.Temperature; t.Temperature();";
        let pass = scan_property_script_file(script, "Temperature", "Heat", false, &affected()).unwrap();
        let out = apply(script, &pass);
        assert_eq!(out, "const t = Things[\"P.T\"]; const o = Things[\"Else\"]; t.Heat; o.Temperature; t.Temperature();");
    }

    /// The script after a property rename of `Temperature`, and the findings that were not edits.
    fn rename(script: &str, local: bool) -> (String, usize) {
        let pass =
            scan_property_script_file(script.as_bytes(), "Temperature", "Heat", local, &affected()).unwrap();
        let reviews = pass.findings.iter().filter(|finding| !finding.applied).count();
        (apply(script.as_bytes(), &pass), reviews)
    }

    #[test]
    fn a_read_is_found_through_line_breaks_and_templates() {
        let (out, reviews) = rename("me\n  .Temperature = 1;\nvar s = `${me.Temperature} ${this.Temperature}`;", true);
        assert_eq!(out, "me\n  .Heat = 1;\nvar s = `${me.Heat} ${this.Heat}`;");
        assert_eq!(reviews, 0);
    }

    #[test]
    fn text_that_only_looks_like_a_read_is_not_a_property() {
        // A string holding a path, a regex and a comment hold no property access at all.
        let script = "var s = 'me.Temperature'; var r = /me.Temperature/; // me.Temperature\n";
        assert_eq!(rename(script, true), (script.to_string(), 0));
    }

    #[test]
    fn a_chain_is_not_a_variable_and_an_ambiguous_variable_proves_nothing() {
        // The receiver of `.Temperature` here is `a.b`, not the variable `b`.
        let chain = "var b = Things[\"P.T\"]; a.b.Temperature;";
        assert_eq!(rename(chain, false), (chain.to_string(), 1));
        for ambiguous in [
            "var t = Things[\"P.T\"]; t = other; t.Temperature;",
            "var t = Things[\"P.T\"]; var t = Things[\"Else\"]; t.Temperature;",
            "var t = Things[\"P.T\"]; function f(t) { return t.Temperature; }",
        ] {
            assert_eq!(rename(ambiguous, false), (ambiguous.to_string(), 1), "{ambiguous}");
        }
    }

    #[test]
    fn an_index_on_me_is_a_read_and_any_other_literal_is_a_mention() {
        let (out, reviews) = rename("me[\"Temperature\"]; this['Temperature']; x[\"Temperature\"]; f('Temperature');", true);
        assert_eq!(out, "me[\"Heat\"]; this['Heat']; x[\"Temperature\"]; f('Temperature');");
        assert_eq!(reviews, 2);
        let own = "me[\"Temperature\"];";
        assert_eq!(rename(own, false), (own.to_string(), 1), "not the entity's own property");
    }

    #[test]
    fn a_script_the_parser_refuses_is_not_edited() {
        // Rhino's `for each` is not ECMAScript: every mention of the name is left for a person.
        let script = "for each (x in y) { me.Temperature; 'Temperature'; }";
        assert_eq!(rename(script, true), (script.to_string(), 2));
    }
}
