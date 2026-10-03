//! Copy a renamed entity's permissions to its new name.
//!
//! A rename creates new entities on the server. What an entity's XML does not carry includes
//! permissions set at run time, and the platform has services for exactly this on every entity:
//! `Get/Set` `RunTime`, `DesignTime` and `Visibility` `PermissionsAsJSON`. A permission names
//! principals (a group, an organization, `Org:Unit`), and after a prefix rename those principals
//! have new names too, so the copied JSON is mapped through every rename in the ledger before it is
//! compared and written. Nothing is written without `apply`, and every write is read back.
//!
//! Verified on a live server: the `Set` services take the **string form** of the whole JSON that
//! the matching `Get` returned (`{"permissions": [...]}` for run time, `{"Read": [...], ...}` for
//! design time, `{"Visibility": [...]}`); a visibility principal is an organization or an
//! organizational unit, never a group.

use super::config::Solution;
use super::ledger::{self, Ledger};
use super::refs;
use super::server::{Client, ServerError};
use serde::Serialize;
use serde_json::{json, Value};
use std::fmt;
use std::time::Duration;

/// The three permission sets of an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Kind {
    RunTime,
    DesignTime,
    Visibility,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::RunTime, Kind::DesignTime, Kind::Visibility];

    pub fn label(self) -> &'static str {
        match self {
            Kind::RunTime => "run-time",
            Kind::DesignTime => "design-time",
            Kind::Visibility => "visibility",
        }
    }

    fn get_service(self) -> &'static str {
        match self {
            Kind::RunTime => "GetRunTimePermissionsAsJSON",
            Kind::DesignTime => "GetDesignTimePermissionsAsJSON",
            Kind::Visibility => "GetVisibilityPermissionsAsJSON",
        }
    }

    fn set_service(self) -> &'static str {
        match self {
            Kind::RunTime => "SetRunTimePermissionsAsJSON",
            Kind::DesignTime => "SetDesignTimePermissionsAsJSON",
            Kind::Visibility => "SetVisibilityPermissionsAsJSON",
        }
    }
}

/// What carrying needs from a server.
pub trait Remote {
    fn exists(&self, collection: &str, name: &str) -> Result<bool, ServerError>;
    fn get(&self, collection: &str, name: &str, kind: Kind) -> Result<Value, ServerError>;
    fn set(&self, collection: &str, name: &str, kind: Kind, value: &Value) -> Result<(), ServerError>;
    /// How many differences the platform reports between two entities.
    fn differences(&self, collection: &str, name: &str, other: &str) -> Result<usize, ServerError>;
}

impl Remote for Client {
    fn exists(&self, collection: &str, name: &str) -> Result<bool, ServerError> {
        self.entity_exists(collection, name)
    }

    fn get(&self, collection: &str, name: &str, kind: Kind) -> Result<Value, ServerError> {
        let target = format!("{collection}/{name}");
        self.call_service(&target, kind.get_service(), &json!({}), Duration::from_secs(60))?
            .ok_or_else(|| ServerError::InvalidResponse {
                url: format!("{target}/Services/{}", kind.get_service()),
                why: "the service returned nothing".to_string(),
            })
    }

    fn set(&self, collection: &str, name: &str, kind: Kind, value: &Value) -> Result<(), ServerError> {
        let target = format!("{collection}/{name}");
        // The JSON parameter is accepted as the text of the object, for all three sets.
        let parameters = json!({ "permissions": value.to_string() });
        self.call_service(&target, kind.set_service(), &parameters, Duration::from_secs(60))?;
        Ok(())
    }

    fn differences(&self, collection: &str, name: &str, other: &str) -> Result<usize, ServerError> {
        let target = format!("{collection}/{name}");
        let value = self
            .call_service(&target, "GetDifferencesAsJSON", &json!({ "otherEntity": other }), Duration::from_secs(120))?
            .unwrap_or_else(|| json!({ "rows": [] }));
        Ok(value.get("rows").and_then(Value::as_array).map_or(0, Vec::len))
    }
}

/// One old entity and the one that replaced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub collection: String,
    pub old: String,
    pub new: String,
}

#[derive(Debug)]
pub enum CarryError {
    Ledger { why: String },
    Arguments { why: String },
}

impl fmt::Display for CarryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CarryError::Ledger { why } => write!(f, "{why}; fix or remove .twaco/renames.json"),
            CarryError::Arguments { why } => f.write_str(why),
        }
    }
}

impl std::error::Error for CarryError {}

/// Every old name that a rename in the ledger replaced, to map a permission's principals.
#[derive(Debug, Default, Clone)]
pub struct Mapping {
    entities: Vec<(String, String)>,
    prefixes: Vec<(String, String)>,
}

impl Mapping {
    pub fn from_ledger(ledger: &Ledger) -> Mapping {
        let mut mapping = Mapping::default();
        for record in ledger.0.iter().filter(|record| record.kind.replaces_entities()) {
            if record.kind == ledger::Kind::Prefix {
                mapping.prefixes.push((record.old.clone(), record.new.clone()));
            }
            for entity in &record.entities {
                mapping.entities.push((entity.old.clone(), entity.new.clone()));
            }
        }
        mapping
    }

    /// A string with every renamed name in it replaced; compound names (`Org:Group`) and URLs work
    /// because the matching is by whole token.
    pub fn map_text(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (old, new) in &self.entities {
            let hits = refs::find(&out, old, refs::Mode::Entity);
            out = refs::replace(&out, &hits, new);
        }
        for (old, new) in &self.prefixes {
            let hits = refs::find(&out, old, refs::Mode::Prefix);
            out = refs::replace(&out, &hits, new);
        }
        out
    }

    /// The JSON with every string value mapped; keys and everything else are left as they are.
    pub fn map_value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.map_text(text)),
            Value::Array(items) => Value::Array(items.iter().map(|item| self.map_value(item)).collect()),
            Value::Object(map) => Value::Object(map.iter().map(|(key, item)| (key.clone(), self.map_value(item))).collect()),
            other => other.clone(),
        }
    }
}

/// A comparison form: arrays sorted, keys ordered, so the order the platform lists things in
/// never makes two equal permission sets differ.
pub fn canonical(value: &Value) -> Value {
    match value {
        Value::Array(items) => {
            let mut canon: Vec<Value> = items.iter().map(canonical).collect();
            canon.sort_by_key(Value::to_string);
            Value::Array(canon)
        }
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<&String, Value> = map.iter().map(|(key, item)| (key, canonical(item))).collect();
            Value::Object(sorted.into_iter().map(|(key, item)| (key.clone(), item)).collect())
        }
        other => other.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Both exist and the new one already has what the old one had.
    Equal,
    /// The new one lacks some of it; a plan reports this, an apply writes it.
    Differs,
    /// Written and read back equal.
    Carried,
    OldAbsent,
    NewAbsent,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityResult {
    pub collection: String,
    pub old: String,
    pub new: String,
    pub status: Status,
    /// The permission sets that differ (plan) or were written (apply).
    pub kinds: Vec<&'static str>,
    pub differences: Option<usize>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct Report {
    pub entities: Vec<EntityResult>,
    pub ledger_changed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub pairs: Vec<Pair>,
    /// Also every entity of the ledger's entity and prefix renames not yet carried or deleted.
    pub renamed: bool,
    pub apply: bool,
    pub detail: bool,
}

/// Parse `Collection/Old Collection/New ...` into pairs.
pub fn pairs_from_names(names: &[String]) -> Result<Vec<Pair>, CarryError> {
    if !names.len().is_multiple_of(2) {
        return Err(CarryError::Arguments {
            why: "give entities in pairs: <Collection/Old> <Collection/New>".to_string(),
        });
    }
    let split = |text: &str| -> Result<(String, String), CarryError> {
        match text.split_once('/') {
            Some((collection, name)) if !collection.is_empty() && !name.is_empty() => Ok((collection.to_string(), name.to_string())),
            _ => Err(CarryError::Arguments {
                why: format!("{text:?} must be written Collection/Name"),
            }),
        }
    };
    names
        .chunks(2)
        .map(|pair| {
            let (collection, old) = split(&pair[0])?;
            let (new_collection, new) = split(&pair[1])?;
            if collection != new_collection {
                return Err(CarryError::Arguments {
                    why: format!("{} and {} are in different collections", pair[0], pair[1]),
                });
            }
            Ok(Pair { collection, old, new })
        })
        .collect()
}

/// Plan, or apply, the carrying of permissions.
pub fn run(
    remote: &dyn Remote,
    solution: &Solution,
    request: &Request,
    date: &str,
) -> Result<Report, CarryError> {
    let ledger_path = solution.root.join(ledger::RELATIVE_PATH);
    let mut ledger = Ledger::read(&ledger_path).map_err(|error| CarryError::Ledger { why: error.to_string() })?;
    let mapping = Mapping::from_ledger(&ledger);

    // (pair, where in the ledger it came from, if it did)
    let mut work: Vec<(Pair, Option<(usize, usize)>)> = request.pairs.iter().cloned().map(|pair| (pair, None)).collect();
    if request.renamed {
        for at in ledger.replaced(|entity| entity.carried.is_none() && entity.deleted.is_none()) {
            let entity = ledger.entity(at);
            let pair = Pair { collection: entity.collection.clone(), old: entity.old.clone(), new: entity.new.clone() };
            work.push((pair, Some(at)));
        }
    }
    if work.is_empty() {
        return Err(CarryError::Arguments {
            why: "nothing to carry: name pairs of entities, or pass --renamed for the ledger's pending renames".to_string(),
        });
    }

    let mut entities = Vec::new();
    let mut marks = Vec::new();
    for (pair, origin) in work {
        let mut result = EntityResult {
            collection: pair.collection.clone(),
            old: pair.old.clone(),
            new: pair.new.clone(),
            status: Status::Equal,
            kinds: Vec::new(),
            differences: None,
            error: None,
        };
        match carry_one(remote, &mapping, &pair, request, &mut result) {
            Ok(()) => {
                if matches!(result.status, Status::Equal | Status::Carried) {
                    if let Some(origin) = origin {
                        marks.push(origin);
                    }
                }
            }
            Err(error) => {
                result.status = Status::Failed;
                result.error = Some(error.to_string());
            }
        }
        entities.push(result);
    }

    let mut ledger_changed = false;
    if request.apply && !marks.is_empty() {
        for at in marks {
            ledger.entity_mut(at).carried = Some(date.to_string());
        }
        ledger.write(&ledger_path).map_err(|error| CarryError::Ledger { why: error.to_string() })?;
        ledger_changed = true;
    }
    Ok(Report { entities, ledger_changed })
}

fn carry_one(
    remote: &dyn Remote,
    mapping: &Mapping,
    pair: &Pair,
    request: &Request,
    result: &mut EntityResult,
) -> Result<(), ServerError> {
    if !remote.exists(&pair.collection, &pair.old)? {
        result.status = Status::OldAbsent;
        return Ok(());
    }
    if !remote.exists(&pair.collection, &pair.new)? {
        result.status = Status::NewAbsent;
        return Ok(());
    }
    let mut to_write = Vec::new();
    for kind in Kind::ALL {
        let old = remote.get(&pair.collection, &pair.old, kind)?;
        let new = remote.get(&pair.collection, &pair.new, kind)?;
        let wanted = mapping.map_value(&old);
        if canonical(&wanted) != canonical(&new) {
            to_write.push((kind, wanted));
        }
    }
    result.kinds = to_write.iter().map(|(kind, _)| kind.label()).collect();
    if to_write.is_empty() {
        result.status = Status::Equal;
        return measure(remote, pair, request, result);
    }
    if !request.apply {
        result.status = Status::Differs;
        return measure(remote, pair, request, result);
    }
    for (kind, wanted) in &to_write {
        remote.set(&pair.collection, &pair.new, *kind, wanted)?;
        // Read it back: a set that answers success and changes nothing is a failure, not a carry.
        let after = remote.get(&pair.collection, &pair.new, *kind)?;
        if canonical(&after) != canonical(wanted) {
            return Err(ServerError::InvalidResponse {
                url: format!("{}/{}", pair.collection, pair.new),
                why: format!("the {} permissions did not read back equal after the write", kind.label()),
            });
        }
    }
    result.status = Status::Carried;
    measure(remote, pair, request, result)
}

/// The platform's own count of what still differs, taken after any write.
fn measure(remote: &dyn Remote, pair: &Pair, request: &Request, result: &mut EntityResult) -> Result<(), ServerError> {
    if request.detail {
        result.differences = Some(remote.differences(&pair.collection, &pair.new, &pair.old)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake {
        entities: RefCell<BTreeMap<String, BTreeMap<&'static str, Value>>>,
        calls: RefCell<Vec<String>>,
        /// A set that answers success but stores nothing.
        ignore_sets: bool,
    }

    impl Fake {
        fn with(self, name: &str, run: Value, design: Value, visibility: Value) -> Self {
            self.entities.borrow_mut().insert(
                name.to_string(),
                BTreeMap::from([("run", run), ("design", design), ("vis", visibility)]),
            );
            self
        }
    }

    fn slot(kind: Kind) -> &'static str {
        match kind {
            Kind::RunTime => "run",
            Kind::DesignTime => "design",
            Kind::Visibility => "vis",
        }
    }

    impl Remote for Fake {
        fn exists(&self, _: &str, name: &str) -> Result<bool, ServerError> {
            Ok(self.entities.borrow().contains_key(name))
        }
        fn get(&self, _: &str, name: &str, kind: Kind) -> Result<Value, ServerError> {
            self.calls.borrow_mut().push(format!("GET {name} {}", slot(kind)));
            Ok(self.entities.borrow()[name][slot(kind)].clone())
        }
        fn set(&self, _: &str, name: &str, kind: Kind, value: &Value) -> Result<(), ServerError> {
            self.calls.borrow_mut().push(format!("SET {name} {}", slot(kind)));
            if !self.ignore_sets {
                self.entities.borrow_mut().get_mut(name).unwrap().insert(slot(kind), value.clone());
            }
            Ok(())
        }
        fn differences(&self, _: &str, _: &str, _: &str) -> Result<usize, ServerError> {
            Ok(4)
        }
    }

    fn principal(name: &str, kind: &str) -> Value {
        json!({ "name": name, "type": kind, "isPermitted": true })
    }

    fn run_perms(groups: &[&str]) -> Value {
        json!({ "permissions": [{ "resourceName": "*", "ServiceInvoke": groups.iter().map(|g| principal(g, "Group")).collect::<Vec<_>>() }] })
    }

    fn solution(tag: &str, ledger: Option<&str>) -> (std::path::PathBuf, Solution) {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("twaco-carry-{tag}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(root.join(".twaco")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        if let Some(ledger) = ledger {
            std::fs::write(root.join(".twaco/renames.json"), ledger).unwrap();
        }
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (root, solution)
    }

    const LEDGER: &str = r#"[
  { "date": "2026-10-01", "kind": "prefix", "old": "Acme.Old", "new": "Acme.New",
    "entities": [ { "collection": "Things", "old": "Acme.Old.Manager", "new": "Acme.New.Manager" },
                  { "collection": "Groups", "old": "Acme.Old.Admin_UG", "new": "Acme.New.Admin_UG" } ] }
]"#;

    #[test]
    fn principals_are_mapped_through_every_rename_by_whole_token() {
        let mapping = Mapping::from_ledger(&serde_json::from_str::<Ledger>(LEDGER).unwrap());
        assert_eq!(mapping.map_text("Acme.Old.Admin_UG"), "Acme.New.Admin_UG");
        // A compound organizational unit names both an organization and a group.
        assert_eq!(mapping.map_text("Acme.Old.Default_OR:Acme.Old.Admin_UG"), "Acme.New.Default_OR:Acme.New.Admin_UG");
        // Nothing else: another name, a longer name, and keys are left alone.
        assert_eq!(mapping.map_text("Acme.Older.X"), "Acme.Older.X");
        let mapped = mapping.map_value(&json!({ "Acme.Old.Key": ["Acme.Old.Manager", 1, true] }));
        assert_eq!(mapped, json!({ "Acme.Old.Key": ["Acme.New.Manager", 1, true] }));
    }

    #[test]
    fn comparison_ignores_the_order_the_platform_lists_things_in() {
        let a = json!({ "permissions": [ { "a": [1, 2], "resourceName": "x" }, { "resourceName": "y" } ] });
        let b = json!({ "permissions": [ { "resourceName": "y" }, { "resourceName": "x", "a": [2, 1] } ] });
        assert_eq!(canonical(&a), canonical(&b));
        assert_ne!(canonical(&a), canonical(&json!({ "permissions": [] })));
    }

    #[test]
    fn a_plan_only_reads_and_an_apply_writes_exactly_what_differs_and_reads_it_back() {
        let (root, solution) = solution("apply", Some(LEDGER));
        let fake = Fake::default()
            .with("Acme.Old.Manager", run_perms(&["Acme.Old.Admin_UG"]), json!({ "Read": [] }), json!({ "Visibility": [] }))
            .with("Acme.New.Manager", run_perms(&[]), json!({ "Read": [] }), json!({ "Visibility": [] }));
        let request = Request { pairs: pairs_from_names(&["Things/Acme.Old.Manager".into(), "Things/Acme.New.Manager".into()]).unwrap(), ..Default::default() };
        let plan = run(&fake, &solution, &request, "2026-10-02").unwrap();
        assert_eq!(plan.entities[0].status, Status::Differs);
        assert_eq!(plan.entities[0].kinds, ["run-time"]);
        assert!(fake.calls.borrow().iter().all(|call| call.starts_with("GET")), "a plan writes nothing");

        let applied = run(&fake, &solution, &Request { apply: true, detail: true, ..request.clone() }, "2026-10-02").unwrap();
        assert_eq!(applied.entities[0].status, Status::Carried);
        assert_eq!(applied.entities[0].differences, Some(4));
        let sets: Vec<String> = fake.calls.borrow().iter().filter(|call| call.starts_with("SET")).cloned().collect();
        assert_eq!(sets, ["SET Acme.New.Manager run"], "only the set that differed");
        // The principal was mapped to the new group's name.
        assert_eq!(fake.entities.borrow()["Acme.New.Manager"]["run"], run_perms(&["Acme.New.Admin_UG"]));
        // Equal now: a second apply writes nothing.
        let again = run(&fake, &solution, &Request { apply: true, ..request }, "2026-10-02").unwrap();
        assert_eq!(again.entities[0].status, Status::Equal);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_set_that_changes_nothing_is_a_failure_and_the_run_continues() {
        let (root, solution) = solution("silent", Some(LEDGER));
        let fake = Fake { ignore_sets: true, ..Default::default() }
            .with("Acme.Old.Manager", run_perms(&["G"]), json!({}), json!({}))
            .with("Acme.New.Manager", run_perms(&[]), json!({}), json!({}))
            .with("Acme.Old.Thing", run_perms(&[]), json!({}), json!({}))
            .with("Acme.New.Thing", run_perms(&[]), json!({}), json!({}));
        let names: Vec<String> = ["Things/Acme.Old.Manager", "Things/Acme.New.Manager", "Things/Acme.Old.Thing", "Things/Acme.New.Thing"].map(String::from).to_vec();
        let report = run(&fake, &solution, &Request { pairs: pairs_from_names(&names).unwrap(), apply: true, ..Default::default() }, "d").unwrap();
        assert_eq!(report.entities[0].status, Status::Failed);
        assert!(report.entities[0].error.as_ref().unwrap().contains("did not read back equal"));
        assert_eq!(report.entities[1].status, Status::Equal, "the next pair was still handled");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn renamed_takes_the_pending_ledger_entries_marks_them_and_skips_carried_and_absent_ones() {
        let (root, solution) = solution("ledger", Some(LEDGER));
        let fake = Fake::default()
            .with("Acme.Old.Manager", run_perms(&["Acme.Old.Admin_UG"]), json!({}), json!({}))
            .with("Acme.New.Manager", run_perms(&[]), json!({}), json!({}))
            .with("Acme.New.Admin_UG", json!({}), json!({}), json!({}));
        let request = Request { renamed: true, apply: true, ..Default::default() };
        let report = run(&fake, &solution, &request, "2026-10-02").unwrap();
        let status: Vec<(&str, Status)> = report.entities.iter().map(|entity| (entity.old.as_str(), entity.status.clone())).collect();
        assert_eq!(status, [("Acme.Old.Manager", Status::Carried), ("Acme.Old.Admin_UG", Status::OldAbsent)]);
        assert!(report.ledger_changed);
        let ledger: Value = serde_json::from_str(&std::fs::read_to_string(root.join(".twaco/renames.json")).unwrap()).unwrap();
        assert_eq!(ledger[0]["entities"][0]["carried"], "2026-10-02");
        assert!(ledger[0]["entities"][1].get("carried").is_none(), "an absent old entity is not marked");
        // Carried entries are not taken again.
        let second = run(&fake, &solution, &request, "2026-10-03").unwrap();
        assert_eq!(second.entities.len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn arguments_and_a_corrupt_ledger_are_refused_before_any_request() {
        assert!(pairs_from_names(&["Things/A".into()]).is_err());
        assert!(pairs_from_names(&["A".into(), "B".into()]).is_err());
        assert!(pairs_from_names(&["Things/A".into(), "Groups/B".into()]).is_err());
        let (root, solution) = solution("corrupt", Some("not json"));
        let fake = Fake::default();
        assert!(matches!(run(&fake, &solution, &Request { renamed: true, ..Default::default() }, "d"), Err(CarryError::Ledger { .. })));
        assert!(fake.calls.borrow().is_empty());
        let (empty_root, empty) = solution_without_pairs();
        assert!(matches!(run(&fake, &empty, &Request::default(), "d"), Err(CarryError::Arguments { .. })));
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(empty_root);
    }

    fn solution_without_pairs() -> (std::path::PathBuf, Solution) {
        solution("none", None)
    }
}
