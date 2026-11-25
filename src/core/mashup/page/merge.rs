use serde_json::{Map, Value};
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct Report {
    pub kept: Vec<String>,
    pub conflicts: Vec<String>,
    /// The current and generated value at a conflict path, where there is one to show.
    pub sides: std::collections::BTreeMap<String, (Option<Value>, Option<Value>)>,
}

pub(super) fn display_path(path: &str) -> String {
    if let Some((id, property)) = path
        .strip_prefix("content/widgets/")
        .and_then(|p| p.split_once("/Properties/"))
    {
        return format!("{id}.{}", property.replace('/', "."));
    }
    path.into()
}

pub(crate) fn merge(
    b: Option<&Value>,
    c: Option<&Value>,
    g: Option<&Value>,
    path: &str,
    report: &mut Report,
) -> Option<Value> {
    if g == b {
        if c != b {
            changed_paths(b, c, path, &mut report.kept);
        }
        return c.cloned();
    }
    if c == b || c == g {
        return g.cloned();
    }
    if let (Some(Value::Object(b)), Some(Value::Object(c)), Some(Value::Object(g))) = (b, c, g) {
        let mut out = c.clone();
        let keys: BTreeSet<_> = b.keys().chain(g.keys()).collect();
        for key in keys {
            match merge(
                b.get(key),
                c.get(key),
                g.get(key),
                &format!("{path}/{key}"),
                report,
            ) {
                Some(v) => {
                    out.insert(key.clone(), v);
                }
                None => {
                    out.remove(key);
                }
            }
        }
        return Some(Value::Object(out));
    }
    if let (Some(Value::Array(b)), Some(Value::Array(c)), Some(Value::Array(g))) = (b, c, g) {
        if let Some(value) = arrays(b, c, g, path, report) {
            return Some(value);
        }
    }
    report.conflicts.push(path.into());
    report.sides.insert(path.into(), (c.cloned(), g.cloned()));
    c.cloned()
}

/// Name changed leaves even when the generated subtree is unchanged. Keyed arrays compare by
/// identity; only a widget's child list has runtime layout order.
fn changed_paths(b: Option<&Value>, c: Option<&Value>, path: &str, out: &mut Vec<String>) {
    if b == c {
        return;
    }
    match (b, c) {
        (Some(Value::Object(b)), Some(Value::Object(c))) => {
            let keys: BTreeSet<_> = b.keys().chain(c.keys()).collect();
            for key in keys {
                if !key.starts_with("twaco")
                    && !matches!(key.as_str(), "id_index" | "CustomMashupCss")
                {
                    changed_paths(b.get(key), c.get(key), &format!("{path}/{key}"), out);
                }
            }
        }
        (Some(Value::Array(b)), Some(Value::Array(c)))
            if b.iter().chain(c).all(|v| key(v).is_some()) =>
        {
            let keyed = |items: &[Value]| {
                items
                    .iter()
                    .map(|v| (key(v).expect("key"), v.clone()))
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            let (before, current) = (keyed(b), keyed(c));
            let keys: BTreeSet<_> = before.keys().chain(current.keys()).collect();
            for key in keys {
                changed_paths(
                    before.get(key),
                    current.get(key),
                    &format!("{path}/{key}"),
                    out,
                );
            }
            if path.ends_with("/Widgets")
                && b.iter().map(key).collect::<Vec<_>>() != c.iter().map(key).collect::<Vec<_>>()
            {
                out.push(format!("{path}/order"));
            }
        }
        _ => out.push(path.into()),
    }
}

pub(super) fn key(value: &Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        return Some(s.into());
    }
    value
        .pointer("/Properties/Id")
        .or_else(|| value.get("Id"))
        .or_else(|| value.get("Name"))
        .or_else(|| value.get("name"))
        .or_else(|| value.get("PropertyName"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn arrays(b: &[Value], c: &[Value], g: &[Value], path: &str, report: &mut Report) -> Option<Value> {
    let keys = |items: &[Value]| -> Option<Vec<String>> {
        let keys: Vec<_> = items.iter().map(key).collect::<Option<_>>()?;
        (keys.iter().collect::<BTreeSet<_>>().len() == keys.len()).then_some(keys)
    };
    let (bk, ck, gk) = (keys(b)?, keys(c)?, keys(g)?);
    // Compare order independently of additions and deletions.
    let common: BTreeSet<_> = bk
        .iter()
        .filter(|id| ck.contains(id) && gk.contains(id))
        .collect();
    let order = |keys: &[String]| {
        keys.iter()
            .filter(|id| common.contains(id))
            .cloned()
            .collect::<Vec<_>>()
    };
    let (bo, co, go) = (order(&bk), order(&ck), order(&gk));
    if co != bo && go != bo && co != go {
        report.conflicts.push(format!("{path}/order"));
    }
    let mut merged = Map::new();
    let all: BTreeSet<_> = bk.iter().chain(ck.iter()).chain(gk.iter()).collect();
    for id in all {
        let get = |items: &[Value], ids: &[String]| {
            ids.iter().position(|k| k == id).map(|i| items[i].clone())
        };
        let (bv, cv, gv) = (get(b, &bk), get(c, &ck), get(g, &gk));
        if let Some(v) = merge(
            bv.as_ref(),
            cv.as_ref(),
            gv.as_ref(),
            &format!("{path}/{id}"),
            report,
        ) {
            merged.insert(id.clone(), v);
        }
    }
    let mut result_order = if go != bo { gk.clone() } else { ck.clone() };
    let extra = if go != bo { &ck } else { &gk };
    for (i, id) in extra.iter().enumerate() {
        if result_order.contains(id) {
            continue;
        }
        let at = extra[i + 1..]
            .iter()
            .find_map(|next| result_order.iter().position(|k| k == next))
            .unwrap_or(result_order.len());
        result_order.insert(at, id.clone());
    }
    Some(Value::Array(
        result_order
            .iter()
            .filter_map(|id| merged.remove(id))
            .collect(),
    ))
}

/// A generated value the current mashup does not have.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Difference {
    /// A pointer in which widgets and other keyed items are named by id.
    pub path: String,
    pub current: Option<Value>,
    pub generated: Value,
}

/// Every place `g` asks for something `c` lacks or has differently. Adoption may retain extra
/// current keys and widgets, but every generated value must exist.
pub(crate) fn differences(c: &Value, g: &Value) -> Vec<Difference> {
    let mut out = Vec::new();
    collect(Some(c), g, "", &mut out);
    out
}

fn collect(c: Option<&Value>, g: &Value, path: &str, out: &mut Vec<Difference>) {
    match (c, g) {
        (Some(Value::Object(c)), Value::Object(g)) => {
            for (k, v) in g {
                // twaco's own markers describe how a page was built, not what it is.
                if k.starts_with("twaco") {
                    continue;
                }
                let at = format!("{path}/{}", k.replace('~', "~0").replace('/', "~1"));
                collect(c.get(k), v, &at, out);
            }
        }
        (Some(Value::Array(c)), Value::Array(g)) if g.iter().all(|v| key(v).is_some()) => {
            let wanted: Vec<_> = g.iter().filter_map(key).collect();
            for v in g {
                let id = key(v).expect("keyed");
                let found = c.iter().find(|cv| key(cv).as_deref() == Some(id.as_str()));
                collect(found, v, &format!("{path}/{id}"), out);
            }
            let actual: Vec<_> = c
                .iter()
                .filter_map(key)
                .filter(|id| wanted.contains(id))
                .collect();
            if actual.len() == wanted.len() && actual != wanted {
                out.push(Difference {
                    path: format!("{path}/order"),
                    current: Some(Value::from(actual)),
                    generated: Value::from(wanted),
                });
            }
        }
        (Some(c), g) if c == g => {}
        (c, g) => out.push(Difference {
            path: path.into(),
            current: c.cloned(),
            generated: g.clone(),
        }),
    }
}

pub(super) fn content(b: &Value, c: &Value, g: &Value, report: &mut Report) -> Value {
    fn flatten(value: &Value) -> Value {
        fn widget(mut w: Value, all: &mut Map<String, Value>) -> Value {
            let Some(id) = w
                .pointer("/Properties/Id")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return w;
            };
            if let Some(children) = w.get_mut("Widgets").and_then(Value::as_array_mut) {
                for child in children {
                    *child = widget(child.take(), all);
                }
            }
            all.insert(id.clone(), w);
            Value::String(id)
        }
        let mut value = value.clone();
        let mut all = Map::new();
        // `id_index` only counts the ids in use; `content` takes the larger one.
        if let Some(properties) = value
            .pointer_mut("/UI/Properties")
            .and_then(Value::as_object_mut)
        {
            properties.remove("id_index");
        }
        if let Some(ui) = value.get_mut("UI") {
            *ui = widget(ui.take(), &mut all);
        }
        value
            .as_object_mut()
            .expect("content")
            .remove("CustomMashupCss");
        serde_json::json!({"content":value,"widgets":all})
    }
    fn expand(
        value: &Value,
        all: &Map<String, Value>,
        seen: &mut BTreeSet<String>,
        invalid: &mut Vec<String>,
    ) -> Value {
        let Some(id) = value.as_str() else {
            return value.clone();
        };
        let Some(mut widget) = all.get(id).cloned() else {
            invalid.push(id.into());
            return Value::Null;
        };
        if !seen.insert(id.into()) {
            invalid.push(id.into());
            return Value::Null;
        }
        if let Some(children) = widget.get_mut("Widgets").and_then(Value::as_array_mut) {
            for child in children {
                *child = expand(child, all, seen, invalid);
            }
        }
        widget
    }
    let current_css = c.get("CustomMashupCss").cloned();
    let index = |v: &Value| v.pointer("/UI/Properties/id_index").and_then(Value::as_u64);
    let id_index = match (index(b), index(c), index(g)) {
        (base, current, generated) if generated == base => current,
        (base, current, generated) if current == base => generated,
        (_, current, generated) => current.max(generated),
    };
    let (b, c, g) = (flatten(b), flatten(c), flatten(g));
    let mut merged =
        merge(Some(&b), Some(&c), Some(&g), "content", report).expect("content object");
    let all = merged
        .as_object_mut()
        .expect("content object")
        .remove("widgets")
        .expect("widget map");
    let mut merged = merged
        .as_object_mut()
        .expect("merge wrapper")
        .remove("content")
        .expect("content");
    let mut seen = BTreeSet::new();
    let mut invalid = Vec::new();
    merged["UI"] = expand(
        &merged["UI"],
        all.as_object().expect("widgets"),
        &mut seen,
        &mut invalid,
    );
    for id in invalid {
        report.conflicts.push(format!(
            "content/widgets/{id}: missing or repeated widget after merge"
        ));
    }
    if seen.len() != all.as_object().expect("widgets").len() {
        report
            .conflicts
            .push("content/widgets: a deletion or move would orphan widgets".into());
    }
    if let Some(css) = current_css {
        merged["CustomMashupCss"] = css;
    }
    if let Some(index) = id_index {
        merged["UI"]["Properties"]["id_index"] = Value::from(index);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn value_matrix() {
        for (b, c, g, want, conflict, kept) in [
            (Some(1), Some(1), Some(1), Some(1), false, false),
            (Some(1), Some(2), Some(1), Some(2), false, true),
            (Some(1), Some(1), Some(2), Some(2), false, false),
            (Some(1), Some(2), Some(2), Some(2), false, false),
            (Some(1), Some(2), Some(3), Some(2), true, false),
            (Some(1), None, Some(2), None, true, false),
            (Some(1), Some(2), None, Some(2), true, false),
            (Some(1), Some(1), None, None, false, false),
            (Some(1), None, Some(1), None, false, true),
            (None, Some(2), Some(3), Some(2), true, false),
            (None, None, Some(2), Some(2), false, false),
            (None, Some(2), None, Some(2), false, true),
        ] {
            let (b, c, g) = (b.map(Value::from), c.map(Value::from), g.map(Value::from));
            let mut report = Report::default();
            assert_eq!(
                merge(
                    b.as_ref(),
                    c.as_ref(),
                    g.as_ref(),
                    "Acme/widget/Label",
                    &mut report
                ),
                want.map(Value::from)
            );
            assert_eq!(!report.conflicts.is_empty(), conflict);
            assert_eq!(!report.kept.is_empty(), kept);
        }
    }

    #[test]
    fn keyed_widgets_properties_styles_sources_bindings_and_events() {
        let b = json!({"Widgets":[{"Properties":{"Id":"Acme-1","Label":"old","Style":{"color":"red","size":1}}}],"Data":{"Acme":{"Services":[{"Name":"Get","x":1,"y":1}]}},"Events":[{"Id":"Acme-event","x":1,"y":1}],"DataBindings":[{"Id":"Acme-binding","x":1,"y":1}]});
        let mut c = b.clone();
        let mut g = b.clone();
        c["Widgets"][0]["Properties"]["Style"]["color"] = json!("blue");
        g["Widgets"][0]["Properties"]["Label"] = json!("new");
        c["Widgets"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Properties":{"Id":"Acme-extra","Label":"manual"}}));
        for path in ["/Data/Acme/Services/0", "/Events/0", "/DataBindings/0"] {
            c.pointer_mut(path).unwrap()["x"] = json!(2);
            g.pointer_mut(path).unwrap()["y"] = json!(3);
        }
        let mut r = Report::default();
        let merged = merge(Some(&b), Some(&c), Some(&g), "content", &mut r).unwrap();
        assert!(r.conflicts.is_empty());
        assert_eq!(merged["Widgets"][0]["Properties"]["Label"], "new");
        assert_eq!(merged["Widgets"][0]["Properties"]["Style"]["color"], "blue");
        assert_eq!(merged["Widgets"][1], c["Widgets"][1]);
        for path in ["/Data/Acme/Services/0", "/Events/0", "/DataBindings/0"] {
            assert_eq!(
                merged.pointer(path).unwrap(),
                &json!({if path.contains("Services") {"Name"} else {"Id"}: if path.contains("Services") {"Get"} else if path.contains("Events") {"Acme-event"} else {"Acme-binding"},"x":2,"y":3})
            );
        }
    }

    #[test]
    fn order_and_deletion_conflicts() {
        let a = json!({"Id":"Acme-a","x":1});
        let b = json!({"Id":"Acme-b"});
        let c = json!({"Id":"Acme-c"});
        let base = json!([a, b, c]);
        let current = json!([b, a, c]);
        let generated = json!([a, c, b]);
        let mut r = Report::default();
        merge(
            Some(&base),
            Some(&current),
            Some(&generated),
            "Widgets",
            &mut r,
        );
        assert!(r.conflicts.iter().any(|p| p.ends_with("order")));
        let mut edited = base.clone();
        edited[0]["x"] = json!(2);
        let mut r = Report::default();
        merge(
            Some(&base),
            Some(&edited),
            Some(&json!([b, c])),
            "Widgets",
            &mut r,
        );
        assert_eq!(r.conflicts, vec!["Widgets/Acme-a"]);
    }
}

#[cfg(test)]
mod content_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn a_move_and_property_edit_merge_by_widget_identity() {
        let child = json!({"Properties":{"Id":"Acme-label","LabelText":"before"}});
        let b = json!({"UI":{"Properties":{"Id":"mashup-root"},"Widgets":[{"Properties":{"Id":"Acme-left"},"Widgets":[child]},{"Properties":{"Id":"Acme-right"},"Widgets":[]}]}});
        let mut c = b.clone();
        c["UI"]["Widgets"][0]["Widgets"][0]["Properties"]["LabelText"] = json!("handmade");
        let mut g = b.clone();
        g["UI"]["Widgets"][0]["Widgets"] = json!([]);
        g["UI"]["Widgets"][1]["Widgets"] = json!([child]);
        let mut report = Report::default();
        let result = content(&b, &c, &g, &mut report);
        assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);
        assert_eq!(
            result["UI"]["Widgets"][1]["Widgets"][0]["Properties"]["LabelText"],
            "handmade"
        );
    }
    #[test]
    fn deleting_a_parent_with_unmanaged_children_conflicts() {
        let b = json!({"UI":{"Properties":{"Id":"mashup-root"},"Widgets":[{"Properties":{"Id":"Acme-parent"},"Widgets":[]}]}});
        let mut c = b.clone();
        c["UI"]["Widgets"][0]["Widgets"] = json!([{"Properties":{"Id":"Acme-handmade"}}]);
        let mut g = b.clone();
        g["UI"]["Widgets"] = json!([]);
        let mut report = Report::default();
        content(&b, &c, &g, &mut report);
        assert!(!report.conflicts.is_empty());
    }
}
