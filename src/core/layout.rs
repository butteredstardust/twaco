//! Source-control presentation of incoming entity bytes, using scanner spans and splices.
//!
//! The bundled corpus uses four spaces per XML depth (entities start at depth two),
//! lexicographically ordered attributes, one attribute per line one space past the tag,
//! paired empty tags, and inline text leaves. Its CDATA has two conventions: inline row
//! values, and block scripts/mashup content. Block markers sit one level inside the leaf;
//! scripts obey `indent_cdata_payload`, while other payloads retain their existing margin.
//! The destination is the layout oracle: matching leaves retain inline/block presentation,
//! CDATA framing, line endings and margins. New leaves inherit a same-tag example or use
//! block CDATA with its XML indentation. This also preserves compact hand-written files.
//! The corpus's `accumulatedDataShape/json` stays nested inline, and service `Row`/`code`
//! margins are sometimes wider than their depth: these are learned from the guide, not
//! corrected. A multi-line source-control opening tag supplies its own guide when omitted.
//!
//! Equality and set membership come from the normaliser's comparison tree, exactly as for
//! `normalise::hash`. An equivalent subtree retains the destination bytes verbatim, including
//! JSON/base64 wrapping, CDATA framing, and ignored metadata. Changed nodes retain equivalent
//! children and opening-tag lexemes; only changed attribute values are spliced when possible.
//! Counterparts use names, principal name+type, permission resourceName, connection endpoints,
//! and adopt's configuration-row keys, with same-tag position only for unkeyed siblings.
//! Set lists keep repository order for common members; new members follow in export order.
//! Ordered lists keep incoming order. Changed JSON is one payload, rendered with the shared
//! Jackson renderer when the destination uses that layout; other payload layouts follow the
//! incoming bytes and destination CDATA framing. New entities drop only the exact metadata
//! the normaliser drops; nested names that happen to resemble live metadata remain intact.

use super::scan::{self, Kind, Span, Token};
use super::splice::{self, Edit};
use super::{normalise, sidecar, sync, workspace};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
#[error("cannot lay out entity XML: {0}")]
pub struct LayoutError(String);

struct Document<'a> {
    src: &'a [u8],
    tokens: Vec<Token>,
    comparison: BTreeMap<usize, Comparison>,
    identities: BTreeMap<usize, Option<Vec<u8>>>,
    preserve: bool,
    retain_ignored: bool,
}

struct Comparison {
    bytes: Vec<u8>,
    header: Vec<u8>,
    attributes: Vec<(Vec<u8>, Vec<u8>)>,
    unordered: bool,
}

impl<'a> Document<'a> {
    fn new(src: &'a [u8]) -> Result<Self, LayoutError> {
        let tokens = scan::tokenize(src).map_err(|e| LayoutError(e.to_string()))?;
        fn collect(element: &normalise::Element, out: &mut BTreeMap<usize, Comparison>) {
            let mut header = element.clone();
            header.children.clear();
            out.insert(
                element.source_start,
                Comparison {
                    bytes: normalise::canonical_element_bytes(element),
                    header: normalise::canonical_element_bytes(&header),
                    attributes: element.attributes.clone(),
                    unordered: element.unordered,
                },
            );
            for child in &element.children {
                if let normalise::Node::Element(child) = child {
                    collect(child, out);
                }
            }
        }
        fn identities(element: &normalise::Element, out: &mut BTreeMap<usize, Option<Vec<u8>>>) {
            let attr = |name: &[u8]| {
                element
                    .attributes
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.clone())
            };
            let key = if element.name == b"Principal" {
                attr(b"name")
                    .map(|name| [name, vec![0], attr(b"type").unwrap_or_default()].concat())
            } else if element.name == b"Connection" {
                attr(b"from").map(|from| [from, vec![0], attr(b"to").unwrap_or_default()].concat())
            } else {
                attr(b"name")
                    .or_else(|| attr(b"resourceName"))
                    .or_else(|| super::adopt::row_key(element).map(String::into_bytes))
            };
            out.insert(element.source_start, key);
            for child in &element.children {
                if let normalise::Node::Element(child) = child {
                    identities(child, out);
                }
            }
        }
        let mut comparison = BTreeMap::new();
        collect(
            &normalise::comparison_tree(src, true).map_err(|e| LayoutError(e.to_string()))?,
            &mut comparison,
        );
        let mut keys = BTreeMap::new();
        identities(
            &normalise::entity_of(src).map_err(|e| LayoutError(e.to_string()))?,
            &mut keys,
        );
        Ok(Self {
            src,
            tokens,
            comparison,
            identities: keys,
            preserve: true,
            retain_ignored: true,
        })
    }

    fn end(&self, at: usize) -> Result<usize, LayoutError> {
        scan::element_end_in(&self.tokens, self.src, at)
            .ok_or_else(|| LayoutError("unclosed or mismatched element".into()))
    }

    fn name(&self, at: usize) -> &[u8] {
        self.tokens[at].name.of(self.src)
    }

    fn children(&self, at: usize) -> Result<Vec<usize>, LayoutError> {
        let end = self.end(at)?;
        let mut result = Vec::new();
        let mut i = at + 1;
        while i < end {
            if matches!(self.tokens[i].kind, Kind::Start | Kind::Empty) {
                result.push(i);
                i = self.end(i)? + 1;
            } else {
                i += 1;
            }
        }
        Ok(result)
    }

    fn multiline(&self, at: usize) -> bool {
        self.tokens[at..=self.end(at).unwrap_or(at)]
            .iter()
            .any(|t| {
                (matches!(t.kind, Kind::Start | Kind::Empty)
                    || t.kind == Kind::Text && xml_space(t.span.of(self.src)))
                    && t.span.of(self.src).contains(&b'\n')
            })
    }

    fn indent(&self, at: usize) -> Option<&str> {
        let before = &self.src[..self.tokens[at].span.start];
        let line = before.rsplit(|b| *b == b'\n').next()?;
        if before.contains(&b'\n') && xml_space(line) {
            std::str::from_utf8(line).ok()
        } else {
            None
        }
    }

    fn matching(&self, parent: usize, source: &Document<'_>, at: usize) -> Option<usize> {
        let children = self.children(parent).ok()?;
        let same_tag = |i: &usize| self.name(*i) == source.name(at);
        let identity = &source.identities[&source.tokens[at].span.start];
        if identity.is_some() {
            return children
                .into_iter()
                .filter(same_tag)
                .find(|i| self.identities[&self.tokens[*i].span.start] == *identity);
        }
        // Unkeyed repeated tags use their occurrence among siblings of that tag.
        let source_parent = (0..at).rev().find(|i| {
            source.tokens[*i].kind == Kind::Start && source.end(*i).is_ok_and(|end| end > at)
        })?;
        let ordinal = source
            .children(source_parent)
            .ok()?
            .into_iter()
            .take_while(|i| *i != at)
            .filter(|i| source.name(*i) == source.name(at))
            .count();
        children.into_iter().filter(same_tag).nth(ordinal)
    }

    fn comparison(&self, at: usize) -> Option<&Comparison> {
        self.comparison.get(&self.tokens[at].span.start)
    }
}

/// Lay out one entity element, using its destination (or a collection peer) as a whitespace
/// guide. The envelope belongs to the caller and is never rebuilt. Refuses malformed markup.
pub fn entity(
    src: &[u8],
    reference: Option<&[u8]>,
    indent_cdata_payload: bool,
) -> Result<Vec<u8>, LayoutError> {
    let doc = Document::new(src)?;
    let root = doc
        .tokens
        .iter()
        .position(|t| matches!(t.kind, Kind::Start | Kind::Empty))
        .ok_or_else(|| LayoutError("no entity element".into()))?;
    let explicit_reference = reference.is_some();
    let mut reference = reference
        .or_else(|| {
            doc.tokens[root]
                .span
                .of(src)
                .contains(&b'\n')
                .then_some(src)
        })
        .map(Document::new)
        .transpose()?;
    if let Some(d) = &mut reference {
        let reference_root = d
            .tokens
            .iter()
            .position(|t| matches!(t.kind, Kind::Start | Kind::Empty))
            .unwrap();
        // A collection peer teaches presentation, but its ignored entity metadata belongs
        // to another entity. An implicit guide must still strip incoming live metadata.
        d.preserve = explicit_reference;
        d.retain_ignored = explicit_reference
            && d.name(reference_root) == doc.name(root)
            && d.identities[&d.tokens[reference_root].span.start]
                == doc.identities[&doc.tokens[root].span.start];
    }
    let guide = reference.as_ref().and_then(|d| {
        d.tokens
            .iter()
            .position(|t| matches!(t.kind, Kind::Start | Kind::Empty))
            .map(|at| (d, at))
    });
    let newline = workspace::line_ending(
        std::str::from_utf8(reference.as_ref().map_or(src, |r| r.src)).unwrap_or(""),
    );
    let mut edits = Vec::new();
    lay_out(
        &doc,
        root,
        guide,
        2,
        newline,
        indent_cdata_payload,
        &mut edits,
    )?;
    splice::splice(src, &edits).map_err(|e| LayoutError(e.to_string()))
}

#[allow(clippy::too_many_arguments)]
fn lay_out(
    doc: &Document<'_>,
    at: usize,
    guide: Option<(&Document<'_>, usize)>,
    depth: usize,
    newline: &str,
    indent_cdata_payload: bool,
    edits: &mut Vec<Edit>,
) -> Result<(), LayoutError> {
    let token = &doc.tokens[at];
    let end = doc.end(at)?;
    if let Some((d, i)) = guide.filter(|(d, _)| d.preserve) {
        if doc
            .comparison(at)
            .zip(d.comparison(i))
            .is_some_and(|(a, b)| a.bytes == b.bytes)
        {
            edits.push(Edit::new(
                Span::new(token.span.start, doc.tokens[end].span.end),
                d.src[d.tokens[i].span.start..d.tokens[d.end(i)?].span.end].to_vec(),
            ));
            return Ok(());
        }
    }
    let name = std::str::from_utf8(doc.name(at)).unwrap();
    let multiline = guide.is_none_or(|(d, i)| {
        d.multiline(i) || d.children(i).is_ok_and(|c| c.is_empty()) && d.multiline(0)
    });
    let indent = guide
        .and_then(|(d, i)| d.indent(i))
        .map(str::to_string)
        .unwrap_or_else(|| " ".repeat(depth * 4));
    let mut attributes =
        scan::attributes(doc.src, token).map_err(|e| LayoutError(e.to_string()))?;
    attributes.retain(|a| a.name.of(doc.src) != b"lastModifiedDate");
    if multiline {
        attributes.sort_by_key(|a| a.name.of(doc.src));
    } else if let Some((d, i)) = guide {
        let order =
            scan::attributes(d.src, &d.tokens[i]).map_err(|e| LayoutError(e.to_string()))?;
        attributes.sort_by_key(|a| {
            order
                .iter()
                .position(|b| a.name.of(doc.src) == b.name.of(d.src))
                .unwrap_or(usize::MAX)
        });
    }
    let mut tag = format!("<{name}");
    for a in attributes {
        if multiline {
            tag.push_str(&format!("{newline}{indent} "));
        } else {
            tag.push(' ');
        }
        tag.push_str(std::str::from_utf8(&doc.src[a.name.start..a.value.end + 1]).unwrap());
    }
    let self_closed = token.kind == Kind::Empty;
    // A hand-written destination may deliberately self-close its empty leaves.
    let keep_empty = self_closed && guide.is_some_and(|(d, i)| d.tokens[i].kind == Kind::Empty);
    if keep_empty {
        tag.push_str("/>");
    } else {
        tag.push('>');
        if self_closed {
            tag.push_str(&format!("</{name}>"));
        }
    }
    let tag = if let Some((d, i)) = guide.filter(|(d, i)| {
        d.preserve
            && doc
                .comparison(at)
                .zip(d.comparison(*i))
                .is_some_and(|(a, b)| a.header == b.header)
            && (d.tokens[*i].kind == Kind::Empty) == self_closed
    }) {
        d.tokens[i].span.of(d.src).to_vec()
    } else if let Some((d, i)) = guide.filter(|(d, i)| {
        d.preserve
            && doc
                .comparison(at)
                .zip(d.comparison(*i))
                .is_some_and(|(a, b)| {
                    a.attributes
                        .iter()
                        .map(|a| &a.0)
                        .eq(b.attributes.iter().map(|a| &a.0))
                })
            && (d.tokens[*i].kind == Kind::Empty) == self_closed
    }) {
        let ours = scan::attributes(d.src, &d.tokens[i]).map_err(|e| LayoutError(e.to_string()))?;
        let theirs = scan::attributes(doc.src, token).map_err(|e| LayoutError(e.to_string()))?;
        let a = &doc.comparison(at).unwrap().attributes;
        let b = &d.comparison(i).unwrap().attributes;
        let mut changes = Vec::new();
        for ((name, value), (_, old)) in a.iter().zip(b) {
            if value != old {
                let from = theirs
                    .iter()
                    .find(|attr| attr.name.of(doc.src) == name)
                    .unwrap();
                let to = ours
                    .iter()
                    .find(|attr| attr.name.of(d.src) == name)
                    .unwrap();
                changes.push(Edit::new(
                    Span::new(to.name.start, to.value.end + 1),
                    doc.src[from.name.start..from.value.end + 1].to_vec(),
                ));
            }
        }
        render_span(d.src, d.tokens[i].span, &changes)?
    } else {
        tag.into_bytes()
    };
    edits.push(Edit::new(token.span, tag));
    if self_closed {
        return Ok(());
    }

    let children = doc.children(at)?;
    if children.is_empty() {
        let cdata: Vec<_> = doc.tokens[at + 1..end]
            .iter()
            .filter(|t| t.kind == Kind::Cdata)
            .collect();
        if let (Some(first), Some(last)) = (cdata.first(), cdata.last()) {
            // Mixed text/CDATA is meaningful, and has no indentation-only framing to replace.
            if doc.tokens[at + 1..end].iter().any(|t| {
                t.kind != Kind::Cdata && (t.kind != Kind::Text || !xml_space(t.span.of(doc.src)))
            }) {
                return Ok(());
            }
            let sample = guide.and_then(|(d, i)| {
                if d.tokens[i].kind == Kind::Empty {
                    return None;
                }
                let e = d.end(i).ok()?;
                let parts: Vec<_> = d.tokens[i + 1..e]
                    .iter()
                    .filter(|t| t.kind == Kind::Cdata)
                    .collect();
                Some((d, i, *parts.first()?, *parts.last()?))
            });
            // A payload split across CDATA sections is text with CDATA inside: whitespace around
            // the sections is part of it, so a new frame would change the value. With no file of
            // its own to copy, it stays as it came.
            if cdata.len() > 1 && sample.is_none() {
                return Ok(());
            }
            let block = sample.map_or(multiline, |(d, i, f, _)| {
                d.src[d.tokens[i].span.end..f.span.start].contains(&b'\n')
            });
            let raw = std::str::from_utf8(&doc.src[first.inner.start..last.inner.end]).unwrap();
            let mut payload = raw.trim_matches([' ', '\t', '\r', '\n']).to_string();
            if name == "mashupContent"
                || name == "content" && matches!(doc.name(0), b"StateDefinition" | b"StyleTheme")
            {
                if let Some((d, _, f, l)) = sample {
                    let existing = std::str::from_utf8(&d.src[f.inner.start..l.inner.end]).unwrap();
                    if let (Ok(old), Ok(new)) = (
                        serde_json::from_str::<serde_json::Value>(existing.trim()),
                        serde_json::from_str::<serde_json::Value>(&payload),
                    ) {
                        if existing.trim().replace("\r\n", "\n")
                            == super::entity::new::jackson_pretty(&old)
                        {
                            payload =
                                super::entity::new::jackson_pretty(&new).replace('\n', newline);
                        }
                    }
                }
            }
            let mut prefix = String::new();
            let mut suffix = String::new();
            if block {
                prefix = format!("{newline}{indent}    ");
                suffix = format!("{newline}{indent}");
                if let Some((d, i, f, l)) = sample {
                    prefix = std::str::from_utf8(&d.src[d.tokens[i].span.end..f.span.start])
                        .unwrap()
                        .to_string();
                    suffix =
                        std::str::from_utf8(&d.src[l.span.end..d.tokens[d.end(i)?].span.start])
                            .unwrap()
                            .to_string();
                }
                let margin = sample
                    .map(|(d, _, _, l)| {
                        let text = std::str::from_utf8(l.inner.of(d.src)).unwrap();
                        text.rsplit('\n')
                            .next()
                            .filter(|s| s.trim().is_empty())
                            .unwrap_or("")
                            .to_string()
                    })
                    .unwrap_or_else(|| format!("{indent}    "));
                if name == "code" {
                    let existing = sample.map_or(margin.as_str(), |(d, _, f, _)| {
                        std::str::from_utf8(f.inner.of(d.src)).unwrap()
                    });
                    // Exporter trims the first line's XML margin, but leaves the margin on
                    // subsequent lines. Restore it before dedenting, rather than indenting
                    // those lines a second time. A flush-left body has no such shared margin.
                    let payload_margin = existing
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .map(|line| {
                            &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
                        })
                        .unwrap_or("");
                    let later_lines: Vec<_> = raw
                        .lines()
                        .skip(1)
                        .filter(|line| !line.trim().is_empty())
                        .collect();
                    let restored;
                    let script = if !raw.starts_with(['\r', '\n'])
                        && !payload_margin.is_empty()
                        && !later_lines.is_empty()
                        && later_lines
                            .iter()
                            .all(|line| line.starts_with(payload_margin))
                    {
                        restored = format!("{payload_margin}{raw}");
                        restored.as_str()
                    } else {
                        raw
                    };
                    payload = sync::render_payload(
                        existing,
                        sidecar::dedent(script).trim_matches('\n'),
                        newline,
                        indent_cdata_payload,
                    );
                    // The XML marker's margin is independent of the script's margin.
                    if !indent_cdata_payload {
                        payload.push_str(&margin);
                    }
                } else if let Some((d, _, f, l)) = sample {
                    let sample_raw =
                        std::str::from_utf8(&d.src[f.inner.start..l.inner.end]).unwrap();
                    if sample_raw.trim_matches([' ', '\t', '\r', '\n']).is_empty() {
                        payload = if payload.is_empty() {
                            sample_raw.to_string()
                        } else {
                            format!("{newline}{margin}{payload}{newline}{margin}")
                        };
                    } else {
                        let leading = &sample_raw[..sample_raw.len()
                            - sample_raw.trim_start_matches([' ', '\t', '\r', '\n']).len()];
                        let trailing = &sample_raw
                            [sample_raw.trim_end_matches([' ', '\t', '\r', '\n']).len()..];
                        payload = format!("{leading}{payload}{trailing}");
                    }
                } else {
                    payload = format!("{newline}{margin}{payload}{newline}{margin}");
                }
            } else if let Some((d, _, f, l)) = sample {
                let sample_raw = std::str::from_utf8(&d.src[f.inner.start..l.inner.end]).unwrap();
                let body = sample_raw.trim_matches([' ', '\t', '\r', '\n']);
                payload = if body.is_empty() && payload.is_empty() {
                    sample_raw.to_string()
                } else if body.is_empty() {
                    payload
                } else {
                    let leading = &sample_raw[..sample_raw.len()
                        - sample_raw.trim_start_matches([' ', '\t', '\r', '\n']).len()];
                    let trailing =
                        &sample_raw[sample_raw.trim_end_matches([' ', '\t', '\r', '\n']).len()..];
                    format!("{leading}{payload}{trailing}")
                };
            }
            edits.push(Edit::new(
                Span::new(token.span.end, first.span.start),
                prefix.into_bytes(),
            ));
            edits.push(Edit::new(
                Span::new(first.inner.start, last.inner.end),
                payload.into_bytes(),
            ));
            edits.push(Edit::new(
                Span::new(last.span.end, doc.tokens[end].span.start),
                suffix.into_bytes(),
            ));
        }
        return Ok(());
    }

    let mut chunks = Vec::new();
    let mut previous = token.span.end;
    for child in children {
        let start = doc.tokens[child].span.start;
        let matched = guide.and_then(|(d, i)| {
            (d.retain_ignored || doc.comparison(child).is_some())
                .then(|| d.matching(i, doc, child))
                .flatten()
        });
        let source_gap = &doc.src[previous..start];
        previous = doc.tokens[doc.end(child)?].span.end;
        // A missing comparison node is metadata or an empty permission kind the hash drops.
        // Retain its repository spelling when present; server-only fill-ins add no content.
        if doc.comparison(child).is_none()
            && !guide.is_some_and(|(d, _)| matched.is_some_and(|i| d.comparison(i).is_none()))
        {
            if !xml_space(source_gap) {
                chunks.push((usize::MAX, source_gap.to_vec()));
            }
            continue;
        }
        let child_guide = guide.and_then(|(d, _)| matched.map(|i| (d, i)));
        let mut gap = source_gap.to_vec();
        if xml_space(source_gap) {
            gap = if let Some((d, i)) = child_guide {
                let parent = guide.unwrap().1;
                let siblings = d.children(parent)?;
                let pos = siblings.iter().position(|c| *c == i).unwrap();
                let before = if pos == 0 {
                    d.tokens[parent].span.end
                } else {
                    d.tokens[d.end(siblings[pos - 1])?].span.end
                };
                let existing = &d.src[before..d.tokens[i].span.start];
                if xml_space(existing) {
                    existing.to_vec()
                } else if multiline {
                    format!("{newline}{indent}    ").into_bytes()
                } else {
                    Vec::new()
                }
            } else if multiline {
                format!("{newline}{indent}    ").into_bytes()
            } else {
                Vec::new()
            };
        }
        let mut child_edits = Vec::new();
        if doc.comparison(child).is_none() {
            let (d, i) = child_guide.unwrap();
            gap.extend_from_slice(&d.src[d.tokens[i].span.start..d.tokens[d.end(i)?].span.end]);
        } else {
            // A new member may learn presentation from a peer, but never its identity/order.
            let sample = child_guide.or_else(|| {
                guide.and_then(|(d, i)| {
                    d.children(i)
                        .ok()?
                        .into_iter()
                        .find(|c| d.name(*c) == doc.name(child))
                        .map(|c| (d, c))
                })
            });
            lay_out(
                doc,
                child,
                sample,
                depth + 1,
                newline,
                indent_cdata_payload,
                &mut child_edits,
            )?;
            gap.extend(render_span(
                doc.src,
                Span::new(start, previous),
                &child_edits,
            )?);
        }
        chunks.push((matched.unwrap_or(usize::MAX), gap));
    }
    if let Some((d, i)) = guide.filter(|(d, _)| d.retain_ignored) {
        // Keep repository order for common members, then append new members in export order.
        // Empty repository permission kinds are presentation too, even if the export omits them.
        for child in d.children(i)? {
            if d.comparison(child).is_none() && !chunks.iter().any(|(index, _)| *index == child) {
                let mut bytes = if multiline {
                    format!("{newline}{indent}    ").into_bytes()
                } else {
                    Vec::new()
                };
                bytes.extend_from_slice(
                    &d.src[d.tokens[child].span.start..d.tokens[d.end(child)?].span.end],
                );
                let position = chunks
                    .iter()
                    .position(|(index, _)| *index > child)
                    .unwrap_or(chunks.len());
                chunks.insert(position, (child, bytes));
            }
        }
        if doc.comparison(at).is_some_and(|c| c.unordered) {
            chunks.sort_by_key(|(index, _)| *index);
        }
    }
    let mut inner: Vec<u8> = chunks.into_iter().flat_map(|(_, bytes)| bytes).collect();
    let finish = doc.tokens[end].span.start;
    let trailing = &doc.src[previous..finish];
    if xml_space(trailing) {
        let sample = guide
            .map(|(d, i)| {
                let children = d.children(i)?;
                let before = match children.last() {
                    Some(c) => d.tokens[d.end(*c)?].span.end,
                    None => d.tokens[i].span.end,
                };
                let finish = if d.tokens[i].kind == Kind::Empty {
                    before
                } else {
                    d.tokens[d.end(i)?].span.start
                };
                Ok::<_, LayoutError>(&d.src[before..finish])
            })
            .transpose()?;
        if let Some(sample) = sample.filter(|bytes| {
            xml_space(bytes) && (!bytes.is_empty() || inner.is_empty() || !multiline)
        }) {
            inner.extend_from_slice(sample);
        } else if !inner.is_empty() && multiline {
            inner.extend_from_slice(format!("{newline}{indent}").as_bytes());
        }
    } else {
        inner.extend_from_slice(trailing);
    }
    edits.push(Edit::new(Span::new(token.span.end, finish), inner));

    Ok(())
}

fn render_span(src: &[u8], span: Span, edits: &[Edit]) -> Result<Vec<u8>, LayoutError> {
    let edits: Vec<_> = edits
        .iter()
        .map(|edit| {
            Edit::new(
                Span::new(edit.span.start - span.start, edit.span.end - span.start),
                edit.replacement.clone(),
            )
        })
        .collect();
    splice::splice(span.of(src), &edits).map_err(|e| LayoutError(e.to_string()))
}

fn xml_space(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_closed_guides_can_grow_children_or_cdata() {
        let ours = br#"<Thing name="Acme.Asset"><RunTimePermissions/><code/></Thing>"#;
        let theirs = br#"<Thing name="Acme.Asset"><RunTimePermissions><Permissions resourceName="Acme.Run"><ServiceInvoke><Principal name="Acme.Users" type="Group"/></ServiceInvoke></Permissions></RunTimePermissions><code><![CDATA[result = true;]]></code></Thing>"#;
        let output = entity(theirs, Some(ours), false).unwrap();
        assert_eq!(
            normalise::normalise(&output).unwrap(),
            normalise::normalise(theirs).unwrap()
        );
    }

    #[test]
    fn clearing_a_permission_kind_removes_its_repository_grants() {
        let ours = br#"<Thing name="Acme.Asset"><RunTimePermissions><Permissions resourceName="*"><ServiceInvoke><Principal name="Acme.Users" type="Group"/></ServiceInvoke><PropertyRead><Principal name="Acme.Readers" type="Group"/></PropertyRead></Permissions></RunTimePermissions></Thing>"#;
        let theirs = br#"<Thing name="Acme.Asset"><RunTimePermissions><Permissions resourceName="*"><ServiceInvoke/><PropertyRead><Principal name="Acme.Readers" type="Group"/></PropertyRead></Permissions></RunTimePermissions></Thing>"#;
        let output = entity(theirs, Some(ours), false).unwrap();
        assert!(!std::str::from_utf8(&output).unwrap().contains("Acme.Users"));
        assert_eq!(
            normalise::normalise(&output).unwrap(),
            normalise::normalise(theirs).unwrap()
        );
    }

    #[test]
    fn an_implicit_or_peer_guide_does_not_reintroduce_ignored_metadata() {
        let incoming = b"<Thing\n         name=\"Acme.New\" lastModifiedDate=\"1\"><Owner>Acme.Exporter</Owner><value>new</value></Thing>";
        let peer =
            br#"<Thing name="Acme.Peer"><Owner>Acme.PeerOwner</Owner><value>old</value></Thing>"#;
        for guide in [None, Some(peer.as_slice())] {
            let output = entity(incoming, guide, false).unwrap();
            let text = std::str::from_utf8(&output).unwrap();
            assert!(!text.contains("Owner") && !text.contains("lastModifiedDate"));
            assert_eq!(
                normalise::normalise(&output).unwrap(),
                normalise::normalise(incoming).unwrap()
            );
        }
    }

    #[test]
    fn changed_sets_match_principal_type_and_resource_name_and_append_new_members() {
        let ours = br#"<Thing name="Acme.Asset"><RunTimePermissions><Permissions resourceName="Acme.Second"><ServiceInvoke><Principal name="Acme.Users" type="Group" isPermitted="true"/><Principal name="Acme.Users" type="Organization" isPermitted="true"/></ServiceInvoke><PropertyRead/></Permissions><Permissions resourceName="Acme.First"><ServiceInvoke><Principal name="Acme.Readers" type="Group"/></ServiceInvoke></Permissions></RunTimePermissions></Thing>"#;
        let theirs = br#"<Thing name="Acme.Asset"><RunTimePermissions><Permissions resourceName="Acme.New"><ServiceInvoke><Principal name="Acme.NewUsers" type="Group"/></ServiceInvoke></Permissions><Permissions resourceName="Acme.First"><ServiceInvoke><Principal type="Group" name="Acme.Readers"/></ServiceInvoke></Permissions><Permissions resourceName="Acme.Second"><ServiceInvoke><Principal isPermitted="false" type="Organization" name="Acme.Users"/><Principal isPermitted="true" type="Group" name="Acme.Users"/></ServiceInvoke></Permissions></RunTimePermissions></Thing>"#;
        let output = entity(theirs, Some(ours), false).unwrap();
        let text = std::str::from_utf8(&output).unwrap();
        let expected = std::str::from_utf8(ours).unwrap().replace(
            "type=\"Organization\" isPermitted=\"true\"",
            "type=\"Organization\" isPermitted=\"false\"",
        );
        assert!(text.starts_with(expected.trim_end_matches("</RunTimePermissions></Thing>")));
        assert!(text.find("Acme.First").unwrap() < text.find("Acme.New\"").unwrap());
        assert_eq!(
            normalise::normalise(&output).unwrap(),
            normalise::normalise(theirs).unwrap()
        );
    }

    #[test]
    fn ordered_rows_match_keys_and_keep_incoming_order() {
        let ours = br#"<Thing name="Acme.Asset"><Rows><Row><name><![CDATA[ Acme.First ]]></name><value><![CDATA[ old ]]></value></Row><Row><name><![CDATA[ Acme.Second ]]></name><value><![CDATA[ kept ]]></value></Row></Rows></Thing>"#;
        let theirs = br#"<Thing name="Acme.Asset"><Rows><Row><name><![CDATA[Acme.Second]]></name><value><![CDATA[kept]]></value></Row><Row><name><![CDATA[Acme.First]]></name><value><![CDATA[new]]></value></Row></Rows></Thing>"#;
        let output = entity(theirs, Some(ours), false).unwrap();
        assert_eq!(std::str::from_utf8(&output).unwrap(), std::str::from_utf8(ours).unwrap().replace(
            "<Row><name><![CDATA[ Acme.First ]]></name><value><![CDATA[ old ]]></value></Row><Row><name><![CDATA[ Acme.Second ]]></name><value><![CDATA[ kept ]]></value></Row>",
            "<Row><name><![CDATA[ Acme.Second ]]></name><value><![CDATA[ kept ]]></value></Row><Row><name><![CDATA[ Acme.First ]]></name><value><![CDATA[ new ]]></value></Row>"));
    }

    #[test]
    fn changed_json_uses_jackson_layout_and_state_key_order_remains_semantic() {
        let old = serde_json::json!({"value": 1, "items": [true, false]});
        let new = serde_json::json!({"value": 2, "items": [true, false]});
        for tag in ["StateDefinition", "StyleTheme", "Mashup"] {
            let leaf = if tag == "Mashup" {
                "mashupContent"
            } else {
                "content"
            };
            let frame = |payload: &str| {
                format!("<{tag} name=\"Acme.Asset\"><{leaf}><![CDATA[{payload}]]></{leaf}></{tag}>")
            };
            let ours = frame(&super::super::entity::new::jackson_pretty(&old));
            let theirs = frame(&new.to_string());
            let out = entity(theirs.as_bytes(), Some(ours.as_bytes()), false).unwrap();
            assert_eq!(
                out,
                frame(&super::super::entity::new::jackson_pretty(&new)).as_bytes()
            );
        }
        let ours = br#"<StateDefinition name="Acme.Asset"><content><![CDATA[{"first":1,"second":2}]]></content></StateDefinition>"#;
        let theirs = br#"<StateDefinition name="Acme.Asset"><content><![CDATA[{"second":2,"first":1}]]></content></StateDefinition>"#;
        assert_eq!(entity(theirs, Some(ours), false).unwrap(), theirs);
    }

    #[test]
    fn stored_values_preserve_repository_stamps_and_order_around_a_changed_value() {
        let ours = br#"<Thing name="Acme.Asset"><ThingProperties><Speed><Value>1</Value><Timestamp>old</Timestamp><Quality>UNKNOWN</Quality></Speed><Label><Value><![CDATA[ kept ]]></Value></Label></ThingProperties></Thing>"#;
        let theirs = br#"<Thing name="Acme.Asset"><ThingProperties><Label><Value><![CDATA[kept]]></Value></Label><Speed><Value>2</Value><Timestamp>new</Timestamp><Quality>GOOD</Quality></Speed></ThingProperties></Thing>"#;
        assert_eq!(
            entity(theirs, Some(ours), false).unwrap(),
            std::str::from_utf8(ours)
                .unwrap()
                .replace("<Value>1</Value>", "<Value>2</Value>")
                .as_bytes()
        );
    }

    #[test]
    fn source_control_tags_keep_lexical_values_and_only_drop_exact_live_metadata() {
        let raw = br#"<Thing z='&amp;' name="Acme.Manager" lastModifiedDate="1" a="&#10;"><Owner>Acme.Exporter</Owner><ConfigurationChanges/><effectiveShape/><EffectiveImplementedShapes/><effectiveAlertConfiguration/><effectiveLocalPropertyBindings/><effectiveRemoteEventBindings/><effectiveRemotePropertyBindings/><effectiveRemoteServiceBindings/><effectiveThingPackage/><ThingShape><Owner>Acme.Source</Owner></ThingShape><UID>1</UID><value><![CDATA[ x & y ]]></value></Thing>"#;
        let result = entity(raw, None, false).unwrap();
        let text = std::str::from_utf8(&result).unwrap();
        assert!(text.starts_with(
            "<Thing\n         a=\"&#10;\"\n         name=\"Acme.Manager\"\n         z='&amp;'>"
        ));
        assert!(!text.contains("lastModifiedDate") && !text.contains("Acme.Exporter"));
        assert!(text.contains("<effectiveThingPackage></effectiveThingPackage>"));
        assert!(text.contains("<Owner>Acme.Source</Owner>"));
        assert!(text.contains("<UID>1</UID>"));
        assert!(text.contains("<![CDATA[\n                x & y\n                ]]>"));
        assert_eq!(entity(&result, Some(&result), false).unwrap(), result);
    }

    #[test]
    fn both_script_payload_modes_roundtrip_without_changing_internal_indentation() {
        let raw = b"<Thing name=\"Acme.Manager\"><code><![CDATA[if (true) {\n    result = 1;\n}]]></code></Thing>";
        for indent in [false, true] {
            let laid_out = entity(raw, None, indent).unwrap();
            assert_eq!(entity(&laid_out, None, indent).unwrap(), laid_out);
            assert_eq!(
                entity(&laid_out, Some(&laid_out), indent).unwrap(),
                laid_out
            );
            assert_eq!(entity(raw, Some(&laid_out), indent).unwrap(), laid_out);
            let tokens = scan::tokenize(&laid_out).unwrap();
            let cdata = tokens.iter().find(|t| t.kind == Kind::Cdata).unwrap();
            let flattened = format!(
                "<Thing name=\"Acme.Manager\"><code><![CDATA[{}]]></code></Thing>",
                std::str::from_utf8(cdata.inner.of(&laid_out))
                    .unwrap()
                    .trim()
            );
            assert_eq!(
                entity(flattened.as_bytes(), Some(&laid_out), indent).unwrap(),
                laid_out
            );
            assert!(std::str::from_utf8(&laid_out).unwrap().contains(if indent {
                "                result = 1;"
            } else {
                "\n    result = 1;"
            }));
        }
    }

    #[test]
    fn a_new_entity_keeps_a_split_cdata_payload_as_the_server_wrote_it() {
        // Around two CDATA sections, whitespace is part of the value; framing them would change
        // the script and leave the entity out of step with the server for good.
        let raw = b"<Thing name=\"Acme.Manager\"><code><![CDATA[result = \"x]]]]><![CDATA[>\";]]></code></Thing>";
        let result = entity(raw, None, false).unwrap();
        assert!(
            std::str::from_utf8(&result)
                .unwrap()
                .contains("<code><![CDATA[result = \"x]]]]><![CDATA[>\";]]></code>"),
            "{}",
            String::from_utf8_lossy(&result)
        );
    }

    #[test]
    fn cdata_splits_mixed_text_comments_and_nested_metadata_are_retained() {
        let raw = b"<Thing name=\"Acme.Manager\"><code><![CDATA[result = \"x]]]]><![CDATA[>\";]]></code><mixed> before <![CDATA[ untouched ]]><b/> after </mixed><!--kept--></Thing>";
        let result = entity(raw, None, false).unwrap();
        let text = std::str::from_utf8(&result).unwrap();
        assert!(text.contains("]]><![CDATA[>"));
        assert!(text.contains(" before <![CDATA[ untouched ]]><b></b> after "));
        assert!(text.contains("<!--kept-->"));
        assert!(entity(b"<Thing><a></b></Thing>", None, false).is_err());
    }

    #[test]
    fn empty_cdata_and_crlf_framing_are_idempotent_and_metadata_beside_comments_is_removed() {
        let src = b"<Thing\r\n         name=\"Acme.Manager\">\r\n            <value>\r\n                <![CDATA[\r\n                ]]>\r\n            </value>\r\n        </Thing>";
        assert_eq!(entity(src, Some(src), false).unwrap(), src);
        let flat = b"<Thing name=\"Acme.Manager\"><!--kept--><Owner>Acme.Exporter</Owner><value><![CDATA[]]></value></Thing>";
        let output = entity(flat, Some(src), false).unwrap();
        assert!(std::str::from_utf8(&output)
            .unwrap()
            .contains("<!--kept-->"));
        assert!(!std::str::from_utf8(&output)
            .unwrap()
            .contains("Acme.Exporter"));
    }
}
