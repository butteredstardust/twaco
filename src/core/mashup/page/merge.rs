use serde_json::{Map, Value};
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct Report {
    pub kept: Vec<String>,
    pub conflicts: Vec<String>,
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
            report.kept.push(path.into());
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
    c.cloned()
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

/// Adoption may retain extra current keys and widgets, but every generated value must exist.
pub(crate) fn adopts(c: &Value, g: &Value) -> bool {
    match (c, g) {
        (Value::Object(c), Value::Object(g)) => g
            .iter()
            .all(|(k, v)| c.get(k).is_some_and(|c| adopts(c, v))),
        (Value::Array(c), Value::Array(g)) if g.iter().all(|v| key(v).is_some()) => {
            let wanted: Vec<_> = g.iter().filter_map(key).collect();
            let actual: Vec<_> = c
                .iter()
                .filter_map(key)
                .filter(|id| wanted.contains(id))
                .collect();
            actual == wanted
                && g.iter().all(|v| {
                    c.iter()
                        .find(|cv| key(cv) == key(v))
                        .is_some_and(|cv| adopts(cv, v))
                })
        }
        _ => c == g,
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
