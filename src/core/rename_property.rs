//! Where a property's name appears, and how a property rename edits it.
//!
//! In an entity's XML a property is named by its definition, the value element under
//! `ThingProperties`, an alert configuration, a property binding, a subscription's event
//! (`<Event sourceProperty="...">`) and a run-time permission keyed by the member's name
//! (`<Permissions resourceName="...">`). In a script it is read as `me.Name`, `this.Name` and
//! `Things["T"].Name`. A call such as `me.Name(` is a service, never a property, and is left alone.
//! Everything else that is exactly the name (another receiver, a quoted string) is a review finding.

use super::rename_scan::{
    add_field_edit, add_review, add_service_edit, js_tokens, param_receiver_variables, previous_token,
    span_text, thing_receiver_tokens, JsKind, JsToken, Place, XmlPass,
};
use super::scan;
use std::collections::BTreeSet;

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
    let tokens: Vec<JsToken> = js_tokens(text).into_iter().filter(|token| token.kind != JsKind::Comment).collect();
    let variables = param_receiver_variables(text, &tokens);
    let is_call = |at: usize| tokens.get(at + 1).is_some_and(|next| next.span.of(text) == b"(");
    for (at, token) in tokens.iter().enumerate() {
        let raw = token.span.of(text);
        match token.kind {
            JsKind::Ident if raw == old.as_bytes() && previous_token(&tokens, at).is_some_and(|dot| dot.span.of(text) == b".") => {
                if is_call(at) {
                    continue; // `me.Name(` is a service call
                }
                let receiver = receiver_before(text, &tokens, at);
                let owns = match &receiver {
                    Receiver::Own => local,
                    Receiver::Thing(entity) => affected.contains(entity),
                    Receiver::Variable(name) => variables.get(name).is_some_and(|entity| affected.contains(entity)),
                    Receiver::Other => false,
                };
                if owns {
                    add_service_edit(src, shift(token.span), new, Place::Property, pass);
                } else {
                    add_review(src, shift(token.span), Place::Property, pass);
                }
            }
            JsKind::String if raw.len() >= 2 && &raw[1..raw.len() - 1] == old.as_bytes() => {
                // `me["Name"]` is a read of the property; any other literal is only a mention.
                let inner = scan::Span::new(token.span.start + 1, token.span.end - 1);
                let own_index = at >= 2
                    && tokens[at - 1].span.of(text) == b"["
                    && matches!(tokens[at - 2].span.of(text), b"me" | b"this")
                    && tokens.get(at + 1).is_some_and(|close| close.span.of(text) == b"]");
                if own_index && local {
                    add_service_edit(src, shift(inner), new, Place::Property, pass);
                } else {
                    add_review(src, shift(inner), Place::Property, pass);
                }
            }
            _ => {}
        }
    }
}

enum Receiver {
    Own,
    Thing(String),
    Variable(Vec<u8>),
    Other,
}

/// What stands before the `.` that precedes token `at`.
fn receiver_before(text: &[u8], tokens: &[JsToken], at: usize) -> Receiver {
    if at < 2 {
        return Receiver::Other;
    }
    let receiver = &tokens[at - 2];
    match receiver.span.of(text) {
        b"me" | b"this" => Receiver::Own,
        b"]" => {
            // Walk back to `Things [ "X" ]`.
            let mut start = at - 2;
            while start > 0 && tokens[start].span.of(text) != b"Things" {
                start -= 1;
                if at - start > 6 {
                    return Receiver::Other;
                }
            }
            match thing_receiver_tokens(text, tokens, start) {
                Some((end, entity)) if end == at - 1 => Receiver::Thing(entity),
                _ => Receiver::Other,
            }
        }
        _ if receiver.kind == JsKind::Ident => Receiver::Variable(receiver.span.of(text).to_vec()),
        _ => Receiver::Other,
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
}
