//! Mashup bindings checked against the same entity model as `types` and `catalog`.

use super::catalog;
use super::config::Solution;
use super::mashup;
use super::types::{self, DataShape, Entity, Model, Service};
use super::workspace;
use serde::Serialize;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub(crate) const GATE: &str = "mashup bindings";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) file: String,
    pub(crate) rule: String,
    pub(crate) message: String,
}

pub(crate) struct Report {
    pub(crate) problems: Vec<Problem>,
    pub(crate) examined: usize,
    pub(crate) mashups: usize,
    pub(crate) not_checked: usize,
    pub(crate) platform_hint: bool,
    seen: BTreeSet<String>,
}

#[derive(Debug, Serialize)]
pub struct FieldsReport {
    /// DataShapes judged: the result of a service a mashup binds field by field. One bound whole
    /// (a grid shows every column) is used whole and not judged.
    pub(crate) judged: usize,
    /// DataShapes some mashup binds whole, so not judged.
    pub(crate) bound_whole: usize,
    /// Only the DataShapes with a field nothing uses.
    pub(crate) data_shapes: Vec<UnusedFields>,
    pub(crate) limits: &'static str,
}

#[derive(Debug, Serialize)]
pub struct UnusedFields {
    pub(crate) data_shape: String,
    pub(crate) services: Vec<String>,
    pub(crate) fields: Vec<String>,
}

pub(crate) const FIELDS_LIMITS: &str =
    "a field read by a script, another service or a REST client looks unused: only mashups are read";

/// Check all managed project mashups. Invalid mashup JSON belongs to the project gate, so it is
/// intentionally not repeated here.
pub(crate) fn run(solution: &Solution) -> Report {
    analyse(solution).0
}

pub(crate) fn unused_fields(solution: &Solution) -> FieldsReport {
    let (_, usage) = analyse(solution);
    // A field a mashup names anywhere counts as used: a grid's column settings, an expression, a
    // binding from another service. Scripts do not count: the service that fills a field always
    // names it, so every field would look used.
    let mut named = HashSet::new();
    for file in workspace::discover(solution)
        .entities
        .iter()
        .filter(|file| file.info.collection == "Mashups")
    {
        if let Ok(bytes) = std::fs::read(&file.path) {
            words(&String::from_utf8_lossy(&bytes), &mut named);
        }
    }
    let mut judged = 0;
    let mut bound_whole = 0;
    let mut data_shapes = Vec::new();
    for (name, seen) in usage.shapes {
        if seen.whole {
            bound_whole += 1;
            continue;
        }
        judged += 1;
        let fields: Vec<String> = seen
            .fields
            .iter()
            .filter(|field| !seen.bound.contains(*field))
            .filter(|field| !named.contains(field.as_str()))
            .cloned()
            .collect();
        if !fields.is_empty() {
            data_shapes.push(UnusedFields {
                data_shape: name,
                services: seen.services.into_iter().collect(),
                fields,
            });
        }
    }
    FieldsReport {
        judged,
        bound_whole,
        data_shapes,
        limits: FIELDS_LIMITS,
    }
}

#[derive(Default)]
struct FieldUsage {
    shapes: BTreeMap<String, SeenShape>,
}

struct SeenShape {
    fields: Vec<String>,
    services: BTreeSet<String>,
    bound: BTreeSet<String>,
    /// Some mashup binds the whole result, so every field may be shown.
    whole: bool,
}

fn analyse(solution: &Solution) -> (Report, FieldUsage) {
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let cache_missing = platform.is_none();
    let resolver = Resolver::new(&model, platform.as_ref());
    let model = &resolver;
    let discovery = workspace::discover(solution);
    let mut report = Report {
        problems: Vec::new(),
        examined: 0,
        mashups: 0,
        not_checked: 0,
        platform_hint: false,
        seen: BTreeSet::new(),
    };
    let mut usage = FieldUsage::default();
    for file in discovery
        .entities
        .into_iter()
        .filter(|file| file.info.collection == "Mashups")
    {
        let Ok(bytes) = std::fs::read(&file.path) else {
            continue;
        };
        let Ok(assets) = mashup::extract(&bytes) else {
            continue;
        };
        let Ok(content) = serde_json::from_str::<Value>(&assets.content) else {
            continue;
        };
        report.mashups += 1;
        let mashup = file.info.name.as_str();
        let relative = file
            .path
            .strip_prefix(&solution.root)
            .unwrap_or(&file.path)
            .to_string_lossy()
            .replace('\\', "/");
        let sections = data_sections(&content);
        for section in sections.values() {
            for service in &section.services {
                let lookup = model.service(section, &service.target);
                if let Some(found) = lookup.service.as_ref() {
                    report.examined += 1;
                    for parameter in &service.static_parameters {
                        let site = Site {
                            file: &relative,
                            mashup,
                            entity: &section.entity,
                            service: &service.target,
                        };
                        check_parameter(&mut report, &site, parameter, found);
                    }
                } else if lookup.complete {
                    report.examined += 1;
                    let site = Site {
                        file: &relative,
                        mashup,
                        entity: &section.entity,
                        service: &service.target,
                    };
                    add_problem(
                        &mut report,
                        &site,
                        "unknown-service",
                        "",
                        format!(
                            "{mashup} calls {}.{}, which {} does not have",
                            section.entity, service.target, section.entity
                        ),
                    );
                } else {
                    report.not_checked += 1;
                    report.platform_hint |= cache_missing;
                }
            }
        }
        for binding in content
            .get("DataBindings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if binding.get("SourceArea").and_then(Value::as_str) == Some("Data") {
                if let (Some(section), Some(instance), Some(property)) = (
                    binding.get("SourceSection").and_then(Value::as_str),
                    binding.get("SourceId").and_then(Value::as_str),
                    binding.get("PropertyMaps").and_then(Value::as_array),
                ) {
                    if let Some((section, service)) = service_at(&sections, section, instance) {
                        let lookup = model.service(section, &service.target);
                        if let Some(found) = lookup.service.as_ref() {
                            track_result(
                                &mut usage,
                                &section.entity,
                                &service.target,
                                found,
                                model,
                            );
                        }
                        for map in property {
                            let name = map
                                .get("SourceProperty")
                                .and_then(Value::as_str)
                                .filter(|value| !value.is_empty());
                            let kind = map.get("SourcePropertyType").and_then(Value::as_str);
                            let Some(name) = name.filter(|_| kind == Some("Field")) else {
                                // The whole table (AllData, SelectedRows) goes to the widget.
                                if name.is_none() || kind == Some("InfoTable") {
                                    if let Some(shape) = lookup
                                        .service
                                        .as_ref()
                                        .and_then(|found| found.result.data_shape.as_ref())
                                        .and_then(|shape| usage.shapes.get_mut(shape))
                                    {
                                        shape.whole = true;
                                    }
                                }
                                continue;
                            };
                            check_field(
                                &mut report,
                                &mut usage,
                                &Site {
                                    file: &relative,
                                    mashup,
                                    entity: &section.entity,
                                    service: &service.target,
                                },
                                name,
                                &lookup,
                                model,
                            );
                            report.platform_hint |=
                                cache_missing && lookup.service.is_none() && !lookup.complete;
                        }
                    }
                }
            }
            if binding.get("TargetArea").and_then(Value::as_str) == Some("Data") {
                if let (Some(section), Some(instance), Some(property)) = (
                    binding.get("TargetSection").and_then(Value::as_str),
                    binding.get("TargetId").and_then(Value::as_str),
                    binding.get("PropertyMaps").and_then(Value::as_array),
                ) {
                    if let Some((section, service)) = service_at(&sections, section, instance) {
                        let lookup = model.service(section, &service.target);
                        for map in property {
                            if map.get("TargetPropertyType").and_then(Value::as_str)
                                != Some("Parameter")
                            {
                                continue;
                            }
                            let Some(name) = map
                                .get("TargetProperty")
                                .and_then(Value::as_str)
                                .filter(|value| !value.is_empty())
                            else {
                                continue;
                            };
                            if let Some(found) = lookup.service.as_ref() {
                                check_parameter(
                                    &mut report,
                                    &Site {
                                        file: &relative,
                                        mashup,
                                        entity: &section.entity,
                                        service: &service.target,
                                    },
                                    name,
                                    found,
                                );
                            } else if !lookup.complete {
                                // A complete chain without the service is reported once, as
                                // unknown-service on the call; its parameters add nothing.
                                report.not_checked += 1;
                                report.platform_hint |= cache_missing;
                            }
                        }
                    }
                }
            }
        }
    }
    report
        .problems
        .sort_by(|a, b| (&a.file, &a.rule, &a.message).cmp(&(&b.file, &b.rule, &b.message)));
    (report, usage)
}

pub(crate) struct Section {
    pub(crate) entity: String,
    pub(crate) collection: String,
    pub(crate) services: Vec<Call>,
}
pub(crate) struct Call {
    /// The mashup's name for the call (`Name`), which bindings and events address.
    pub(crate) instance: String,
    /// The service it calls (`Target`).
    pub(crate) target: String,
    pub(crate) static_parameters: Vec<String>,
}

struct Site<'a> {
    file: &'a str,
    mashup: &'a str,
    entity: &'a str,
    service: &'a str,
}

pub(crate) fn data_sections(content: &Value) -> BTreeMap<String, Section> {
    let mut out = BTreeMap::new();
    let Some(data) = content.get("Data").and_then(Value::as_object) else {
        return out;
    };
    for (name, section) in data {
        let Some(entity) = section
            .get("EntityName")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let Some(collection) = collection(
            section
                .get("EntityType")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ) else {
            continue;
        };
        let services = section
            .get("Services")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|service| {
                let instance = service.get("Name")?.as_str()?.to_string();
                let target = service
                    .get("Target")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .unwrap_or(&instance)
                    .to_string();
                let static_parameters = service
                    .get("Parameters")
                    .and_then(Value::as_object)
                    .map(|parameters| parameters.keys().cloned().collect())
                    .unwrap_or_default();
                Some(Call {
                    instance,
                    target,
                    static_parameters,
                })
            })
            .collect();
        out.insert(
            name.clone(),
            Section {
                entity: entity.to_string(),
                collection: collection.to_string(),
                services,
            },
        );
    }
    out
}

fn collection(kind: &str) -> Option<&'static str> {
    match kind {
        "Things" | "Thing" => Some("Things"),
        "ThingTemplates" | "ThingTemplate" | "DynamicThingTemplates" => Some("ThingTemplates"),
        "ThingShapes" | "ThingShape" | "DynamicThingShapes" => Some("ThingShapes"),
        _ => None,
    }
}

fn service_at<'a>(
    sections: &'a BTreeMap<String, Section>,
    section: &str,
    instance: &str,
) -> Option<(&'a Section, &'a Call)> {
    let section = sections.get(section)?;
    section
        .services
        .iter()
        .find(|service| service.instance == instance)
        .map(|service| (section, service))
}

/// The model indexed by name, and each service looked up once: a large solution binds the same
/// few services from hundreds of mashups.
pub(crate) struct Resolver<'a> {
    model: &'a Model,
    platform: Option<&'a types::Platform>,
    entities: HashMap<(&'a str, &'a str), &'a Entity>,
    shapes: HashMap<&'a str, &'a DataShape>,
    looked_up: RefCell<HashMap<(String, String, String), catalog::ServiceLookup>>,
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(model: &'a Model, platform: Option<&'a types::Platform>) -> Self {
        let mut entities = HashMap::new();
        for entity in &model.entities {
            // The first of two same-named entities, as a search through the list finds it.
            entities
                .entry((entity.collection.as_str(), entity.name.as_str()))
                .or_insert(entity);
        }
        let mut shapes = HashMap::new();
        for shape in &model.data_shapes {
            shapes.entry(shape.name.as_str()).or_insert(shape);
        }
        Resolver {
            model,
            platform,
            entities,
            shapes,
            looked_up: RefCell::new(HashMap::new()),
        }
    }

    pub(crate) fn service(&self, section: &Section, service: &str) -> catalog::ServiceLookup {
        let key = (
            section.collection.clone(),
            section.entity.clone(),
            service.to_string(),
        );
        if let Some(found) = self.looked_up.borrow().get(&key) {
            return found.clone();
        }
        let found = self
            .entities
            .get(&(section.collection.as_str(), section.entity.as_str()))
            .map(|entity| {
                catalog::lookup_service(entity, &self.model.entities, self.platform, service)
            })
            .unwrap_or(catalog::ServiceLookup {
                service: None,
                complete: false,
            });
        self.looked_up.borrow_mut().insert(key, found.clone());
        found
    }

    /// A DataShape whose fields are all in the repository: present, and not derived.
    fn checkable_shape(&self, name: &str) -> Option<&'a DataShape> {
        self.shapes
            .get(name)
            .copied()
            .filter(|shape| shape.base_data_shape.is_none())
    }
}

fn head(name: &str) -> &str {
    name.split(['[', '.']).next().unwrap_or(name)
}

fn check_parameter(report: &mut Report, site: &Site<'_>, name: &str, found: &Service) {
    report.examined += 1;
    let part = head(name);
    if !found
        .parameters
        .iter()
        .any(|parameter| parameter.name == part)
    {
        add_problem(
            report,
            site,
            "unknown-parameter",
            part,
            format!(
                "{} passes {name} to {}.{}, which has no parameter {part}",
                site.mashup, site.entity, site.service
            ),
        );
    }
}

fn check_field(
    report: &mut Report,
    usage: &mut FieldUsage,
    site: &Site<'_>,
    name: &str,
    lookup: &catalog::ServiceLookup,
    model: &Resolver<'_>,
) {
    let Some(service) = lookup.service.as_ref() else {
        report.not_checked += 1;
        return;
    };
    if !service.result.base_type.eq_ignore_ascii_case("INFOTABLE") {
        report.not_checked += 1;
        return;
    }
    let Some(shape_name) = service.result.data_shape.as_deref() else {
        report.not_checked += 1;
        return;
    };
    let Some(shape) = model.checkable_shape(shape_name) else {
        report.not_checked += 1;
        return;
    };
    report.examined += 1;
    let part = head(name);
    track_result(usage, site.entity, site.service, service, model);
    let entry = usage.shapes.get_mut(&shape.name).expect("tracked above");
    entry.bound.insert(part.to_string());
    if !shape.fields.iter().any(|field| field.name == part) {
        add_problem(report, site, "unknown-field", part, format!("{} binds {name} from {}.{}, but its result {shape_name} has no field {part}; the widget gets no value", site.mashup, site.entity, site.service));
    }
}

fn track_result(
    usage: &mut FieldUsage,
    entity: &str,
    service_name: &str,
    service: &Service,
    model: &Resolver<'_>,
) {
    if !service.result.base_type.eq_ignore_ascii_case("INFOTABLE") {
        return;
    }
    let Some(shape_name) = service.result.data_shape.as_deref() else {
        return;
    };
    let Some(shape) = model.checkable_shape(shape_name) else {
        return;
    };
    let entry = usage
        .shapes
        .entry(shape.name.clone())
        .or_insert_with(|| SeenShape {
            fields: shape
                .fields
                .iter()
                .map(|field| field.name.clone())
                .collect(),
            services: BTreeSet::new(),
            bound: BTreeSet::new(),
            whole: false,
        });
    entry.services.insert(format!("{entity}.{service_name}"));
}

fn add_problem(report: &mut Report, site: &Site<'_>, rule: &str, name: &str, message: String) {
    let key = format!(
        "{}\0{rule}\0{}\0{}\0{name}",
        site.file, site.entity, site.service
    );
    if report.seen.insert(key) {
        report.problems.push(Problem {
            file: site.file.to_string(),
            rule: rule.to_string(),
            message,
        });
    }
}

/// Every word of `text` into `out`. A word is letters, digits, `_` and `-`, so a field named
/// `foo` is not found inside `foo-bar`.
fn words(text: &str, out: &mut HashSet<String>) {
    for word in text.split(|character: char| {
        !(character.is_alphanumeric() || character == '_' || character == '-')
    }) {
        if !word.is_empty() && !out.contains(word) {
            out.insert(word.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::check;
    use crate::core::config::Solution;
    use std::path::Path;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn solution(content: &str) -> (tempfile::TempDir, Solution) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "twaco.toml", "[[project]]\nname = \"P\"\ncollections = [\"Things\", \"ThingTemplates\", \"ThingShapes\", \"DataShapes\", \"Mashups\"]\n");
        write(
            root,
            "DataShapes/P.Rows.xml",
            r#"<Entities><DataShapes><DataShape name="P.Rows"><FieldDefinitions><FieldDefinition name="good" baseType="STRING"/><FieldDefinition name="scripted" baseType="STRING"/><FieldDefinition name="unused" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
        );
        write(
            root,
            "ThingShapes/P.Shape.xml",
            r#"<Entities><ThingShapes><ThingShape name="P.Shape"><ServiceDefinitions><ServiceDefinition name="Svc"><ParameterDefinitions><FieldDefinition name="p" baseType="STRING"/></ParameterDefinitions><ResultType baseType="INFOTABLE" aspect.dataShape="P.Rows"/></ServiceDefinition></ServiceDefinitions><ServiceImplementations/></ThingShape></ThingShapes></Entities>"#,
        );
        write(
            root,
            "Things/P.Thing.xml",
            r#"<Entities><Things><Thing name="P.Thing"><ThingShape><ServiceDefinitions/><ServiceImplementations/></ThingShape><ImplementedShapes><ImplementedShape name="P.Shape"/></ImplementedShapes></Thing></Things></Entities>"#,
        );
        write(
            root,
            "Mashups/P.View.xml",
            &format!(
                r#"<Entities><Mashups><Mashup name="P.View"><mashupContent><![CDATA[{content}]]></mashupContent></Mashup></Mashups></Entities>"#
            ),
        );
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        (dir, solution)
    }

    fn mashup(field: &str, parameter: &str, static_parameter: &str) -> String {
        format!(
            r#"{{"Data":{{"Shape":{{"EntityName":"P.Shape","EntityType":"DynamicThingShapes","Services":[{{"Name":"Svc1","Target":"Svc","Parameters":{{"{static_parameter}":"x"}}}}]}},"Thing":{{"EntityName":"P.Thing","EntityType":"Things","Services":[{{"Name":"Svc","Target":"Svc"}}]}}}},"DataBindings":[{{"SourceArea":"Data","SourceSection":"Shape","SourceId":"Svc1","PropertyMaps":[{{"SourcePropertyType":"Field","SourceProperty":"{field}"}}]}},{{"TargetArea":"Data","TargetSection":"Thing","TargetId":"Svc","PropertyMaps":[{{"TargetPropertyType":"Parameter","TargetProperty":"{parameter}"}}]}}]}}"#
        )
    }

    #[test]
    fn checks_target_not_instance_and_nested_heads() {
        let (_dir, solution) = solution(&mashup("good[AllData].x", "p.sub", "p"));
        let report = run(&solution);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.examined, 5);
    }

    #[test]
    fn reports_distinct_missing_fields_and_parameters() {
        let (_dir, solution) = solution(&mashup("missing.x", "bad.sub", "also_bad"));
        let report = run(&solution);
        let rules: Vec<&str> = report
            .problems
            .iter()
            .map(|problem| problem.rule.as_str())
            .collect();
        assert_eq!(
            rules,
            ["unknown-field", "unknown-parameter", "unknown-parameter"]
        );
        assert!(report
            .problems
            .iter()
            .any(|problem| problem.message.contains("missing")));
        assert!(report
            .problems
            .iter()
            .any(|problem| problem.message.contains("bad")));
    }

    #[test]
    fn incomplete_platform_chain_is_silent_until_cached() {
        let (_dir, solution) = solution(
            r#"{"Data":{"Shape":{"EntityName":"P.Shape","EntityType":"DynamicThingShapes","Services":[{"Name":"Nope","Target":"Nope"}]}}}"#,
        );
        let report = run(&solution);
        assert!(report.problems.is_empty());
        assert!(report.platform_hint);
        std::fs::create_dir_all(solution.root.join(".twaco")).unwrap();
        std::fs::write(solution.root.join(".twaco/platform.json"), r#"{"templates":{"GenericThing":{"services":{"GetPropertyValues":{"inputs":{},"result":{"baseType":"INFOTABLE"}}}}},"shapes":{},"resources":{}}"#).unwrap();
        let report = run(&solution);
        assert_eq!(report.problems.len(), 1);
        assert_eq!(report.problems[0].rule, "unknown-service");
    }

    #[test]
    fn unused_fields_lists_what_no_mashup_binds_even_when_a_script_names_it() {
        let (dir, solution) = solution(&mashup("good", "p", "p"));
        // The service that fills a field names it; that is not use.
        write(
            dir.path(),
            "src/P.Thing/services/S/script.js",
            "result.scripted = 1;",
        );
        let report = unused_fields(&solution);
        assert_eq!((report.judged, report.bound_whole), (1, 0));
        assert_eq!(report.data_shapes.len(), 1);
        assert_eq!(report.data_shapes[0].fields, ["scripted", "unused"]);
        assert_eq!(report.data_shapes[0].services, ["P.Shape.Svc"]);
    }

    const GENERIC_THING: &str = r#"{"templates":{"GenericThing":{"services":{"GetPropertyValues":{"inputs":{},"result":{"baseType":"INFOTABLE"}}}}},"shapes":{},"resources":{}}"#;

    fn cache_platform(solution: &Solution) {
        std::fs::create_dir_all(solution.root.join(".twaco")).unwrap();
        std::fs::write(solution.root.join(".twaco/platform.json"), GENERIC_THING).unwrap();
    }

    #[test]
    fn a_platform_service_is_fine_once_the_platform_is_cached() {
        let (_dir, solution) = solution(
            r#"{"Data":{"Shape":{"EntityName":"P.Shape","EntityType":"DynamicThingShapes","Services":[{"Name":"GetPropertyValues","Target":"GetPropertyValues"}]}}}"#,
        );
        cache_platform(&solution);
        let report = run(&solution);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!((report.examined, report.not_checked), (1, 0));
        assert!(!report.platform_hint);
    }

    #[test]
    fn a_thing_on_a_platform_template_the_cache_does_not_hold_is_not_checked() {
        let (dir, solution) = solution(
            r#"{"Data":{"T":{"EntityName":"P.Other","EntityType":"Things","Services":[{"Name":"Nope","Target":"Nope"}]}}}"#,
        );
        write(
            dir.path(),
            "Things/P.Other.xml",
            r#"<Entities><Things><Thing name="P.Other" thingTemplate="RemoteThing"><ThingShape><ServiceDefinitions/><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
        );
        cache_platform(&solution);
        let report = run(&solution);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.not_checked, 1);
    }

    #[test]
    fn a_service_inherited_through_a_templates_shape_is_found() {
        let (dir, solution) = solution(
            r#"{"Data":{"T":{"EntityName":"P.Child","EntityType":"Things","Services":[{"Name":"Svc","Target":"Svc"}]}},"DataBindings":[{"SourceArea":"Data","SourceSection":"T","SourceId":"Svc","PropertyMaps":[{"SourcePropertyType":"Field","SourceProperty":"nope"}]}]}"#,
        );
        write(
            dir.path(),
            "ThingTemplates/P.Base_TT.xml",
            r#"<Entities><ThingTemplates><ThingTemplate name="P.Base_TT" baseThingTemplate="GenericThing"><ThingShape><ServiceDefinitions/><ServiceImplementations/></ThingShape><ImplementedShapes><ImplementedShape name="P.Shape"/></ImplementedShapes></ThingTemplate></ThingTemplates></Entities>"#,
        );
        write(
            dir.path(),
            "Things/P.Child.xml",
            r#"<Entities><Things><Thing name="P.Child" thingTemplate="P.Base_TT"><ThingShape><ServiceDefinitions/><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
        );
        let report = run(&solution);
        let rules: Vec<&str> = report.problems.iter().map(|p| p.rule.as_str()).collect();
        assert_eq!(rules, ["unknown-field"], "{:?}", report.problems);
    }

    #[test]
    fn a_result_outside_the_repository_or_with_a_base_is_not_checked() {
        for rows in [
            None,
            Some(
                r#"<Entities><DataShapes><DataShape name="P.Rows" baseDataShape="P.Base"><FieldDefinitions><FieldDefinition name="good" baseType="STRING"/></FieldDefinitions></DataShape></DataShapes></Entities>"#,
            ),
        ] {
            let (dir, solution) = solution(&mashup("inherited", "p", "p"));
            let path = dir.path().join("DataShapes/P.Rows.xml");
            match rows {
                None => std::fs::remove_file(path).unwrap(),
                Some(text) => std::fs::write(path, text).unwrap(),
            }
            let report = run(&solution);
            assert!(report.problems.is_empty(), "{:?}", report.problems);
            assert_eq!(report.not_checked, 1);
        }
    }

    #[test]
    fn session_sections_and_entity_targets_are_neither_checked_nor_counted() {
        let (_dir, solution) = solution(
            r#"{"Data":{"Session":{"EntityName":"","EntityType":"Session","Services":[{"Name":"GetGlobalSessionValues","Target":"GetGlobalSessionValues"}]},"Shape":{"EntityName":"P.Shape","EntityType":"DynamicThingShapes","Services":[{"Name":"Svc","Target":"Svc"}]}},"DataBindings":[{"SourceArea":"Data","SourceSection":"Session","SourceId":"GetGlobalSessionValues","PropertyMaps":[{"SourcePropertyType":"Field","SourceProperty":"anything"}]},{"TargetArea":"Data","TargetSection":"Shape","TargetId":"EntityName","PropertyMaps":[{"TargetPropertyType":"Entity","TargetProperty":"value"}]}]}"#,
        );
        let report = run(&solution);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!((report.examined, report.not_checked), (1, 0));
    }

    #[test]
    fn a_result_bound_whole_is_not_judged_and_a_field_named_in_a_mashup_is_used() {
        let whole = r#"{"Data":{"Shape":{"EntityName":"P.Shape","EntityType":"DynamicThingShapes","Services":[{"Name":"Svc","Target":"Svc"}]}},"DataBindings":[{"SourceArea":"Data","SourceSection":"Shape","SourceId":"Svc","SourceDetails":"AllData","PropertyMaps":[{"SourcePropertyType":"InfoTable","SourceProperty":""}]}]}"#;
        let (_dir, bound_whole) = super::tests::solution(whole);
        let report = unused_fields(&bound_whole);
        assert_eq!((report.judged, report.bound_whole), (0, 1));
        assert!(report.data_shapes.is_empty());

        let named = r#"{"Data":{"Shape":{"EntityName":"P.Shape","EntityType":"DynamicThingShapes","Services":[{"Name":"Svc","Target":"Svc"}]}},"DataBindings":[{"SourceArea":"Data","SourceSection":"Shape","SourceId":"Svc","PropertyMaps":[{"SourcePropertyType":"Field","SourceProperty":"good"}]}],"UI":{"ColumnFormat":"scripted"}}"#;
        let (_named_dir, named) = super::tests::solution(named);
        let report = unused_fields(&named);
        assert_eq!(report.judged, 1);
        assert_eq!(report.data_shapes[0].fields, ["unused"]);
    }

    #[test]
    fn generic_thing_does_not_answer_for_a_template_twaco_cannot_see() {
        // RemoteThing may declare GetPropertyValues with other parameters; nothing is known.
        let (dir, solution) = solution(
            r#"{"Data":{"T":{"EntityName":"P.Other","EntityType":"Things","Services":[{"Name":"GetPropertyValues","Target":"GetPropertyValues","Parameters":{"x":"1"}}]}}}"#,
        );
        write(
            dir.path(),
            "Things/P.Other.xml",
            r#"<Entities><Things><Thing name="P.Other" thingTemplate="RemoteThing"><ThingShape><ServiceDefinitions/><ServiceImplementations/></ThingShape></Thing></Things></Entities>"#,
        );
        cache_platform(&solution);
        let report = run(&solution);
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!((report.examined, report.not_checked), (0, 1));
    }

    #[test]
    fn an_infotable_map_with_a_source_name_is_still_the_whole_table() {
        let content = r#"{"Data":{"Shape":{"EntityName":"P.Shape","EntityType":"DynamicThingShapes","Services":[{"Name":"Svc","Target":"Svc"}]}},"DataBindings":[{"SourceArea":"Data","SourceSection":"Shape","SourceId":"Svc","PropertyMaps":[{"SourcePropertyType":"Field","SourceProperty":"good"},{"SourcePropertyType":"InfoTable","SourceProperty":"AllData"}]}]}"#;
        let (_dir, solution) = solution(content);
        let report = unused_fields(&solution);
        assert_eq!((report.judged, report.bound_whole), (0, 1));
    }

    #[test]
    fn a_field_is_not_found_inside_a_longer_hyphenated_name() {
        let content = r#"{"Data":{"Shape":{"EntityName":"P.Shape","EntityType":"DynamicThingShapes","Services":[{"Name":"Svc","Target":"Svc"}]}},"DataBindings":[{"SourceArea":"Data","SourceSection":"Shape","SourceId":"Svc","PropertyMaps":[{"SourcePropertyType":"Field","SourceProperty":"good"}]}],"UI":{"Label":"unused-label scripted"}}"#;
        let (_dir, solution) = solution(content);
        let report = unused_fields(&solution);
        assert_eq!(report.data_shapes[0].fields, ["unused"]);
    }

    #[test]
    fn check_includes_the_gate_and_advisory_does_not_block() {
        let (_dir, mut solution) = solution(&mashup("missing", "p", "p"));
        let report = check::run(&solution);
        let gate = report.gates.iter().find(|gate| gate.name == GATE).unwrap();
        assert!(gate.blocks());
        solution.gates.advisory.push(GATE.to_string());
        let report = check::run(&solution);
        assert!(!report
            .gates
            .iter()
            .find(|gate| gate.name == GATE)
            .unwrap()
            .blocks());
    }
}
