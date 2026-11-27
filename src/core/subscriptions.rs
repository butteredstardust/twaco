//! Subscription triggers as held in legacy and version-2 entity exports.

use super::scan::{self, Token};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Subscription {
    pub name: String,
    pub stateful: bool,
    pub distributed: bool,
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Event {
    #[serde(skip_serializing_if = "str::is_empty")]
    pub alias: String,
    pub event: String,
    pub source: String,
    pub source_property: String,
    pub source_type: String,
}

impl Event {
    pub(crate) fn member(&self) -> &str {
        if self.source_property.is_empty() {
            &self.event
        } else {
            &self.source_property
        }
    }

    pub fn address(&self) -> String {
        let source = if self.source.is_empty() {
            "me"
        } else {
            &self.source
        };
        let property = if self.source_property.is_empty() {
            String::new()
        } else {
            format!(":{}", self.source_property)
        };
        let alias = if self.alias.is_empty() {
            String::new()
        } else {
            format!("={}", self.alias)
        };
        format!("{source}:{}{property}{alias}", self.event)
    }
}

pub(crate) fn tags(tokens: &[Token], src: &[u8], entity: usize) -> Vec<usize> {
    let hosts = std::iter::once(entity).chain(scan::child_tags(tokens, src, "ThingShape", entity));
    hosts
        .flat_map(|host| scan::child_tags(tokens, src, "Subscriptions", host))
        .flat_map(|list| scan::child_tags(tokens, src, "Subscription", list))
        .collect()
}

/// The tags that contain source fields. Legacy subscriptions carry them on the outer tag.
pub(crate) fn event_tags(tokens: &[Token], src: &[u8], subscription: usize) -> Vec<usize> {
    let mut events: Vec<_> = scan::child_tags(tokens, src, "Events", subscription)
        .into_iter()
        .flat_map(|group| scan::child_tags(tokens, src, "Event", group))
        .collect();
    // Older repository files can have direct Event nodes as well.
    events.extend(scan::child_tags(tokens, src, "Event", subscription));
    if events.is_empty() && attr(src, &tokens[subscription], "version") != "2" {
        events.push(subscription);
    }
    events
}

pub(crate) fn read(tokens: &[Token], src: &[u8], subscription: usize) -> Subscription {
    let tag = &tokens[subscription];
    let events = event_tags(tokens, src, subscription)
        .into_iter()
        .filter_map(|index| {
            let tag = &tokens[index];
            let event = attr(src, tag, "eventName");
            (!event.is_empty()).then(|| Event {
                alias: attr(src, tag, "alias"),
                event,
                source: attr(src, tag, "source"),
                source_type: attr(src, tag, "sourceType"),
                source_property: attr(src, tag, "sourceProperty"),
            })
        })
        .collect();
    Subscription {
        name: attr(src, tag, "name"),
        stateful: matches!(attr(src, tag, "aspect.isStateful").as_str(), "true" | "1"),
        distributed: matches!(
            attr(src, tag, "aspect.isDistributed").as_str(),
            "true" | "1"
        ),
        events,
    }
}

fn attr(src: &[u8], tag: &Token, name: &str) -> String {
    scan::attribute(src, tag, name)
        .ok()
        .flatten()
        .map(|value| scan::decode_entities(&String::from_utf8_lossy(value.of(src))))
        .unwrap_or_default()
}
