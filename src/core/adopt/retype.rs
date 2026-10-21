//! Service parameters an export retypes, and the literal arguments scripts pass them.
//!
//! When a designer changes a service parameter's base type (STRING to LONG, say), a script that
//! passes it `"42"` still passes a string. Each such literal is mapped to one the new type takes
//! (`42`), or reported when there is none (`"abc"`). One scan finds them for the report and
//! rewrites them on apply, in a sidecar script and in its entity's CDATA alike.

use super::flatten::{attribute, child_elements};
use super::{AdoptError, EntityRef, Report, RetypedArgument, RetypedDefault};
use crate::core::basetype::{map_literal, Literal, Mapped};
use crate::core::config::Solution;
use crate::core::normalise::Element;
use crate::core::rename::scan::is_caller_receiver;
use crate::core::scan::{self, Span};
use crate::core::{catalog, script, splice, workspace};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One retyped service parameter, with the entities whose `me` reaches the service.
pub(super) struct Retype {
    pub(super) entity: EntityRef,
    service: String,
    parameter: String,
    from: String,
    to: String,
    callers: BTreeSet<String>,
}

/// A literal argument found in one script text.
struct Found<'a> {
    retype: &'a Retype,
    span: Span,
    literal: String,
    mapped: Mapped,
}

impl Retype {
    /// `(entity, service, parameter)`, as the report's arguments and defaults name them.
    pub(super) fn key(&self) -> (String, String, String) {
        (
            self.entity.path(),
            self.service.clone(),
            self.parameter.clone(),
        )
    }
}

/// The service parameters the report retypes. A mashup parameter or a DataShape field is not one.
pub(super) fn retypes(solution: &Solution, report: &Report) -> Vec<Retype> {
    let mut found: Vec<Retype> = report
        .type_changes()
        .into_iter()
        .filter_map(|(entity, difference)| {
            let (service, parameter) = service_parameter(&difference.path)?;
            Some(Retype {
                entity: entity.clone(),
                service,
                parameter,
                from: difference.repo.clone().unwrap_or_default(),
                to: difference.export.clone().unwrap_or_default(),
                callers: BTreeSet::new(),
            })
        })
        .collect();
    if found.is_empty() {
        return found;
    }
    let (inheritance, _) = catalog::inheritance(solution);
    for retype in &mut found {
        retype.callers.insert(retype.entity.name.clone());
        for item in &inheritance {
            if item.inherits.contains(&retype.entity.name)
                || (retype.entity.collection == "ThingShapes"
                    && item.implemented_by.contains(&retype.entity.name))
            {
                retype.callers.insert(item.name.clone());
            }
        }
    }
    found
}

/// `<svc>` and `<p>` of `[/ThingShape]/ServiceDefinitions/ServiceDefinition[<svc>]/
/// ParameterDefinitions/FieldDefinition[<p>]@baseType`.
fn service_parameter(path: &str) -> Option<(String, String)> {
    let rest = path
        .strip_prefix("/ThingShape")
        .unwrap_or(path)
        .strip_prefix("/ServiceDefinitions/ServiceDefinition[")?;
    let (service, rest) = rest.split_once("]/ParameterDefinitions/FieldDefinition[")?;
    let parameter = rest.strip_suffix("]@baseType")?;
    Some((service.to_string(), parameter.to_string()))
}

/// The report's arguments and defaults for `retypes`, read from the repository's scripts: each
/// entity's sidecars where it has them, its XML otherwise.
pub(super) fn report(
    solution: &Solution,
    retypes: &[Retype],
    exported: &BTreeMap<EntityRef, Element>,
) -> Result<(Vec<RetypedArgument>, Vec<RetypedDefault>), AdoptError> {
    let mut defaults = Vec::new();
    for retype in retypes {
        let Some(default) = exported
            .get(&retype.entity)
            .and_then(|entity| parameter_default(entity, &retype.service, &retype.parameter))
        else {
            continue;
        };
        if let Mapped::Unmappable(why) =
            map_literal(&default_literal(&default, &retype.to), &retype.to)
        {
            defaults.push(RetypedDefault {
                entity: retype.entity.path(),
                service: retype.service.clone(),
                parameter: retype.parameter.clone(),
                to: retype.to.clone(),
                default,
                why,
            });
        }
    }
    let mut arguments = Vec::new();
    if retypes.is_empty() {
        return Ok((arguments, defaults));
    }
    for (entity, texts) in script_texts(solution)? {
        for (path, bytes, spans) in texts {
            for span in spans {
                let text = span.of(&bytes);
                for found in literals(text, &entity.info.name, retypes) {
                    let at = span.start + found.span.start;
                    arguments.push(RetypedArgument {
                        service_entity: found.retype.entity.path(),
                        service: found.retype.service.clone(),
                        parameter: found.retype.parameter.clone(),
                        from: found.retype.from.clone(),
                        to: found.retype.to.clone(),
                        caller: format!("{}/{}", entity.info.collection, entity.info.name),
                        file: path.clone(),
                        line: bytes[..at].iter().filter(|byte| **byte == b'\n').count() + 1,
                        literal: found.literal,
                        mapped: found.mapped,
                    });
                }
            }
        }
    }
    Ok((arguments, defaults))
}

/// `bytes` with every mappable literal for `retypes` rewritten, in each of `spans` (one script
/// each); `None` when nothing changes.
pub(super) fn rewrite(
    bytes: &[u8],
    spans: &[Span],
    caller: &str,
    retypes: &[Retype],
) -> Option<Vec<u8>> {
    let mut edits = Vec::new();
    for span in spans {
        for found in literals(span.of(bytes), caller, retypes) {
            if let Mapped::Rewritten(text) = found.mapped {
                let at = Span::new(span.start + found.span.start, span.start + found.span.end);
                edits.push(splice::Edit::new(at, text.into_bytes()));
            }
        }
    }
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|edit| edit.span.start);
    splice::splice(bytes, &edits).ok()
}

/// Every entity's script texts as `(path, bytes, script spans)`, as its source has them: a
/// service's sidecar `script.js` where it has one, and its entity XML for every other service and
/// for subscriptions.
#[allow(clippy::type_complexity)]
fn script_texts(
    solution: &Solution,
) -> Result<Vec<(workspace::EntityFile, Vec<(PathBuf, Vec<u8>, Vec<Span>)>)>, AdoptError> {
    let mut out = Vec::new();
    for entity in workspace::discover(solution).entities {
        let mut texts = Vec::new();
        let mut in_sidecars = BTreeSet::new();
        for path in sidecar_scripts(solution, &entity) {
            if let Some(service) = path.parent().and_then(Path::file_name) {
                in_sidecars.insert(service.to_string_lossy().into_owned());
            }
            let bytes = read(&path)?;
            let whole = Span::new(0, bytes.len());
            texts.push((path, bytes, vec![whole]));
        }
        let bytes = read(&entity.path)?;
        let spans: Vec<Span> = scripts_in_xml(&bytes)
            .into_iter()
            .filter(|(service, _)| {
                service
                    .as_ref()
                    .is_none_or(|name| !in_sidecars.contains(name))
            })
            .map(|(_, span)| span)
            .collect();
        if !spans.is_empty() {
            texts.push((entity.path.clone(), bytes, spans));
        }
        out.push((entity, texts));
    }
    Ok(out)
}

/// The sidecar scripts of an entity, in name order.
pub(super) fn sidecar_scripts(solution: &Solution, entity: &workspace::EntityFile) -> Vec<PathBuf> {
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(workspace::services_dir(solution, entity))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("script.js"))
        .filter(|path| path.is_file())
        .collect();
    scripts.sort();
    scripts
}

/// The script bodies of an entity's XML: the CDATA of each `code` cell of a service
/// implementation (with the service's name) or of a subscription (with none). A description or
/// any other CDATA is not a script, however much it looks like one.
pub(super) fn scripts_in_xml(bytes: &[u8]) -> Vec<(Option<String>, Span)> {
    let Ok(tokens) = scan::tokenize(bytes) else {
        return Vec::new();
    };
    let mut open: Vec<(&[u8], Option<String>)> = Vec::new();
    let mut out = Vec::new();
    for token in &tokens {
        match token.kind {
            scan::Kind::Start => {
                let name = token.name.of(bytes);
                let named = scan::attribute(bytes, token, "name")
                    .ok()
                    .flatten()
                    .map(|value| scan::decode_entities(&String::from_utf8_lossy(value.of(bytes))));
                open.push((name, named));
            }
            scan::Kind::End => {
                open.pop();
            }
            scan::Kind::Cdata if open.last().is_some_and(|(name, _)| *name == b"code") => {
                let host = open.iter().rev().find(|(name, _)| {
                    *name == b"ServiceImplementation" || *name == b"Subscription"
                });
                match host {
                    Some((b"ServiceImplementation", service)) => {
                        out.push((service.clone(), token.inner));
                    }
                    Some(_) => out.push((None, token.inner)),
                    None => {}
                }
            }
            _ => {}
        }
    }
    out
}

/// The script spans of an entity's XML, for a rewrite: every script body, sidecar-backed or not,
/// so the XML stays in step with the sidecars.
pub(super) fn script_spans(bytes: &[u8]) -> Vec<Span> {
    scripts_in_xml(bytes)
        .into_iter()
        .map(|(_, span)| span)
        .collect()
}

fn read(path: &Path) -> Result<Vec<u8>, AdoptError> {
    std::fs::read(path).map_err(|error| AdoptError::Repository {
        path: path.to_path_buf(),
        why: error.to_string(),
    })
}

/// The literal arguments one script of `caller` passes to a retyped parameter, with their spans
/// in `text`. A call reaches the service as `rename param` decides it does: `me` or `this` in
/// an entity that has the service, or a Thing that has it, named or held in a variable.
fn literals<'a>(text: &[u8], caller: &str, retypes: &'a [Retype]) -> Vec<Found<'a>> {
    let Ok(parsed) = script::parse(text) else {
        return Vec::new();
    };
    // A `me` the script declares or takes as a parameter is not the entity: which Thing a call
    // on it reaches cannot be told, so nothing in such a script is mapped.
    let binds_me = parsed.identifiers.iter().any(|identifier| {
        matches!(
            identifier.role,
            script::Role::Declaration | script::Role::Parameter
        ) && identifier.span.of(text) == b"me"
    });
    if binds_me {
        return Vec::new();
    }
    let mut out = Vec::new();
    for call in &parsed.calls {
        for retype in retypes {
            if call.property.of(text) != retype.service.as_bytes()
                || !is_caller_receiver(
                    &parsed,
                    &call.receiver,
                    retype.callers.contains(caller),
                    &retype.callers,
                )
            {
                continue;
            }
            let script::FirstArgument::Object(keys) = &call.first else {
                continue;
            };
            let Some(key) = keys.iter().find(|key| key.text == retype.parameter) else {
                continue;
            };
            let Some(value) = value_of(&parsed, key.span) else {
                continue;
            };
            let Some((literal, span)) = literal_of(value, text) else {
                continue;
            };
            out.push(Found {
                retype,
                span,
                literal: String::from_utf8_lossy(span.of(text)).into_owned(),
                mapped: map_literal(&literal.as_literal(), &retype.to),
            });
        }
    }
    out
}

/// The value written after the object key at `key`.
fn value_of(parsed: &script::Script, key: Span) -> Option<&script::Value> {
    parsed.objects.iter().find_map(|object| {
        object
            .properties
            .iter()
            .find_map(|property| match property {
                script::ObjectProperty::KeyValue {
                    key: written,
                    value,
                } if written.span == key => Some(value),
                _ => None,
            })
    })
}

/// A scalar literal as written, a number already in decimal.
enum Written<'a> {
    String(&'a str),
    Number(String),
    Bool(bool),
    Null,
}

impl Written<'_> {
    fn as_literal(&self) -> Literal<'_> {
        match self {
            Written::String(text) => Literal::String(text),
            Written::Number(text) => Literal::Number(text),
            Written::Bool(value) => Literal::Bool(*value),
            Written::Null => Literal::Null,
        }
    }
}

/// A scalar literal and its span, quotes included. A variable, call or other expression is no
/// literal to map, and an object or array is what a JSON or INFOTABLE parameter takes.
fn literal_of<'a>(value: &'a script::Value, text: &'a [u8]) -> Option<(Written<'a>, Span)> {
    match value {
        script::Value::String(string) => Some((
            Written::String(&string.value),
            Span::new(string.span.start - 1, string.span.end + 1),
        )),
        script::Value::Other(span) => {
            let written = std::str::from_utf8(span.of(text)).ok()?;
            let literal = match written {
                "true" => Written::Bool(true),
                "false" => Written::Bool(false),
                "null" => Written::Null,
                number => Written::Number(js_number(number)?),
            };
            Some((literal, *span))
        }
        script::Value::Object(_) | script::Value::Array(_) => None,
    }
}

/// A JavaScript number literal in decimal: `1_000` is `1000`, `.5` is `0.5`, `0x10` is `16`. A
/// BigInt (`5n`), a legacy octal (`010`) or anything that is not a number literal is `None`.
fn js_number(text: &str) -> Option<String> {
    let (sign, digits) = text
        .strip_prefix('-')
        .map_or(("", text), |rest| ("-", rest));
    let digits = digits.replace('_', "");
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(rest) = digits.strip_prefix(prefix) {
            let value = u64::from_str_radix(rest, radix).ok()?;
            return Some(format!("{sign}{value}"));
        }
    }
    let (mantissa, exponent) = digits
        .split_once(['e', 'E'])
        .map_or((digits.as_str(), None), |(m, e)| (m, Some(e)));
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all_digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if (integer.is_empty() && fraction.is_empty())
        || !all_digits(integer)
        || !all_digits(fraction)
        || (integer.len() > 1 && integer.starts_with('0') && !mantissa.contains('.'))
    {
        return None;
    }
    let mut out = format!("{sign}{}", if integer.is_empty() { "0" } else { integer });
    if !fraction.is_empty() {
        out.push('.');
        out.push_str(fraction);
    }
    if let Some(exponent) = exponent {
        let bare = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if bare.is_empty() || !all_digits(bare) {
            return None;
        }
        out.push('e');
        out.push_str(exponent);
    }
    Some(out)
}

/// A decimal number literal, optionally negative and with a fraction or exponent.
fn is_number(text: &str) -> bool {
    let text = text.strip_prefix('-').unwrap_or(text);
    let (mantissa, exponent) = text
        .split_once(['e', 'E'])
        .map_or((text, None), |(m, e)| (m, Some(e)));
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    !integer.is_empty()
        && digits(integer)
        && digits(fraction)
        && exponent.is_none_or(|e| {
            let e = e.strip_prefix(['+', '-']).unwrap_or(e);
            !e.is_empty() && digits(e)
        })
}

/// The retyped parameter's `aspect.defaultValue` in the export, on the entity or its ThingShape.
fn parameter_default(entity: &Element, service: &str, parameter: &str) -> Option<String> {
    let shape = child_elements(entity).find(|child| child.name == b"ThingShape");
    let definitions = child_elements(entity)
        .chain(shape.into_iter().flat_map(child_elements))
        .find(|child| child.name == b"ServiceDefinitions")?;
    let definition = child_elements(definitions)
        .find(|child| attribute(child, "name").as_deref() == Some(service))?;
    let parameters =
        child_elements(definition).find(|child| child.name == b"ParameterDefinitions")?;
    child_elements(parameters)
        .find(|field| attribute(field, "name").as_deref() == Some(parameter))
        .and_then(|field| attribute(field, "aspect.defaultValue"))
        .filter(|default| !default.is_empty())
}

/// A default as the literal a script would write: aspects are text, so a number or boolean
/// default of a numeric or boolean type reads as one.
fn default_literal<'a>(default: &'a str, to: &str) -> Literal<'a> {
    match to {
        "BOOLEAN" if default == "true" || default == "false" => Literal::Bool(default == "true"),
        "NUMBER" | "INTEGER" | "LONG" | "DATETIME" if is_number(default) => {
            Literal::Number(default)
        }
        _ => Literal::String(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_service_parameter_path_is_found_on_a_thing_and_on_a_shape() {
        for path in [
            "/ThingShape/ServiceDefinitions/ServiceDefinition[GetOrder]/ParameterDefinitions/FieldDefinition[orderId]@baseType",
            "/ServiceDefinitions/ServiceDefinition[GetOrder]/ParameterDefinitions/FieldDefinition[orderId]@baseType",
        ] {
            assert_eq!(
                service_parameter(path),
                Some(("GetOrder".into(), "orderId".into())),
                "{path}"
            );
        }
        assert_eq!(
            service_parameter("/ParameterDefinitions/FieldDefinition[cardUid]@baseType"),
            None,
            "a mashup parameter is not a service parameter"
        );
    }

    #[test]
    fn every_javascript_number_form_reads_as_its_decimal() {
        for (written, decimal) in [
            ("1_000", Some("1000")),
            (".5", Some("0.5")),
            ("5.", Some("5")),
            ("0x10", Some("16")),
            ("0b101", Some("5")),
            ("-0o17", Some("-15")),
            ("2.5E-2", Some("2.5e-2")),
            ("0", Some("0")),
            ("010", None),
            ("5n", None),
            ("given", None),
            (".", None),
        ] {
            assert_eq!(js_number(written).as_deref(), decimal, "{written}");
        }
    }

    #[test]
    fn only_service_and_subscription_code_is_a_script() {
        let xml = b"<Thing description=\"x\"><documentationContent><![CDATA[me.GetOrder({orderId: \"1\"});]]></documentationContent><ServiceImplementations><ServiceImplementation name=\"Run\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[a]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations><Subscriptions><Subscription name=\"On\"><ServiceImplementation name=\"On\"><ConfigurationTables><ConfigurationTable name=\"Script\"><Rows><Row><code><![CDATA[b]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></Subscription></Subscriptions></Thing>";
        let found: Vec<_> = scripts_in_xml(xml)
            .into_iter()
            .map(|(service, span)| (service, String::from_utf8(span.of(xml).to_vec()).unwrap()))
            .collect();
        assert_eq!(
            found,
            [
                (Some("Run".to_string()), "a".to_string()),
                (Some("On".to_string()), "b".to_string())
            ]
        );
    }

    #[test]
    fn a_script_that_binds_its_own_me_maps_nothing() {
        let retype = Retype {
            entity: EntityRef {
                collection: "Things".into(),
                name: "P.Orders".into(),
            },
            service: "GetOrder".into(),
            parameter: "orderId".into(),
            from: "STRING".into(),
            to: "LONG".into(),
            callers: BTreeSet::from(["P.Orders".to_string()]),
        };
        let retypes = [retype];
        let plain = b"return me.GetOrder({orderId: \"42\"});";
        assert_eq!(literals(plain, "P.Orders", &retypes).len(), 1);
        let shadowed =
            b"return (function (me) { return me.GetOrder({orderId: \"42\"}); })(Things[\"P.Other\"]);";
        assert!(literals(shadowed, "P.Orders", &retypes).is_empty());
    }

    #[test]
    fn number_literals_are_decimal_with_fraction_or_exponent() {
        for yes in ["0", "-5", "12.5", "1e3", "2.5E-2"] {
            assert!(is_number(yes), "{yes}");
        }
        for no in ["", "-", "0x1F", "1.2.3", "1e", "abc", "5n"] {
            assert!(!is_number(no), "{no}");
        }
    }
}
