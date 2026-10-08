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

use super::entity_key::{EntityKey, ServiceTarget};
use super::server::{Client, ServerError};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

/// Every entity collection the search knows, with the singular type name it takes. Each was
/// checked on a live server: the singular name narrows the search, and only these do.
pub const TYPES: &[(&str, &str)] = &[
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
pub const MAX_LIMIT: usize = 10_000;
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
pub fn entity_type(given: &str) -> Result<&'static str, SearchError> {
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

#[derive(Debug, Default)]
pub struct Query<'a> {
    /// Text in a name or description; `*` makes it a pattern. Absent or blank: every entity.
    pub text: Option<&'a str>,
    /// Type or collection names, each checked by [`entity_type`].
    pub types: &'a [String],
    pub project: Option<&'a str>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Found {
    /// `Collection/Name`, as `export` and `entity get` take it.
    pub entity: String,
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
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

    let mut parameters = json!({
        "maxItems": query.limit + 1,
        "sortBy": "name",
        "isAscending": true,
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
        });
    }
    let more = entities.len() > query.limit;
    entities.truncate(query.limit);

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
            let limit = parameters["maxItems"].as_u64().unwrap() as usize;
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
            project: None,
            limit: DEFAULT_LIMIT,
        }
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
