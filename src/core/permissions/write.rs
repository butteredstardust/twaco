//! Write a permission block into entity XML, changing nothing else.
//!
//! A block whose grants already are the wanted ones is left byte for byte. One that differs is
//! replaced whole, in the layout of a ThingWorx export (one attribute per line, four-space steps
//! from the block's own indent, the file's line ending). What stays keeps its order; what is new
//! follows, ordered by the caller's rank (a role's position in the policy, a helper row's).

use super::{elements, from_xml, Grant, Grants, KindKey, PermissionsError, RUN_TIME_ACTIONS};
use crate::core::entity_carry::Kind;
use crate::core::normalise::{self, Element};
use crate::core::scan;
use crate::core::splice::{self, Edit};
use std::collections::BTreeMap;

const PERMISSION_ELEMENTS: [&str; 6] = [
    "RunTimePermissions",
    "DesignTimePermissions",
    "VisibilityPermissions",
    "InstanceRunTimePermissions",
    "InstanceDesignTimePermissions",
    "InstanceVisibilityPermissions",
];

fn error(message: impl Into<String>) -> PermissionsError {
    PermissionsError(message.into())
}

/// How new entries are ordered: lower first, then by name.
pub struct Order<'a> {
    pub principal: &'a dyn Fn(&str) -> usize,
    pub resource: &'a dyn Fn(&str) -> usize,
}

/// The grants of one block, in document order.
fn document_order(entity: &Element, kind: Kind) -> Vec<(Grant, bool)> {
    let mut out = Vec::new();
    let Some(block) = elements(entity).find(|e| e.name == kind.element().as_bytes()) else {
        return out;
    };
    let mut principals = |resource: &str, action: &Element| {
        let action_name = String::from_utf8_lossy(&action.name).into_owned();
        for principal in elements(action).filter(|e| e.name == b"Principal") {
            out.push((
                Grant {
                    resource: resource.to_string(),
                    action: action_name.clone(),
                    principal: super::attribute(principal, "name")
                        .unwrap_or_default()
                        .to_string(),
                    principal_type: super::attribute(principal, "type")
                        .unwrap_or_default()
                        .to_string(),
                },
                super::attribute(principal, "isPermitted") != Some("false"),
            ));
        }
    };
    if kind.is_run_time() {
        for permissions in elements(block).filter(|e| e.name == b"Permissions") {
            let resource = super::attribute(permissions, "resourceName").unwrap_or("*");
            for action in elements(permissions) {
                principals(resource, action);
            }
        }
    } else {
        for action in elements(block) {
            principals("", action);
        }
    }
    out
}

/// The wanted grants in the order they are written: what the block already lists, as it lists
/// it, then the rest by rank.
fn ordered(existing: &[(Grant, bool)], wanted: &Grants, order: &Order<'_>) -> Vec<(Grant, bool)> {
    let mut out: Vec<(Grant, bool)> = existing
        .iter()
        .filter(|(grant, _)| wanted.contains_key(grant))
        .map(|(grant, _)| (grant.clone(), wanted[grant]))
        .collect();
    let mut new: Vec<(Grant, bool)> = wanted
        .iter()
        .filter(|(grant, _)| !existing.iter().any(|(g, _)| g == *grant))
        .map(|(grant, allowed)| (grant.clone(), *allowed))
        .collect();
    new.sort_by(|(a, _), (b, _)| {
        (
            (order.principal)(&a.principal),
            &a.principal,
            &a.principal_type,
        )
            .cmp(&(
                (order.principal)(&b.principal),
                &b.principal,
                &b.principal_type,
            ))
    });
    out.extend(new);
    out
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn principal_xml(out: &mut String, grant: &Grant, allowed: bool, indent: &str, eol: &str) {
    out.push_str(&format!(
        "{indent}<Principal{eol}{indent} isPermitted=\"{allowed}\"{eol}{indent} name=\"{}\"{eol}{indent} type=\"{}\"></Principal>{eol}",
        escape(&grant.principal),
        escape(&grant.principal_type)
    ));
}

/// The block's XML, starting at its opening tag and ending at its closing tag.
fn render(kind: Kind, grants: &[(Grant, bool)], indent: &str, eol: &str) -> String {
    let tag = kind.element();
    if grants.is_empty() && kind.is_run_time() {
        return format!("<{tag}></{tag}>");
    }
    let step = |n: usize| format!("{indent}{}", " ".repeat(4 * n));
    let mut out = format!("<{tag}>{eol}");
    if kind.is_run_time() {
        // Resources in first-seen order: the block's own order, new ones after by rank.
        let mut resources: Vec<&str> = Vec::new();
        for (grant, _) in grants {
            if !resources.contains(&grant.resource.as_str()) {
                resources.push(&grant.resource);
            }
        }
        for resource in resources {
            out.push_str(&format!(
                "{}<Permissions{eol}{} resourceName=\"{}\">{eol}",
                step(1),
                step(1),
                escape(resource)
            ));
            for action in RUN_TIME_ACTIONS {
                let listed: Vec<&(Grant, bool)> = grants
                    .iter()
                    .filter(|(g, _)| g.resource == resource && g.action == action)
                    .collect();
                if listed.is_empty() {
                    out.push_str(&format!("{}<{action}></{action}>{eol}", step(2)));
                } else {
                    out.push_str(&format!("{}<{action}>{eol}", step(2)));
                    for (grant, allowed) in listed {
                        principal_xml(&mut out, grant, *allowed, &step(3), eol);
                    }
                    out.push_str(&format!("{}</{action}>{eol}", step(2)));
                }
            }
            out.push_str(&format!("{}</Permissions>{eol}", step(1)));
        }
    } else {
        let actions: Vec<String> = if kind.is_design_time() {
            super::DESIGN_TIME_ACTIONS
                .iter()
                .map(|a| a.to_string())
                .collect()
        } else {
            vec!["Visibility".to_string()]
        };
        for action in actions {
            let listed: Vec<&(Grant, bool)> =
                grants.iter().filter(|(g, _)| g.action == action).collect();
            if listed.is_empty() {
                out.push_str(&format!("{}<{action}></{action}>{eol}", step(1)));
            } else {
                out.push_str(&format!("{}<{action}>{eol}", step(1)));
                for (grant, allowed) in listed {
                    principal_xml(&mut out, grant, *allowed, &step(2), eol);
                }
                out.push_str(&format!("{}</{action}>{eol}", step(1)));
            }
        }
    }
    out.push_str(&format!("{indent}</{tag}>"));
    out
}

/// The whitespace a line holds before `at`, if only whitespace precedes it on its line.
fn indent_before(src: &[u8], at: usize) -> String {
    let line_start = src[..at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let before = &src[line_start..at];
    if before.iter().all(|b| *b == b' ' || *b == b'\t') {
        String::from_utf8_lossy(before).into_owned()
    } else {
        String::new()
    }
}

/// One edit per place. Two missing blocks can go to the same place (both before the closing
/// tag, or both after the same block), and one can follow a block that is itself replaced; splice
/// takes neither two insertions at one offset nor an insertion touching a replacement.
fn merge(edits: Vec<Edit>) -> Vec<Edit> {
    let (mut replaced, inserted): (Vec<Edit>, Vec<Edit>) =
        edits.into_iter().partition(|edit| !edit.span.is_empty());
    let mut at_offset: Vec<Edit> = Vec::new();
    for insertion in inserted {
        let offset = insertion.span.start;
        if let Some(edit) = replaced.iter_mut().find(|e| e.span.end == offset) {
            edit.replacement.extend(insertion.replacement);
        } else if let Some(edit) = at_offset.iter_mut().find(|e| e.span.start == offset) {
            edit.replacement.extend(insertion.replacement);
        } else {
            at_offset.push(insertion);
        }
    }
    replaced.extend(at_offset);
    replaced.sort_by_key(|edit| (edit.span.start, edit.span.end));
    replaced
}

/// Make each named set of the entity exactly `wanted`. Returns the new bytes, the same bytes
/// when nothing differs. A missing block is added after the entity's last permission block, or
/// before its closing tag.
pub fn rewrite(
    src: &[u8],
    wanted: &BTreeMap<KindKey, Grants>,
    order: &Order<'_>,
) -> Result<Vec<u8>, PermissionsError> {
    let current = from_xml(src)?;
    let entity = normalise::entity_of(src).map_err(|e| error(e.to_string()))?;
    let scanned = scan::scan(src).map_err(|e| error(e.to_string()))?;
    let tokens = &scanned.tokens;
    let root = crate::core::sidecar::entity_element(tokens, src)
        .ok_or_else(|| error("no entity element"))?;
    let text = String::from_utf8_lossy(src);
    let eol = crate::core::workspace::line_ending(&text);
    let mut edits = Vec::new();
    for (key, grants) in wanted {
        let kind = key.kind();
        if current.get(key) == Some(grants) {
            continue;
        }
        let existing = document_order(&entity, kind);
        let mut rows = ordered(&existing, grants, order);
        if kind.is_run_time() {
            // New resources go after the ones the block has, by rank, then name.
            let known: Vec<String> = existing.iter().map(|(g, _)| g.resource.clone()).collect();
            let position = |resource: &str| known.iter().position(|r| r == resource);
            rows.sort_by(|(a, _), (b, _)| {
                let rank = |g: &Grant| match position(&g.resource) {
                    Some(at) => (0, at, 0, String::new()),
                    None => (1, 0, (order.resource)(&g.resource), g.resource.clone()),
                };
                rank(a).cmp(&rank(b))
            });
        }
        let blocks: Vec<usize> = scan::child_tags(tokens, src, kind.element(), root);
        match blocks.as_slice() {
            [at] => {
                let span = scan::element_span(tokens, *at)
                    .ok_or_else(|| error(format!("{} is not closed", kind.element())))?;
                let indent = indent_before(src, span.start);
                edits.push(Edit::new(
                    span,
                    render(kind, &rows, &indent, eol).into_bytes(),
                ));
            }
            [] => {
                // The entity's own permission blocks, never one nested deeper.
                let last = PERMISSION_ELEMENTS
                    .iter()
                    .flat_map(|name| scan::child_tags(tokens, src, name, root))
                    .max();
                if let Some(last) = last {
                    let span = scan::element_span(tokens, last)
                        .ok_or_else(|| error("a permission block is not closed"))?;
                    let indent = indent_before(src, span.start);
                    let block = render(kind, &rows, &indent, eol);
                    edits.push(Edit::new(
                        scan::Span::new(span.end, span.end),
                        format!("{eol}{indent}{block}").into_bytes(),
                    ));
                } else {
                    let close = scan::element_end_in(tokens, src, root)
                        .filter(|&end| end != root)
                        .ok_or_else(|| error("the entity element has no closing tag"))?;
                    let at = tokens[close].span.start;
                    let indent = format!("{}    ", indent_before(src, at));
                    let block = render(kind, &rows, &indent, eol);
                    let close_indent = indent_before(src, at);
                    edits.push(Edit::new(
                        scan::Span::new(at, at),
                        format!("    {block}{eol}{close_indent}").into_bytes(),
                    ));
                }
            }
            _ => {
                return Err(error(format!(
                    "the entity has two {} blocks",
                    kind.element()
                )))
            }
        }
    }
    if edits.is_empty() {
        return Ok(src.to_vec());
    }
    let edits = merge(edits);
    let out = splice::splice(src, &edits).map_err(|e| error(e.to_string()))?;
    // What was written must read back as wanted, and nothing but permissions may have changed.
    let back = from_xml(&out)?;
    for (key, grants) in wanted {
        if back.get(key) != Some(grants) {
            return Err(error(format!(
                "the rewritten {} block does not read back as intended",
                key.kind().element()
            )));
        }
    }
    let same = normalise::normalise(src).ok() == normalise::normalise(&out).ok();
    if !same && !normalise::differ_only_in_permissions(src, &out) {
        return Err(error(
            "rewriting the permission blocks changed something else",
        ));
    }
    Ok(out)
}
