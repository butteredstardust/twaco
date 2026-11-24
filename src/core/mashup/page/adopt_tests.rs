//! Adopting a mashup made by hand: derive its spec, build it, and nothing changes.

use super::super::extract as extraction;
use super::*;
use crate::core::mashup::Assets;

pub(super) const HAND_SPEC: &str = "version: 1
rules: v1
page:
  name: Acme.Hand_MU
  template: blank
  layout: as-built
  master: Acme.Master_MM
  use_master_theme: true
  root: {props: {DisplayName: page, CustomClass: acme-page, Margin: \"\", ShowVerticalScrollbar: true}}
  data:
    rows: {service: Acme.Data.GetRows, on: load}
  slots:
    center:
      - row:
          id: head
          props: {CustomClass: acme-title, flex-wrap: wrap, DisplayName: title}
          children:
            - heading: {id: heading, text: Acme}
            - button: {id: refresh, text: Refresh}
      - column:
          id: body
          children:
            - widget: {id: editor, type: ptcsrte, props: {DefaultFontSize: 14}}
            - label: {id: out, props: {LabelText: {bind: rows.Name}}}
            - embed: {id: inner, mashup: Acme.Child_MU}
            - collection: {id: cards, source: {bind: rows}, mashup: Acme.Child_MU, map: {Name: id}, props: {CellWidth: 360}}
  events:
    - {on: refresh.Clicked, invoke: rows}
";

fn strip_markers(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|key, _| !key.starts_with("twaco"));
            map.values_mut().for_each(strip_markers);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_markers),
        _ => {}
    }
}

/// Ids as Composer numbers them (`type-3`, `type-4`...) and ids for the wiring that no hash made.
fn composer_ids(text: &str) -> String {
    let mut out = text.to_string();
    let mut found: Vec<String> = vec![];
    let mut rest = text;
    while let Some(at) = rest.find('"') {
        let after = &rest[at + 1..];
        let Some(end) = after.find('"') else { break };
        let word = &after[..end];
        if let Some((kind, number)) = word.rsplit_once('-') {
            if number.len() >= 4
                && number.chars().all(|c| c.is_ascii_digit())
                && kind
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && !found.contains(&number.to_string())
            {
                found.push(number.to_string());
            }
        }
        if word.len() == 36
            && word.matches('-').count() == 4
            && word.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        {
            let mut key = word.to_string();
            key.insert(0, '#');
            if !found.contains(&key) {
                found.push(key);
            }
        }
        rest = &after[end + 1..];
    }
    let (mut next, mut uuid) = (3, 1);
    for token in found {
        if let Some(old) = token.strip_prefix('#') {
            out = out.replace(old, &format!("00000000-0000-4000-8000-{uuid:012}"));
            uuid += 1;
        } else {
            out = out.replace(&format!("-{token}\""), &format!("-{next}\""));
            next += 1;
        }
    }
    out
}

/// A mashup nobody built from a spec: no twaco markers, Composer-style ids, and settings no rule
/// would choose. Returns its name.
pub(super) fn handmade(solution: &Solution, spec: &str, name: &str) -> String {
    let built = compile(solution, "Acme", spec).unwrap();
    let mut value: Value = serde_json::from_str(&built.content).unwrap();
    strip_markers(&mut value);
    let text = composer_ids(&serde_json::to_string_pretty(&value).unwrap());
    let mut value: Value = serde_json::from_str(&text).unwrap();
    value["UI"]["Properties"]["id_index"] = 3.into();
    fn tweak(widget: &mut Value) {
        let name = widget["Properties"]["DisplayName"].as_str().unwrap_or("");
        match name {
            "title" => {
                widget["Properties"]["align-items"] = "center".into();
                widget["Properties"]["flex-shrink"] = 0.into();
            }
            n if n.starts_with("ptcs-label") => widget["Properties"]["Margin"] = "3".into(),
            _ => {}
        }
        for child in widget["Widgets"].as_array_mut().into_iter().flatten() {
            tweak(child);
        }
    }
    tweak(&mut value["UI"]);
    let assets = Assets {
        content: mashup::to_sidecar(&value),
        css: String::new(),
    };
    let entity = mashup::sync(&built.entity, &assets).unwrap().0;
    std::fs::write(
        solution.root.join(format!("acme/Mashups/{name}.xml")),
        entity,
    )
    .unwrap();
    name.to_string()
}

fn widget_ids(content: &Value) -> Vec<String> {
    fn walk(widget: &Value, out: &mut Vec<String>) {
        out.push(widget["Properties"]["Id"].as_str().unwrap().to_string());
        for child in widget["Widgets"].as_array().into_iter().flatten() {
            walk(child, out);
        }
    }
    let mut out = vec![];
    walk(&content["UI"], &mut out);
    out
}

pub(super) fn current(solution: &Solution, name: &str) -> Value {
    let loaded = authoring::load_existing(solution, name).unwrap();
    serde_json::from_str(&loaded.assets.content).unwrap()
}

pub(super) fn write_spec(solution: &Solution, name: &str, spec: &str) {
    let dir = solution.src_root().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("page.yaml"), spec).unwrap();
}

pub(super) fn apply(solution: &Solution, name: &str) -> Value {
    let mut plan = super::super::build::plan(solution, name, None, None).unwrap();
    assert!(!plan.conflicts(), "{}", plan.report);
    let guard = lock::acquire(&solution.root, "page test", &[]).unwrap();
    plan.apply(solution, &guard).unwrap();
    plan.report
}

#[test]
fn a_derived_spec_adopts_its_mashup_with_no_extra_level_and_the_same_ids() {
    let (_dir, solution) = element_solution();
    let name = handmade(&solution, HAND_SPEC, "Acme.Hand_MU");
    let before = current(&solution, &name);
    let ids = widget_ids(&before);
    assert!(
        ids.iter()
            .filter(|id| *id != "mashup-root")
            .all(|id| id.rsplit('-').next().unwrap().len() < 3),
        "{ids:?}"
    );

    let derived = extraction::extract(&solution, &name).unwrap();
    assert!(
        derived.spec.contains("layout: as-built"),
        "{}",
        derived.spec
    );
    assert!(derived.spec.contains("master: Acme.Master_MM"));
    assert!(derived.spec.contains("use_master_theme: true"));
    assert!(
        derived.spec.contains("root:") && derived.spec.contains("CustomClass: acme-page"),
        "{}",
        derived.spec
    );
    // The root container is the template's: its children are the slot, not one more level.
    assert!(
        !derived.spec.contains("id: flexcontainer-2"),
        "{}",
        derived.spec
    );
    write_spec(&solution, &name, &derived.spec);

    let files = [
        solution.root.join(format!("acme/Mashups/{name}.xml")),
        solution.src_root().join(&name).join(".page-baseline.json"),
    ];
    let xml_before = std::fs::read(&files[0]).unwrap();
    let report = apply(&solution, &name);
    assert_eq!(report["conflicts"], serde_json::json!([]));
    assert_eq!(
        std::fs::read(&files[0]).unwrap(),
        xml_before,
        "adopting changes no byte of the mashup"
    );
    let after = current(&solution, &name);
    assert_eq!(widget_ids(&after), ids);
    assert_eq!(after, before);

    // The baseline keeps the ids, so every widget is a node of the spec under its own id.
    let baseline =
        super::super::baseline::Baseline::read(&std::fs::read(&files[1]).unwrap()).unwrap();
    assert_eq!(widget_ids(&baseline.content), ids);
    for id in ids
        .iter()
        .filter(|id| *id != "mashup-root" && *id != "flexcontainer-2")
    {
        assert!(
            baseline.ids.values().any(|w| w == id) || id.starts_with("expression"),
            "{id} is not a node: {:?}",
            baseline.ids
        );
    }

    // Nothing changes on the next build either.
    let again = super::super::build::plan(&solution, &name, None, None).unwrap();
    assert_eq!(
        again.report["files"],
        serde_json::json!([]),
        "{}",
        again.report
    );
    assert!(!again.conflicts());
    // A pristine adoption has nothing to report as kept hand edits on the next build.
    assert_eq!(
        again.report["hand_edits_kept"],
        serde_json::json!([]),
        "{}",
        again.report
    );
}

#[test]
fn nodes_added_after_adopting_get_stable_hashed_ids_and_the_old_ids_stay() {
    let (_dir, solution) = element_solution();
    let name = handmade(&solution, HAND_SPEC, "Acme.Hand_MU");
    let before = widget_ids(&current(&solution, &name));
    let derived = extraction::extract(&solution, &name).unwrap();
    write_spec(&solution, &name, &derived.spec);
    apply(&solution, &name);
    let node = "      - column: {id: extra, children: [{label: {id: extra_label, text: Added}}]}
";
    let more = match derived.spec.find("  events:") {
        Some(at) => format!("{}{node}{}", &derived.spec[..at], &derived.spec[at..]),
        None => format!("{}{node}", derived.spec),
    };
    write_spec(&solution, &name, &more);
    apply(&solution, &name);
    let first = widget_ids(&current(&solution, &name));
    for id in &before {
        assert!(first.contains(id), "{id} was renamed: {first:?}");
    }
    let added: Vec<_> = first.iter().filter(|id| !before.contains(id)).collect();
    assert!(!added.is_empty());
    // Rebuilding from the same spec gives the same ids again.
    let plan = super::super::build::plan(&solution, &name, None, None).unwrap();
    assert_eq!(
        plan.report["files"],
        serde_json::json!([]),
        "{}",
        plan.report
    );
}

#[test]
fn derived_nodes_keep_what_the_mashup_has() {
    let (_dir, solution) = element_solution();
    let name = handmade(&solution, HAND_SPEC, "Acme.Hand_MU");
    let derived = extraction::extract(&solution, &name).unwrap();
    // The settings no v1 rule would choose are in the spec, so the build does not overwrite them.
    for needle in [
        "CustomClass: acme-page",
        "Margin: \"\"",
        "ShowVerticalScrollbar: true",
        "flex-wrap: wrap",
        "flex-shrink: 0",
        "align-items: center",
        "CustomClass: acme-title",
        "DefaultFontSize: 14",
        // A collection keeps its cell mashup and its cell size; no preset is generated again.
        "mashup: Acme.Child_MU",
        "CellWidth: 360",
    ] {
        assert!(derived.spec.contains(needle), "{needle}\n{}", derived.spec);
    }
    assert!(derived
        .notes
        .iter()
        .all(|n| !n.contains("bindings or events could not")));
}

#[test]
fn a_difference_the_spec_cannot_adopt_is_listed_with_both_values_and_its_line() {
    let (_dir, solution) = element_solution();
    let name = handmade(&solution, HAND_SPEC, "Acme.Hand_MU");
    let derived = extraction::extract(&solution, &name).unwrap();
    let edited = derived
        .spec
        .replace("CustomClass: acme-title", "CustomClass: other-title");
    assert_ne!(edited, derived.spec);
    write_spec(&solution, &name, &edited);
    let plan = super::super::build::plan(&solution, &name, None, None).unwrap();
    assert!(plan.conflicts());
    let conflicts = plan.report["conflicts"].as_array().unwrap();
    let found = conflicts
        .iter()
        .find(|c| {
            c["path"]
                .as_str()
                .unwrap()
                .ends_with("/Properties/CustomClass")
        })
        .unwrap_or_else(|| panic!("{conflicts:?}"));
    assert_eq!(found["current"], "acme-title");
    assert_eq!(found["generated"], "other-title");
    let line = usize::try_from(found["line"].as_u64().unwrap()).unwrap();
    assert!(
        edited
            .lines()
            .nth(line - 1)
            .unwrap()
            .contains("other-title"),
        "line {line} of\n{edited}"
    );
    assert!(found["file"].as_str().unwrap().ends_with("page.yaml"));
    let message = found["message"].as_str().unwrap();
    assert!(
        message.contains("acme-title") && message.contains("other-title"),
        "{message}"
    );
}

#[test]
fn derived_specs_always_compile_numbers_and_empty_contained_parameters_included() {
    let (_dir, solution) = element_solution();
    let name = handmade(&solution, HAND_SPEC, "Acme.Hand_MU");
    let derived = extraction::extract(&solution, &name).unwrap();
    compile(&solution, "Acme", &derived.spec).unwrap_or_else(|e| panic!("{e}\n{}", derived.spec));
    // A contained mashup derives to `embed`; nothing carries its managed parameter list.
    assert!(derived.spec.contains("embed:"), "{}", derived.spec);
    assert!(
        !derived.spec.contains("MashupParameters"),
        "{}",
        derived.spec
    );
}

#[test]
fn a_page_keeps_its_master() {
    let (_dir, solution) = element_solution();
    let built = compile(&solution, "Acme", HAND_SPEC).unwrap();
    let content: Value = serde_json::from_str(&built.content).unwrap();
    assert_eq!(content["UI"]["Properties"]["Master"], "Acme.Master_MM");
    assert_eq!(content["UI"]["Properties"]["UseMasterTheme"], true);
    assert_eq!(content["mashupType"], "mashup");
}

#[test]
fn spec_errors_become_json_objects_with_file_line_and_column() {
    let object =
        super::super::error_object("src/Acme.Page_MU/page.yaml:12:7: unknown property Foo");
    assert_eq!(object["error"]["file"], "src/Acme.Page_MU/page.yaml");
    assert_eq!(object["error"]["line"], 12);
    assert_eq!(object["error"]["column"], 7);
    assert_eq!(object["error"]["message"], "unknown property Foo");
    let plain = super::super::error_object("twaco: no mashup Acme.X_MU");
    assert_eq!(plain["error"]["message"], "twaco: no mashup Acme.X_MU");
    // The file a build read is named, not a default.
    let (_dir, solution) = element_solution();
    let spec = solution.root.join("other.spec.yaml");
    std::fs::write(
        &spec,
        HAND_SPEC.replace(
            "props: {DefaultFontSize: 14}",
            "props: {DefaultFontSize: 15}",
        ),
    )
    .unwrap();
    let error = super::super::build::plan(&solution, "Acme.Hand_MU", Some("Acme"), Some(&spec))
        .unwrap_err()
        .to_string();
    assert!(error.contains("other.spec.yaml:"), "{error}");
    assert!(error.contains("DefaultFontSize"), "{error}");
    let line: usize = error
        .split("other.spec.yaml:")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .and_then(|n| n.parse().ok())
        .unwrap();
    assert!(
        std::fs::read_to_string(&spec)
            .unwrap()
            .lines()
            .nth(line - 1)
            .unwrap()
            .contains("DefaultFontSize"),
        "the error points at the property's line, {line}: {error}"
    );
}
