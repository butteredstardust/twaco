//! `twaco search`: the server's own entity search, the one behind Composer's Spotlight box.
//!
//! Read-only: `Resources/SearchFunctions.SpotlightSearchV2` changes nothing. What it gets wrong
//! silently, this module refuses or says out loud (verified live on 10.0 and 10.1, see the
//! `SpotlightSearchV2` sections of the platform quirks):
//!
//! - text matches a whole name or description, so `Card` finds only an entity named exactly
//!   `Card`. Text without a `*` is searched as `*text*`, which is what a person typing it means,
//!   and finds it anywhere in a name or a description;
//! - a type must be the singular entity type. A plural collection name finds nothing, and a name
//!   the server does not know is dropped, so the search covers every entity instead. Types are
//!   translated from either spelling here and anything else is refused before it is sent; a
//!   reply holding a type that was not asked for is refused rather than shown;
//! - the server stops at 500 rows unless told otherwise, without saying so. One row more than the
//!   limit is asked for, so a cut list says that it is one;
//! - a project that does not exist matches nothing, like one with no entities. When a project
//!   search finds nothing, the server is asked whether the project exists.
//!
//! A search by modification time asks for the newest first and cuts the list at the date here,
//! instead of sending the service's own date window: every row carries `lastModifiedDate`, so a
//! reply out of order is refused, and no filter the server might drop silently is sent.

use super::codes::{Coded, ErrorCode};
use super::entity::key::{EntityKey, ServiceTarget};
use super::server::{Client, ServerError};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

/// Every entity collection the search knows, with the singular type name it takes. Each was
/// checked on a live server: the singular name narrows the search, and only these do.
pub(crate) const TYPES: &[(&str, &str)] = &[
    ("ApplicationKeys", "ApplicationKey"),
    ("Authenticators", "Authenticator"),
    ("Dashboards", "Dashboard"),
    ("DataShapes", "DataShape"),
    ("DirectoryServices", "DirectoryService"),
    ("ExtensionPackages", "ExtensionPackage"),
    ("Groups", "Group"),
    ("LocalizationTables", "LocalizationTable"),
    ("Logs", "Log"),
    ("Mashups", "Mashup"),
    ("MediaEntities", "MediaEntity"),
    ("Menus", "Menu"),
    ("Networks", "Network"),
    ("Organizations", "Organization"),
    ("PersistenceProviders", "PersistenceProvider"),
    ("Projects", "Project"),
    ("Resources", "Resource"),
    ("StateDefinitions", "StateDefinition"),
    ("StyleDefinitions", "StyleDefinition"),
    ("StyleThemes", "StyleTheme"),
    ("Subsystems", "Subsystem"),
    ("ThingShapes", "ThingShape"),
    ("ThingTemplates", "ThingTemplate"),
    ("Things", "Thing"),
    ("Users", "User"),
];

/// The most a search may ask for. Far more than a person reads; an agent narrows instead.
pub(crate) const MAX_LIMIT: usize = 10_000;
pub const DEFAULT_LIMIT: usize = 100;

/// What the search asks of the server, as a trait so it is tested offline.
pub trait Remote {
    fn spotlight(&self, parameters: &Value) -> Result<Value, ServerError>;
    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError>;
}

impl Remote for Client {
    fn spotlight(&self, parameters: &Value) -> Result<Value, ServerError> {
        let reply = self.call_service(
            &ServiceTarget::platform("Resources", "SearchFunctions"),
            "SpotlightSearchV2",
            parameters,
            Duration::from_secs(120),
        )?;
        Ok(reply.unwrap_or(Value::Null))
    }

    fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
        self.entity_exists(key)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Remote(ServerError),
    /// The reply is not what the search returns, or it ignored the type filter.
    #[error("the search's reply: {0}")]
    Reply(String),
}

/// The singular type the search takes for a type or collection name, in any case.
pub(crate) fn entity_type(given: &str) -> Result<&'static str, SearchError> {
    let given = given.trim();
    TYPES
        .iter()
        .find(|(collection, singular)| {
            collection.eq_ignore_ascii_case(given) || singular.eq_ignore_ascii_case(given)
        })
        .map(|(_, singular)| *singular)
        .ok_or_else(|| {
            SearchError::Invalid(format!(
                "{given:?} is not an entity type the search knows; one of: {}",
                TYPES
                    .iter()
                    .map(|(_, singular)| *singular)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

/// The order of the results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    /// By name, A to Z.
    Name,
    /// Newest change first.
    Modified,
}

#[derive(Debug, Default)]
pub struct Query<'a> {
    /// Text in a name or description; `*` makes it a pattern. Absent or blank: every entity.
    pub text: Option<&'a str>,
    /// Type or collection names, each checked by `entity_type`.
    pub types: &'a [String],
    pub project: Option<&'a str>,
    /// Only entities changed at or after this time.
    pub modified_since: Option<jiff::Timestamp>,
    /// Absent: newest first with `modified_since`, by name without.
    pub sort: Option<Sort>,
    pub limit: usize,
}

/// When `--modified-since` starts: `1d`, `12h` or `30m` before `now`, a date (midnight UTC that
/// day), or a full RFC 3339 timestamp. It must be in the past.
pub fn parse_since(text: &str, now: jiff::Timestamp) -> Result<jiff::Timestamp, SearchError> {
    let text = text.trim();
    let invalid = || {
        SearchError::Invalid(format!(
            "modified since {text:?} is not a duration (1d, 12h, 30m), a date (2026-10-24) or a \
             timestamp (2026-10-24T08:00:00Z)"
        ))
    };
    let when = if let Some(ms) = super::duration::positive_ms(text) {
        i64::try_from(ms)
            .ok()
            .map(jiff::SignedDuration::from_millis)
            .and_then(|span| now.checked_sub(span).ok())
            .ok_or_else(invalid)?
    } else if let Ok(date) = text.parse::<jiff::civil::Date>() {
        date.to_zoned(jiff::tz::TimeZone::UTC)
            .map_err(|_| invalid())?
            .timestamp()
    } else {
        text.parse::<jiff::Timestamp>().map_err(|_| invalid())?
    };
    if when > now {
        return Err(SearchError::Invalid(format!(
            "modified since {text:?} is in the future ({when})"
        )));
    }
    // Whole seconds: a window nobody reads to the nanosecond.
    Ok(jiff::Timestamp::from_second(when.as_second()).unwrap_or(when))
}

/// A row's `lastModifiedDate`: the epoch milliseconds the REST API returns, or a timestamp.
fn modified_of(row: &Value) -> Option<jiff::Timestamp> {
    match row.get("lastModifiedDate")? {
        Value::Number(ms) => jiff::Timestamp::from_millisecond(ms.as_i64()?).ok(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Found {
    /// `Collection/Name`, as `export` and `entity get` take it.
    pub entity: String,
    #[serde(rename = "type")]
    pub(crate) type_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// When the server last saw it change, as RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// What was searched for, as sent: `*Card*` for `Card`.
    pub expression: Option<String>,
    pub entities: Vec<Found>,
    /// More entities match than the limit let through.
    pub more: bool,
    /// Something the list alone would not say, such as a project that does not exist.
    pub note: Option<String>,
}

/// Search the server.
pub fn search(remote: &dyn Remote, query: &Query) -> Result<Outcome, SearchError> {
    if query.limit == 0 || query.limit > MAX_LIMIT {
        return Err(SearchError::Invalid(format!(
            "the limit must be between 1 and {MAX_LIMIT}"
        )));
    }
    let mut types = Vec::new();
    for given in query.types.iter().flat_map(|types| types.split(',')) {
        // A filter that was given but says nothing must not quietly become no filter.
        if given.trim().is_empty() {
            return Err(SearchError::Invalid(
                "a type is empty; name one, or leave the types out to search every type"
                    .to_string(),
            ));
        }
        let singular = entity_type(given)?;
        if !types.contains(&singular) {
            types.push(singular);
        }
    }
    let expression = query
        .text
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| {
            if text.contains('*') {
                text.to_string()
            } else {
                format!("*{text}*")
            }
        });
    let project = query.project.map(str::trim);
    if project == Some("") {
        return Err(SearchError::Invalid(
            "the project is empty; name one, or leave it out to search every project".to_string(),
        ));
    }

    // Newest first whenever time matters, so the cut at `modified_since` loses nothing older
    // rows would have held.
    let sort = query.sort.unwrap_or(if query.modified_since.is_some() {
        Sort::Modified
    } else {
        Sort::Name
    });
    let by_time = sort == Sort::Modified || query.modified_since.is_some();
    let mut parameters = json!({
        "maxItems": query.limit + 1,
        "sortBy": if by_time { "lastModifiedDate" } else { "name" },
        "isAscending": !by_time,
    });
    if let Some(expression) = &expression {
        parameters["searchExpression"] = json!(expression);
    }
    if !types.is_empty() {
        parameters["types"] = json!({ "items": types });
    }
    if let Some(project) = project {
        parameters["projectName"] = json!(project);
    }
    let reply = remote.spotlight(&parameters).map_err(SearchError::Remote)?;
    let rows = reply
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| SearchError::Reply("it has no rows".to_string()))?;

    let text = |row: &Value, field: &str| {
        row.get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut entities = Vec::new();
    let mut previous: Option<jiff::Timestamp> = None;
    let mut older = false;
    for row in rows {
        let name = text(row, "name");
        let type_name = text(row, "type");
        if name.is_empty() || type_name.is_empty() {
            return Err(SearchError::Reply(format!(
                "a row has no name or type: {row}"
            )));
        }
        // An unknown type widens the search to everything; never show what was not asked for.
        if !types.is_empty() && !types.contains(&type_name.as_str()) {
            return Err(SearchError::Reply(format!(
                "it holds {type_name} {name}, which is not one of the types asked for ({}); \
                 the server ignored the type filter",
                types.join(", ")
            )));
        }
        let modified = modified_of(row);
        if by_time {
            let Some(when) = modified else {
                return Err(SearchError::Reply(format!(
                    "{type_name} {name} has no lastModifiedDate, so the search cannot go by time"
                )));
            };
            if previous.is_some_and(|newer| when > newer) {
                return Err(SearchError::Reply(format!(
                    "it is not newest first ({type_name} {name} comes after an older change); \
                     the server ignored the order asked for"
                )));
            }
            previous = Some(when);
            if query.modified_since.is_some_and(|since| when < since) {
                // Every row after this one is older still, and is not looked at.
                older = true;
                break;
            }
        }
        let collection = match text(row, "parentType") {
            parent if !parent.is_empty() => parent,
            _ => TYPES
                .iter()
                .find(|(_, singular)| *singular == type_name)
                .map(|(collection, _)| collection.to_string())
                .unwrap_or_else(|| type_name.clone()),
        };
        entities.push(Found {
            entity: format!("{collection}/{name}"),
            type_name,
            project: text(row, "projectName"),
            description: text(row, "description"),
            modified: modified.map(|when| when.to_string()),
        });
    }
    let more = !older && entities.len() > query.limit;
    entities.truncate(query.limit);
    if sort == Sort::Name && by_time {
        let name = |found: &Found| {
            let name = found
                .entity
                .split_once('/')
                .map_or(found.entity.as_str(), |(_, name)| name);
            name.to_lowercase()
        };
        entities.sort_by_key(name);
    }

    let mut note = None;
    if let (Some(project), true) = (project, entities.is_empty()) {
        let key = EntityKey::new("Projects", project)
            .map_err(|why| SearchError::Invalid(format!("project {why}")))?;
        if !remote.exists(&key).map_err(SearchError::Remote)? {
            note = Some(format!("the server has no project named {project}"));
        }
    }
    Ok(Outcome {
        expression,
        entities,
        more,
        note,
    })
}

impl Coded for SearchError {
    fn code(&self) -> ErrorCode {
        match self {
            Self::Invalid(_) => ErrorCode::InvalidArguments,
            Self::Remote(error) => error.code(),
            Self::Reply(_) => ErrorCode::InvalidData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        rows: Vec<Value>,
        projects: Vec<&'static str>,
        sent: RefCell<Vec<Value>>,
    }

    impl Remote for Fake {
        fn spotlight(&self, parameters: &Value) -> Result<Value, ServerError> {
            self.sent.borrow_mut().push(parameters.clone());
            let limit = usize::try_from(parameters["maxItems"].as_u64().unwrap()).unwrap();
            Ok(json!({ "rows": self.rows.iter().take(limit).collect::<Vec<_>>() }))
        }

        fn exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
            Ok(key.collection() == "Projects" && self.projects.contains(&key.name()))
        }
    }

    fn row(name: &str, type_name: &str, collection: &str) -> Value {
        json!({
            "name": name,
            "type": type_name,
            "parentType": collection,
            "projectName": "Acme.Orders",
            "description": "",
        })
    }

    fn query<'a>(text: Option<&'a str>, types: &'a [String]) -> Query<'a> {
        Query {
            text,
            types,
            limit: DEFAULT_LIMIT,
            ..Query::default()
        }
    }

    fn at(text: &str) -> jiff::Timestamp {
        text.parse().unwrap()
    }

    fn dated(name: &str, when: &str) -> Value {
        let mut row = row(name, "Thing", "Things");
        row["lastModifiedDate"] = json!(at(when).as_millisecond());
        row
    }

    #[test]
    fn modified_since_takes_a_duration_a_date_or_a_timestamp_in_the_past() {
        let now = at("2026-10-25T12:00:00Z");
        for (given, wanted) in [
            ("1d", "2026-10-24T12:00:00Z"),
            ("90m", "2026-10-25T10:30:00Z"),
            ("2026-10-20", "2026-10-20T00:00:00Z"),
            ("2026-10-25T08:15:00Z", "2026-10-25T08:15:00Z"),
        ] {
            assert_eq!(parse_since(given, now).unwrap(), at(wanted), "{given}");
        }
        for bad in ["yesterday", "", "2026-13-01", "-1d"] {
            let error = parse_since(bad, now).unwrap_err();
            assert!(
                error.to_string().contains("is not a duration"),
                "{bad}: {error}"
            );
        }
        let error = parse_since("2026-10-26", now).unwrap_err();
        assert!(error.to_string().contains("in the future"), "{error}");
    }

    #[test]
    fn a_search_by_time_asks_newest_first_and_cuts_at_the_date() {
        let fake = Fake {
            rows: vec![
                dated("Acme.C", "2026-10-25T09:00:00Z"),
                dated("Acme.A", "2026-10-24T18:00:00Z"),
                dated("Acme.B", "2026-10-20T00:00:00Z"),
            ],
            ..Fake::default()
        };
        let mut wanted = query(None, &[]);
        wanted.modified_since = Some(at("2026-10-24T00:00:00Z"));
        let outcome = search(&fake, &wanted).unwrap();
        assert_eq!(fake.sent.borrow()[0]["sortBy"], "lastModifiedDate");
        assert_eq!(fake.sent.borrow()[0]["isAscending"], false);
        assert!(
            fake.sent.borrow()[0].get("startDate").is_none(),
            "the window is cut here, never sent"
        );
        assert_eq!(
            outcome
                .entities
                .iter()
                .map(|found| found.entity.as_str())
                .collect::<Vec<_>>(),
            ["Things/Acme.C", "Things/Acme.A"]
        );
        assert_eq!(
            outcome.entities[1].modified.as_deref(),
            Some("2026-10-24T18:00:00Z")
        );
        assert!(!outcome.more, "an older row ends the window");

        // Asked for by name, the window is still cut newest first, then put in name order.
        wanted.sort = Some(Sort::Name);
        let outcome = search(&fake, &wanted).unwrap();
        assert_eq!(fake.sent.borrow()[1]["sortBy"], "lastModifiedDate");
        assert_eq!(
            outcome
                .entities
                .iter()
                .map(|found| found.entity.as_str())
                .collect::<Vec<_>>(),
            ["Things/Acme.A", "Things/Acme.C"]
        );

        // Every row within the window and one more than the limit: more do match.
        wanted.modified_since = Some(at("2026-10-01T00:00:00Z"));
        wanted.limit = 2;
        assert!(search(&fake, &wanted).unwrap().more);
        wanted.limit = 3;
        assert!(!search(&fake, &wanted).unwrap().more);
    }

    #[test]
    fn a_reply_out_of_order_or_without_dates_is_refused_when_the_search_goes_by_time() {
        let mut wanted = query(None, &[]);
        wanted.sort = Some(Sort::Modified);
        let unordered = Fake {
            rows: vec![
                dated("Acme.A", "2026-10-20T00:00:00Z"),
                dated("Acme.B", "2026-10-25T00:00:00Z"),
            ],
            ..Fake::default()
        };
        let error = search(&unordered, &wanted).unwrap_err();
        assert!(error.to_string().contains("not newest first"), "{error}");
        let undated = Fake {
            rows: vec![row("Acme.A", "Thing", "Things")],
            ..Fake::default()
        };
        let error = search(&undated, &wanted).unwrap_err();
        assert!(error.to_string().contains("no lastModifiedDate"), "{error}");
        // Past the cut nothing is read, so an undated or misplaced row there refuses nothing.
        let tail = Fake {
            rows: vec![
                dated("Acme.New", "2026-10-25T00:00:00Z"),
                dated("Acme.Old", "2026-10-01T00:00:00Z"),
                row("Acme.Undated", "Thing", "Things"),
                dated("Acme.Newer", "2026-10-26T00:00:00Z"),
            ],
            ..Fake::default()
        };
        let mut since = query(None, &[]);
        since.modified_since = Some(at("2026-10-20T00:00:00Z"));
        let outcome = search(&tail, &since).unwrap();
        assert_eq!(outcome.entities.len(), 1);
        assert!(!outcome.more);
        // A search by name never needs the date.
        assert_eq!(
            search(&undated, &query(None, &[])).unwrap().entities.len(),
            1
        );
        let iso = Fake {
            rows: vec![json!({ "name": "Acme.A", "type": "Thing",
                "lastModifiedDate": "2026-10-25T07:00:00Z" })],
            ..Fake::default()
        };
        assert_eq!(
            search(&iso, &wanted).unwrap().entities[0]
                .modified
                .as_deref(),
            Some("2026-10-25T07:00:00Z")
        );
    }

    #[test]
    fn text_is_a_part_of_a_name_unless_it_is_already_a_pattern() {
        let fake = Fake::default();
        for (given, sent) in [
            (Some("Card"), Some("*Card*")),
            (Some("  Card "), Some("*Card*")),
            (Some("*_DS"), Some("*_DS")),
            (Some("Acme.*.Order"), Some("Acme.*.Order")),
            (Some("  "), None),
            (None, None),
        ] {
            let outcome = search(&fake, &query(given, &[])).unwrap();
            assert_eq!(outcome.expression.as_deref(), sent, "{given:?}");
            let parameters = fake.sent.borrow().last().unwrap().clone();
            assert_eq!(
                parameters.get("searchExpression").and_then(Value::as_str),
                sent,
                "{given:?}"
            );
        }
    }

    #[test]
    fn a_type_is_sent_singular_from_either_spelling_and_an_unknown_one_is_never_sent() {
        let fake = Fake::default();
        let types = vec![
            "thingtemplates".to_string(),
            "Mashup, DataShapes".to_string(),
        ];
        search(&fake, &query(None, &types)).unwrap();
        assert_eq!(
            fake.sent.borrow()[0]["types"],
            json!({ "items": ["ThingTemplate", "Mashup", "DataShape"] })
        );
        // The server would search every entity for a name it does not know.
        for bad in ["ModelTags", "Widget", "Logger"] {
            let error = search(&fake, &query(None, &[bad.to_string()])).unwrap_err();
            assert!(error.to_string().contains("not an entity type"), "{error}");
        }
        // Given but blank is refused too: dropped, it would be no filter at all.
        for blank in ["", "  ", ",", "Thing,"] {
            let error = search(&fake, &query(None, &[blank.to_string()])).unwrap_err();
            assert!(
                error.to_string().contains("a type is empty"),
                "{blank:?}: {error}"
            );
        }
        let mut wanted = query(None, &[]);
        wanted.project = Some("   ");
        let error = search(&fake, &wanted).unwrap_err();
        assert!(
            error.to_string().contains("the project is empty"),
            "{error}"
        );
        assert_eq!(
            fake.sent.borrow().len(),
            1,
            "nothing was sent for a bad filter"
        );
    }

    #[test]
    fn a_reply_holding_a_type_not_asked_for_is_refused_rather_than_shown() {
        let fake = Fake {
            rows: vec![
                row("A", "Thing", "Things"),
                row("D", "DataShape", "DataShapes"),
            ],
            ..Fake::default()
        };
        let error = search(&fake, &query(None, &["Thing".to_string()])).unwrap_err();
        assert!(
            error.to_string().contains("ignored the type filter"),
            "{error}"
        );
    }

    #[test]
    fn results_are_collection_and_name_and_a_cut_list_says_so() {
        let fake = Fake {
            rows: (1..=4)
                .map(|n| row(&format!("Acme.T{n}"), "Thing", "Things"))
                .chain([json!({ "name": "Acme.S", "type": "ThingShape" })])
                .collect(),
            ..Fake::default()
        };
        let mut wanted = query(None, &[]);
        wanted.limit = 3;
        let outcome = search(&fake, &wanted).unwrap();
        assert_eq!(
            fake.sent.borrow()[0]["maxItems"],
            4,
            "one more than the limit"
        );
        assert!(outcome.more);
        assert_eq!(
            outcome
                .entities
                .iter()
                .map(|found| found.entity.as_str())
                .collect::<Vec<_>>(),
            ["Things/Acme.T1", "Things/Acme.T2", "Things/Acme.T3"]
        );
        wanted.limit = 10;
        let outcome = search(&fake, &wanted).unwrap();
        assert!(!outcome.more);
        // No parentType: the collection comes from the type.
        assert_eq!(outcome.entities[4].entity, "ThingShapes/Acme.S");
        for limit in [0, MAX_LIMIT + 1] {
            wanted.limit = limit;
            assert!(search(&fake, &wanted).is_err(), "{limit}");
        }
    }

    #[test]
    fn a_project_that_does_not_exist_is_told_apart_from_an_empty_one() {
        let fake = Fake {
            projects: vec!["Acme.Empty"],
            ..Fake::default()
        };
        let mut wanted = query(None, &[]);
        wanted.project = Some("Acme.Empty");
        assert_eq!(search(&fake, &wanted).unwrap().note, None);
        assert_eq!(fake.sent.borrow()[0]["projectName"], "Acme.Empty");
        wanted.project = Some("Acme.Typo");
        assert_eq!(
            search(&fake, &wanted).unwrap().note.as_deref(),
            Some("the server has no project named Acme.Typo")
        );
    }

    #[test]
    fn a_reply_without_rows_is_an_error_not_an_empty_list() {
        struct Odd;
        impl Remote for Odd {
            fn spotlight(&self, _: &Value) -> Result<Value, ServerError> {
                Ok(Value::Null)
            }
            fn exists(&self, _: &EntityKey) -> Result<bool, ServerError> {
                Ok(true)
            }
        }
        assert!(matches!(
            search(&Odd, &query(Some("x"), &[])),
            Err(SearchError::Reply(_))
        ));
    }
}
