//! The one walk up a Thing's or template's template chain.
//!
//! The index and the service catalog both need it, over different representations of an entity
//! (a graph node, a model entity), so it is written once over the three facts it reads: which
//! collection the entity is in, its template, and its implemented shapes.

use std::collections::BTreeSet;

/// What the walk reads of one entity.
#[derive(Clone, Copy)]
pub(crate) struct Inherits<'a> {
    pub collection: &'a str,
    /// A Thing's `thingTemplate` or a template's `baseThingTemplate`.
    pub template: Option<&'a str>,
    /// The ThingShapes it implements, as written and in order.
    pub shapes: &'a [String],
}

/// The templates and shapes `start` inherits, nearest first: its own shapes, then its template's
/// name, then that template's shapes, then its base template's name, and so on up to a platform
/// template that `template_named` does not know. A ThingShape inherits nothing here. Each name is
/// reported once and a cycle ends the walk.
pub(crate) fn inheritance_chain<'a>(
    start: Inherits<'a>,
    template_named: impl Fn(&str) -> Option<Inherits<'a>>,
) -> Vec<String> {
    if start.collection == "ThingShapes" {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut visited = BTreeSet::new();
    let mut current = Some(start);
    while let Some(item) = current {
        for shape in item.shapes {
            if visited.insert(shape.clone()) {
                out.push(shape.clone());
            }
        }
        let Some(template) = item.template else {
            break;
        };
        if !visited.insert(template.to_string()) {
            break;
        }
        out.push(template.to_string());
        current = template_named(template);
    }
    out
}

/// Whether `start` implements the ThingShape named `shape`, directly or through its templates.
pub(crate) fn implements_shape<'a>(
    start: Inherits<'a>,
    shape: &str,
    template_named: impl Fn(&str) -> Option<Inherits<'a>>,
) -> bool {
    let mut visited = BTreeSet::new();
    let mut current = Some(start);
    while let Some(item) = current {
        if item.shapes.iter().any(|name| name == shape) {
            return true;
        }
        let Some(template) = item.template else {
            return false;
        };
        if !visited.insert(template.to_string()) {
            return false;
        }
        current = template_named(template);
    }
    false
}
