use super::{
    ids::{Ids, Pins},
    schema::{self, Node, Page, Size},
    Error,
};
use crate::core::{
    config::Solution,
    mashup::{
        authoring::{self, session::Session},
        edit::{Placement, Side},
    },
    types,
};
use serde_json::{json, Value};
use serde_saphyr::Location;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Artifacts {
    pub(crate) warnings: Vec<String>,
    pub(crate) entity: Vec<u8>,
    pub(crate) content: String,
    pub(crate) css: String,
    pub(crate) ids: BTreeMap<String, String>,
    /// Pages the spec generates for its collections, created beside it when absent.
    pub(crate) cells: Vec<GeneratedCell>,
    /// `(node, role, id)` as hashed, before pins renamed anything.
    pub(crate) allocations: Vec<(String, String, String)>,
    /// The pins that applied.
    pub(crate) pins: Pins,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct GeneratedCell {
    pub(crate) name: String,
    /// The page spec that built it; written as the cell's own `page.yaml`.
    pub(crate) spec: String,
    pub(crate) artifacts: Artifacts,
}

pub(crate) fn compile(solution: &Solution, project: &str, text: &str) -> Result<Artifacts, Error> {
    compile_at(solution, project, text, &solution.root)
}

pub(crate) fn compile_at(
    solution: &Solution,
    project: &str,
    text: &str,
    base: &std::path::Path,
) -> Result<Artifacts, Error> {
    compile_pinned(solution, project, text, base, &Pins::default())
}

/// As `compile_at`, with widget and wiring ids kept where the baseline recorded them.
pub(crate) fn compile_pinned(
    solution: &Solution,
    project: &str,
    text: &str,
    base: &std::path::Path,
    pins: &Pins,
) -> Result<Artifacts, Error> {
    super::deep(|| {
        let mut allocator = Ids::new(std::iter::empty());
        let result = compile_nested(solution, project, text, base, pins, &mut allocator);
        let mut diagnostic_ids = allocator.allocated().to_vec();
        diagnostic_ids.extend(pins.widgets.iter().cloned());
        result
            .map_err(|mut error| {
                if let Ok(spec) = schema::parse_at(text, solution, base) {
                    if error.line == spec.page.referenced.line().max(1) {
                        if let Some(location) =
                            diagnostic_location(&error.message, &diagnostic_ids, &spec.page.value)
                        {
                            error.line = location.line().max(1);
                            error.column = location.column().max(1);
                        }
                    }
                }
                error.message = super::ids::diagnostic(&error.message, &diagnostic_ids);
                error
            })
            .map(|mut artifacts| {
                let located = artifacts
                    .warnings
                    .iter()
                    .any(|message| {
                        diagnostic_ids
                            .iter()
                            .any(|(_, _, id)| super::ids::references(message, id))
                    })
                    .then(|| schema::parse_at(text, solution, base).ok())
                    .flatten();
                artifacts.warnings = artifacts
                    .warnings
                    .iter()
                    .map(|message| {
                        let named = super::ids::diagnostic(message, &diagnostic_ids);
                        match located.as_ref().and_then(|spec| {
                            diagnostic_location(message, &diagnostic_ids, &spec.page.value)
                        }) {
                            Some(location) => Error::at(location, named).to_string(),
                            None => named,
                        }
                    })
                    .collect();
                artifacts
            })
    })
}

fn diagnostic_location(
    message: &str,
    allocated: &[(String, String, String)],
    page: &Page,
) -> Option<Location> {
    fn find(nodes: &[Node], id: &str) -> Option<Location> {
        for node in nodes {
            if node.body().id.value == id {
                return Some(node.body().id.referenced);
            }
            for child in descendants(node) {
                if let Some(location) = find(std::slice::from_ref(child), id) {
                    return Some(location);
                }
            }
        }
        None
    }
    allocated.iter().find_map(|(node, _, id)| {
        if !super::ids::references(message, id) {
            return None;
        }
        if let Some(alias) = node.strip_prefix("$data:") {
            page.data.get(alias).map(|source| source.referenced)
        } else {
            page.slots.values().find_map(|slot| find(&slot.value, node))
        }
    })
}

fn compile_nested(
    solution: &Solution,
    project: &str,
    text: &str,
    base: &std::path::Path,
    pins: &Pins,
    allocator: &mut Ids,
) -> Result<Artifacts, Error> {
    let spec = schema::parse_at(text, solution, base)?;
    let page = &spec.page.value;
    if solution.project(project).is_none() {
        return Err(Error::at(
            page.name.referenced,
            format!("unknown project {project}"),
        ));
    }
    let theme = page
        .theme
        .as_ref()
        .map_or("PTC Convergence Theme", |t| t.value.as_str());
    authoring::session::validate_theme(solution, theme).map_err(|e| {
        Error::at(
            page.theme
                .as_ref()
                .map_or(page.name.referenced, |t| t.referenced),
            e,
        )
    })?;
    fn hosts(nodes: &[Node]) -> usize {
        nodes
            .iter()
            .map(|n| {
                usize::from(n.widget().ok() == Some("pagemashupcontainer"))
                    + descendants(n)
                        .map(|c| hosts(std::slice::from_ref(c)))
                        .sum::<usize>()
            })
            .sum()
    }
    let count: usize = page.slots.values().map(|s| hosts(&s.value)).sum();
    if count != usize::from(page.is_master()) {
        return Err(Error::at(
            spec.page.referenced,
            if page.is_master() {
                "master needs exactly one page_host where pages render; add it to page.slots"
            } else {
                "page_host needs page.kind: master; ordinary pages cannot host pages"
            },
        ));
    }
    let mut session = Session::new_kind(
        &page.name.value,
        project,
        &page.template.value,
        theme,
        page.is_master(),
    )
    .map_err(|e| Error::at(page.template.referenced, e))?;
    spec.resolved_tokens
        .apply_page(&mut session.document)
        .map_err(|e| Error::at(page.name.referenced, e))?;
    session.tokens = Some(spec.resolved_tokens.clone());
    session.token_source = spec
        .tokens
        .as_ref()
        .map(|t| (t.value.clone(), base.to_path_buf()));
    for (name, base_type) in &page.parameters {
        if matches!(&base_type.value, schema::Parameter::Long(details) if details.base_type == "INFOTABLE" && details.data_shape.is_none())
        {
            return Err(Error::at(
                base_type.referenced,
                format!("INFOTABLE parameter {name} needs data_shape"),
            ));
        }
        let declaration = base_type.value.declaration();
        // `TYPE`, `TYPE:DIRECTION`, `INFOTABLE:DataShape` or `INFOTABLE:DataShape:DIRECTION`;
        // a parameter with no direction is an input.
        let segments = declaration.split(':').count();
        let text = if segments
            > if declaration.starts_with("INFOTABLE:") {
                2
            } else {
                1
            } {
            format!("{name}:{declaration}")
        } else {
            format!("{name}:{declaration}:IN")
        };
        session
            .add_parameter_described(
                &text,
                match &base_type.value {
                    schema::Parameter::Long(details) => {
                        details.description.as_deref().unwrap_or("")
                    }
                    schema::Parameter::Short(_) => "",
                },
            )
            .map_err(|e| Error::at(base_type.referenced, e))?;
        if let schema::Parameter::Long(details) = &base_type.value {
            if let Some(default) = &details.default {
                session
                    .document
                    .root_properties_mut()
                    .insert(name.clone(), default.clone());
            }
        }
    }
    if let Some(layout) = &page.layout {
        if layout.value != "as-built" {
            return Err(Error::at(layout.referenced, "layout must be as-built"));
        }
        // No marker: the document is laid out by the classic table, as the commands do.
        session.document.root_mut().remove("twacoPageRules");
        session.document.set_verbatim(true);
    }
    // A claimed root takes its id before nodes are placed, so a node can keep the id the template's
    // center had.
    if let Some(id) = page.root.as_ref().and_then(|r| r.value.id.as_ref()) {
        let path = session
            .document
            .find("center")
            .map_err(|e| Error::at(id.referenced, e))?
            .path;
        session
            .document
            .set_property(&path, "Id", json!(id.value.clone()));
    }
    *allocator = Ids::new(session.document.widgets().into_iter().map(|w| w.id));
    fn preserve_ids(node: &Node, allocator: &mut Ids) {
        if let Some(id) = &node.body().widget_id {
            allocator.preserve(&node.body().id.value, &id.value);
        } else {
            let id = &node.body().id.value;
            if !id.contains("--")
                && id.rsplit_once('-').is_some_and(|(_, suffix)| {
                    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
                })
            {
                allocator.preserve(id, id);
            }
        }
        for tab in &node.body().tabs {
            if let Some(id) = &tab.widget_id {
                allocator.preserve(
                    &format!(
                        "{}--tab-container-{}",
                        node.body().id.value,
                        tab.label.value
                    ),
                    &id.value,
                );
            }
        }
        for child in descendants(node) {
            preserve_ids(child, allocator);
        }
    }
    for node in page.slots.values().flat_map(|s| &s.value) {
        preserve_ids(node, allocator);
    }
    let mut ids = BTreeMap::new();
    let mut cells = vec![];
    if let Some(title) = &page.title {
        let parent = match session.document.find("title") {
            Ok(slot) => slot.id,
            Err(_) => {
                session
                    .document
                    .find("center")
                    .map_err(|e| Error::at(title.referenced, e))?
                    .id
            }
        };
        let old = session
            .document
            .widgets()
            .into_iter()
            .map(|w| w.id)
            .collect();
        let id = session
            .add(
                solution,
                &authoring::AddRequest {
                    mashup: page.name.value.clone(),
                    widget: "ptcslabel".into(),
                    inputs: vec![],
                    placement: Some(Placement::In(parent)),
                    label: Some(title.value.clone()),
                    properties: vec![("LabelType".into(), "header".into())],
                },
            )
            .map_err(|e| Error::at(title.referenced, e))?;
        stabilize(
            &mut session,
            allocator,
            &old,
            &id,
            "$page",
            title.referenced,
        )?;
    }
    for (slot, nodes) in &page.slots {
        let parent = session
            .document
            .find(slot)
            .map_err(|e| Error::at(nodes.referenced, e))?
            .id;
        let mut previous = None;
        for node in &nodes.value {
            let added = add(
                solution,
                &mut session,
                allocator,
                &mut ids,
                page,
                node,
                (&parent, previous.as_deref()),
            )?;
            if !is_function(node)? {
                previous = Some(added);
            }
        }
    }
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    for (alias, source) in entity_first(page) {
        let source = &source.value;
        if let Some(kind) = &source.session {
            if source.service.is_some() || source.expression.is_some() {
                return Err(Error::at(
                    kind.referenced,
                    "session cannot also declare a service or expression",
                ));
            }
            authoring::ensure_session_source(&mut session.document, &kind.value)
                .map_err(|e| Error::at(kind.referenced, e))?;
        }
        let Some(service_spec) = &source.service else {
            continue;
        };
        if alias.is_empty() || !alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::at(
                service_spec.referenced,
                "data name must contain letters, digits or _",
            ));
        }
        let (entity, service) =
            authoring::session::source_target(&model, platform.as_ref(), &service_spec.value)
                .map_err(|e| Error::at(service_spec.referenced, e))?;
        let refresh = source
            .on
            .as_ref()
            .filter(|on| on.value.starts_with("every "))
            .map(|on| {
                on.value
                    .strip_prefix("every ")
                    .and_then(|s| s.strip_suffix('s'))
                    .and_then(|s| s.parse::<u32>().ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| {
                        Error::at(on.referenced, "refresh must be every Ns with N above zero")
                    })
            })
            .transpose()?
            .or(source.refresh.as_ref().map(|r| r.value).filter(|n| *n > 0));
        let params = source
            .params
            .iter()
            .filter(|(_, v)| binding(&v.value).is_none())
            .chain(source.fixed.iter())
            .map(|(k, v)| format!("{k}={}", scalar(&v.value)))
            .collect();
        // A dynamic source's entity is bound with the call when it comes from a parameter or a
        // widget; one that comes from another call is bound once every call exists.
        let entity_from = match &source.entity {
            Some(entity) => {
                let from = binding(&entity.value).ok_or_else(|| {
                    Error::at(
                        entity.referenced,
                        "entity must be {bind: param.Name} or {bind: node.Property}",
                    )
                })?;
                Some(address(page, &ids, from, entity.referenced)?)
                    .filter(|a| a.starts_with("mashup:") || a.starts_with("ui:"))
            }
            None => None,
        };
        session
            .source_as(
                solution,
                &authoring::SourceAddRequest {
                    mashup: page.name.value.clone(),
                    entity,
                    service,
                    dynamic: source.entity.is_some(),
                    on_load: source.on.as_ref().is_some_and(|on| on.value == "load"),
                    alias: Some(alias.clone()),
                    entity_from,
                    refresh,
                    cache: false,
                    cache_keys: vec![],
                    cache_strategy: None,
                    max_result_sets: None,
                    param_values: params,
                },
                source.entity_type.as_ref().map(|kind| kind.value.as_str()),
            )
            .map_err(|e| Error::at(service_spec.referenced, e))?;
    }
    add_expressions(
        solution,
        &mut session,
        &model,
        platform.as_ref(),
        page,
        allocator,
        &mut ids,
    )?;
    // Static initial rows can coexist with a bound Data property. Install that binding before
    // checking the controls and events it creates, then restore its normal declaration order.
    let mut menu_data = BTreeMap::new();
    for (index, binding) in page.bindings.iter().enumerate() {
        let binding = &binding.value;
        let target = binding
            .to
            .value
            .strip_prefix("ui:")
            .unwrap_or(&binding.to.value);
        let Some(node) = target.strip_suffix(".Data") else {
            continue;
        };
        let Some(id) = ids.get(node) else {
            continue;
        };
        let found = session
            .document
            .find_exact(id)
            .map_err(|e| Error::at(binding.to.referenced, e))?;
        if found.type_id != "ptcsmenubutton"
            || session.document.at(&found.path)["Properties"]["ItemConfigurationMode"]
                != "widgetProperties"
        {
            continue;
        }
        let before = session.document.bindings_mut().len();
        let from = address(page, &ids, &binding.from.value, binding.from.referenced)?;
        session
            .bind(
                solution,
                &model,
                platform.as_ref(),
                &from,
                &format!("ui:{id}.Data"),
            )
            .map_err(|e| Error::at(binding.to.referenced, e))?;
        let made = session.document.bindings_mut();
        menu_data.insert(
            index,
            (made.len() > before).then(|| {
                made.last().expect("new binding")["Id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            }),
        );
    }
    wire_nodes(
        solution,
        project,
        &mut session,
        &model,
        platform.as_ref(),
        page,
        allocator,
        &mut ids,
        &mut cells,
    )?;
    for (alias, source) in &page.data {
        let source = &source.value;
        if source.expression.is_some() {
            continue;
        }
        if let (Some(entity), Some(service)) = (&source.entity, &source.service) {
            let from = address(
                page,
                &ids,
                binding(&entity.value).unwrap_or_default(),
                entity.referenced,
            )?;
            if !(from.starts_with("mashup:") || from.starts_with("ui:")) {
                let (shape, _) =
                    authoring::session::source_target(&model, platform.as_ref(), &service.value)
                        .map_err(|e| Error::at(service.referenced, e))?;
                session
                    .bind(
                        solution,
                        &model,
                        platform.as_ref(),
                        &from,
                        &format!("data:{shape}[EntityName][as {alias}]"),
                    )
                    .map_err(|e| Error::at(entity.referenced, e))?;
            }
        }
        for (param, value) in &source.params {
            if let Some(from) = binding(&value.value) {
                let from = address(page, &ids, from, value.referenced)?;
                let to = format!("{}.{param}", call(page, alias));
                session
                    .bind(solution, &model, platform.as_ref(), &from, &to)
                    .map_err(|e| Error::at(value.referenced, e))?;
            }
        }
        if let Some(on) = source
            .on
            .as_ref()
            .filter(|on| on.value != "load" && !on.value.starts_with("every "))
        {
            let from = address(page, &ids, &on.value, on.referenced)?;
            session
                .on(&model, platform.as_ref(), &from, &call(page, alias))
                .map_err(|e| Error::at(on.referenced, e))?;
        }
    }
    for event in &page.events {
        let on = &event.value.on;
        if let Some(navigate) = &event.value.navigate {
            let id = navigate_event(
                solution,
                &mut session,
                page,
                &ids,
                allocator,
                &event.value.on,
                navigate,
            )?;
            if let Some(name) = &navigate.value.id {
                ids.insert(name.value.clone(), id);
            }
            continue;
        }
        let from = address(page, &ids, &on.value, on.referenced)?;
        let to = if let Some(show) = &event.value.show {
            let id = ids.get(&show.value).ok_or_else(|| {
                Error::at(
                    show.referenced,
                    format!("unknown status_message {}", show.value),
                )
            })?;
            let found = session
                .document
                .find_exact(id)
                .map_err(|e| Error::at(show.referenced, e))?;
            if found.type_id != "statusmessage" {
                return Err(Error::at(
                    show.referenced,
                    "show needs a status_message node",
                ));
            }
            format!("ui:{id}.ShowMessage")
        } else {
            let invoke = event.value.invoke.as_ref().expect("validated event");
            if page
                .data
                .get(&invoke.value)
                .is_some_and(|s| s.value.expression.is_some())
            {
                format!("ui:{}.Evaluate", expression_id(page, &invoke.value))
            } else {
                address(page, &ids, &invoke.value, invoke.referenced)?
            }
        };
        session
            .on(&model, platform.as_ref(), &from, &to)
            .map_err(|e| Error::at(on.referenced, e))?;
    }
    for (index, binding) in page.bindings.iter().enumerate() {
        if let Some(made) = menu_data.get(&index) {
            if let Some(id) = made {
                let bindings = session.document.bindings_mut();
                if let Some(at) = bindings.iter().position(|binding| binding["Id"] == *id) {
                    let binding = bindings.remove(at);
                    bindings.push(binding);
                }
            }
            continue;
        }
        let binding = &binding.value;
        let from = address(page, &ids, &binding.from.value, binding.from.referenced)?;
        let to = address(page, &ids, &binding.to.value, binding.to.referenced)?;
        session
            .bind(solution, &model, platform.as_ref(), &from, &to)
            .map_err(|e| Error::at(binding.to.referenced, e))?;
    }
    wire_expressions(
        solution,
        &mut session,
        &model,
        platform.as_ref(),
        page,
        &ids,
    )?;
    for nodes in page.slots.values() {
        for node in &nodes.value {
            apply_size(solution, &mut session, page, &ids, node)?;
        }
    }
    super::elements::finish_layout(&mut session, page, &ids)?;
    apply_root(solution, &mut session, page, &ids)?;
    let applied = apply_pins(
        &mut session,
        &mut ids,
        allocator,
        pins,
        spec.page.referenced,
    )?;
    for nodes in page.slots.values() {
        for node in &nodes.value {
            apply_stored_styles(&mut session, &ids, node)?;
        }
    }
    if let Some(css) = &page.css {
        session.css = read_css(base, css)?;
    }
    session
        .sync()
        .map_err(|e| Error::at(spec.page.referenced, e))?;
    session
        .warnings
        .extend(crate::core::widgets::configuration_warnings(
            session.document.value(),
        ));
    for widget in session.document.widgets() {
        if widget.type_id == "navigationfunction" {
            if let Some(target) = session.document.at(&widget.path)["Properties"]["TargetMashup"]
                .as_str()
                .filter(|name| !name.is_empty())
            {
                if matches!(
                    authoring::load_existing(solution, target),
                    Err(authoring::AuthoringError::Unknown(_))
                ) {
                    session.warnings.push(format!("{target}: target mashup is outside the solution; its parameter contract cannot be checked offline"));
                }
            }
        }
    }
    for widget in session.document.widgets() {
        // A mashup chosen at run time has no parameter contract to check; say which pages carry
        // bindings that were taken as written.
        if widget.type_id == "mashupcontainer"
            && crate::core::widgets::property_bound(session.document.value(), &widget.id, "Name")
            && touches_parameters(session.document.value(), &widget.id)
        {
            session.warnings.push(format!("{}: its mashup is set at run time, so the parameters bound on it cannot be checked offline", widget.id));
        }
    }
    session.warnings.sort();
    session.warnings.dedup();
    Ok(Artifacts {
        warnings: session.warnings,
        entity: session.entity,
        content: session.document.to_content(),
        css: session.css,
        ids,
        cells,
        allocations: allocator.allocated().to_vec(),
        pins: applied,
    })
}

/// The page's master and the template root container's own properties, stated as the spec has
/// them. These come last so nothing the build placed overrides them.
fn apply_root(
    solution: &Solution,
    session: &mut Session,
    page: &Page,
    ids: &BTreeMap<String, String>,
) -> Result<(), Error> {
    for (alias, source) in &page.data {
        let Some(id) = ids.get(&format!("$data:{alias}")) else {
            continue;
        };
        let path = session
            .document
            .find_exact(id)
            .map_err(|e| Error::at(source.referenced, e))?
            .path;
        for (key, value) in &source.value.props {
            super::elements::set_or_remove(&mut session.document, &path, key, &value.value);
        }
    }
    for (key, value) in &page.props {
        if matches!(
            key.as_str(),
            "Id" | "Type" | "Widgets" | "id_index" | "_currentParameterDefs"
        ) {
            return Err(Error::at(
                value.referenced,
                format!("page props cannot set {key}"),
            ));
        }
        super::elements::set_or_remove(&mut session.document, &[], key, &value.value);
    }
    if let Some(styles) = &page.styles {
        if !styles.value.is_object() {
            return Err(Error::at(
                styles.referenced,
                "page.styles must be a map of part to state to property",
            ));
        }
        let id = session.document.root_properties()["Id"]
            .as_str()
            .unwrap_or("mashup-root")
            .to_string();
        session
            .document
            .root_properties_mut()
            .entry("StyleProperties")
            .or_insert_with(|| json!({"data":{}}))["data"][id] = json!({"parts": styles.value});
    }
    if let Some(master) = &page.master {
        session
            .document
            .root_properties_mut()
            .insert("Master".into(), json!(master.value));
    }
    if let Some(flag) = &page.use_master_theme {
        session
            .document
            .root_properties_mut()
            .insert("UseMasterTheme".into(), json!(flag.value));
    }
    let Some(root) = &page.root else {
        return Ok(());
    };
    let path = session
        .document
        .find("center")
        .map_err(|e| Error::at(root.referenced, e))?
        .path;
    if let Some(id) = &root.value.id {
        session
            .document
            .set_property(&path, "Id", json!(id.value.clone()));
    }
    for (key, value) in &root.value.props {
        if matches!(key.as_str(), "Id" | "Type" | "Widgets") {
            return Err(Error::at(
                value.referenced,
                format!("root props cannot set {key}"),
            ));
        }
        // A binding is wired with the others, not stored as a value.
        if binding(&value.value).is_some() {
            continue;
        }
        // An absent property is null here: remove it rather than store null.
        super::elements::set_or_remove(&mut session.document, &path, key, &value.value);
    }
    let id = session.document.at(&path)["Properties"]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if let Some(styles) = &root.value.styles {
        if !styles.value.is_object() {
            return Err(Error::at(
                styles.referenced,
                "root styles must be a map of part to state to property",
            ));
        }
        // The same entry a widget's `styles:` writes, so the root container keeps its look.
        let mut entry = json!({"parts": styles.value});
        if let Some(styler) = crate::core::mashup::authoring::set::styler_name("flexcontainer") {
            entry["widget"] = json!(styler);
        }
        let data = session
            .document
            .root_properties_mut()
            .entry("StyleProperties")
            .or_insert_with(|| json!({}));
        if !data.is_object() {
            *data = json!({});
        }
        let data = data.as_object_mut().expect("object");
        data.entry("meta")
            .or_insert_with(|| json!({"type": "WidgetStyler", "version": "1.0"}));
        data.entry("data")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| Error::at(styles.referenced, "StyleProperties.data is not a map"))?
            .insert(id.clone(), entry);
    }
    if !root.value.style.is_empty() {
        session
            .set(
                solution,
                &authoring::SetRequest {
                    mashup: page.name.value.clone(),
                    widget: id,
                    properties: vec![],
                    style_from: None,
                    styles: root.value.style.iter().map(|s| s.value.clone()).collect(),
                },
            )
            .map_err(|e| Error::at(root.referenced, e))?;
    }
    Ok(())
}

/// Rename the widgets the baseline pinned back to the ids the mashup already had, then give the
/// bindings and events their recorded ids. A pin whose node or role no longer exists is ignored.
fn apply_pins(
    session: &mut Session,
    ids: &mut BTreeMap<String, String>,
    allocator: &Ids,
    pins: &Pins,
    at: Location,
) -> Result<Pins, Error> {
    let mut applied = Pins::default();
    let mut mapping = BTreeMap::new();
    for (node, role, target) in &pins.widgets {
        if let Some((_, _, hashed)) = allocator
            .allocated()
            .iter()
            .find(|(n, r, _)| n == node && r == role)
        {
            mapping.insert(hashed.clone(), target.clone());
            applied
                .widgets
                .push((node.clone(), role.clone(), target.clone()));
        }
    }
    if !mapping.is_empty() {
        let mut seen = BTreeSet::new();
        for widget in session.document.widgets() {
            let id = mapping.get(&widget.id).unwrap_or(&widget.id);
            if !seen.insert(id.clone()) {
                return Err(Error::at(
                    at,
                    format!("widget id {id} is pinned twice or collides with another widget; delete src/<mashup>/.page-baseline.json and adopt again"),
                ));
            }
        }
        rename_widgets(session, &mapping, true);
        for id in ids.values_mut() {
            if let Some(renamed) = mapping.get(id.as_str()) {
                *id = renamed.clone();
            }
        }
    }
    let mut keep = |items: &mut [Value]| {
        for item in items {
            let Some(kept) = item["Id"].as_str().and_then(|id| pins.wiring.get(id)) else {
                continue;
            };
            applied.wiring.insert(
                item["Id"].as_str().unwrap_or_default().to_string(),
                kept.clone(),
            );
            item["Id"] = json!(kept);
        }
    };
    keep(session.document.bindings_mut());
    keep(session.document.events_mut());
    if let Some(sections) = session
        .document
        .root_mut()
        .get_mut("Data")
        .and_then(Value::as_object_mut)
    {
        for section in sections.values_mut() {
            keep(std::slice::from_mut(section));
            if let Some(calls) = section["Services"].as_array_mut() {
                keep(calls);
            }
        }
    }
    Ok(applied)
}

/// A raw widget the catalogue cannot construct; its properties and children are carried explicitly.
fn raw_unknown(node: &Node) -> Option<&str> {
    if !matches!(node, Node::Widget(_)) {
        return None;
    }
    let widget = node.widget().ok()?;
    crate::core::widgets::catalog()
        .by_type(widget)
        .is_none()
        .then_some(widget)
}

fn option_text(node: &Node, key: &str, value: &Value) -> String {
    if let (Some(number), Ok(widget)) = (
        value.as_f64().filter(|n| n.fract() == 0.0),
        node.widget().and_then(|w| {
            crate::core::widgets::catalog()
                .resolve(w)
                .map_err(|e| Error::at(serde_saphyr::Location::UNKNOWN, e))
        }),
    ) {
        let pixels = format!("{number}px");
        if widget
            .property(key)
            .is_some_and(|p| p.options.contains(&json!(pixels)))
        {
            return pixels;
        }
    }
    scalar(value)
}

pub(super) fn scalar(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

pub(super) fn binding(value: &Value) -> Option<&str> {
    value
        .as_object()
        .filter(|m| m.len() == 1)?
        .get("bind")?
        .as_str()
}

/// Where an error about one of the node's props points: that prop's own line, when the message
/// names it, else the node's `id:` line.
fn key_location(body: &schema::Body, message: &str, default: Location) -> Location {
    body.props
        .iter()
        .find(|(key, _)| {
            message
                .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
                .any(|word| word == key.as_str())
        })
        .map_or(default, |(_, value)| value.referenced)
}

/// The page's data sources in spec order, except that a source naming its entity comes before the
/// other calls of the same ThingShape: they share one dynamic section whose entity binding the
/// first call creates, and a call added before it has no Thing to run on.
fn entity_first(page: &schema::Page) -> Vec<(&String, &serde_saphyr::Spanned<schema::Source>)> {
    let shape = |source: &schema::Source| {
        source
            .service
            .as_ref()
            .and_then(|s| s.value.rsplit_once('.').map(|(shape, _)| shape.to_string()))
    };
    let items: Vec<_> = page.data.iter().collect();
    let key = |i: usize| -> (usize, u8, usize) {
        let own = &items[i].1.value;
        let Some(name) = shape(own) else {
            return (i, 0, i);
        };
        let group = |j: usize| shape(&items[j].1.value).as_deref() == Some(name.as_str());
        let carries = |j: usize| items[j].1.value.entity.is_some() && group(j);
        if !(0..items.len()).any(carries) {
            return (i, 0, i);
        }
        let first = (0..items.len()).find(|j| group(*j)).unwrap_or(i);
        (first, u8::from(own.entity.is_none()), i)
    };
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|i| key(*i));
    order.into_iter().map(|i| items[i]).collect()
}

fn is_function(node: &Node) -> Result<bool, Error> {
    if matches!(node, Node::Pagination(_)) {
        return Ok(true);
    }
    Ok(crate::core::widgets::catalog()
        .resolve(node.widget()?)
        .is_ok_and(|w| w.function))
}

/// Whether every catalogue widget that defines `name` types it as a number.
pub(super) fn always_numeric(name: &str) -> bool {
    let catalog = crate::core::widgets::catalog();
    let mut seen = false;
    for widget in &catalog.widgets {
        let Some(contract) = widget.property(name) else {
            continue;
        };
        if contract.css_size {
            return false;
        }
        match contract.base_type.as_deref() {
            Some("NUMBER" | "INTEGER" | "LONG") => seen = true,
            None => {}
            Some(_) => return false,
        }
    }
    seen
}

fn add(
    solution: &Solution,
    session: &mut Session,
    allocator: &mut Ids,
    ids: &mut BTreeMap<String, String>,
    page: &Page,
    node: &Node,
    position: (&str, Option<&str>),
) -> Result<String, Error> {
    let body = node.body();
    let loc = body.id.referenced;
    if matches!(node, Node::Pagination(_)) {
        return Ok(String::new());
    }
    let mut props: Vec<_> = body
        .props
        .iter()
        .filter(|(k, v)| {
            binding(&v.value).is_none() && k.as_str() != "DisplayName" && !v.value.is_null()
        })
        .map(|(k, v)| (k.clone(), option_text(node, k, &v.value)))
        .collect();
    let menu_data_bound = node.widget()? == "ptcsmenubutton"
        && (body
            .props
            .get("Data")
            .is_some_and(|value| binding(&value.value).is_some())
            || page.bindings.iter().any(|binding| {
                binding
                    .value
                    .to
                    .value
                    .strip_prefix("ui:")
                    .unwrap_or(&binding.value.to.value)
                    == format!("{}.Data", body.id.value)
            }));
    let configuration = Value::Object(
        body.props
            .iter()
            .map(|(key, value)| (key.clone(), value.value.clone()))
            .collect(),
    );
    let mut dynamic_values = Vec::new();
    if menu_data_bound {
        let widget = crate::core::widgets::catalog()
            .by_type("ptcsmenubutton")
            .expect("menu button");
        for (name, value) in &body.props {
            if binding(&value.value).is_none() {
                if let Some(contract) = crate::core::widgets::dynamic_property(
                    "ptcsmenubutton",
                    &configuration,
                    name,
                    true,
                ) {
                    let mut dynamic = widget.clone();
                    dynamic
                        .properties
                        .get_or_insert_default()
                        .insert(name.clone(), contract);
                    let checked = authoring::property_value(&dynamic, name, value.value.clone())
                        .map_err(|e| Error::at(value.referenced, e))?;
                    dynamic_values.push((name.clone(), checked));
                }
            }
        }
        props.retain(|(name, _)| !dynamic_values.iter().any(|(dynamic, _)| dynamic == name));
    }
    if page.layout.is_some() && body.native.is_none() {
        // An as-built page carries properties the catalogue does not list; they are set after
        // placement, as the page has them.
        if let Ok(widget) = crate::core::widgets::catalog().resolve(node.widget()?) {
            // Carried verbatim, but a property that is numeric on every widget that defines it
            // (Height, Width, Top...) still has to be a number.
            for (key, value) in &body.props {
                // Source mashups do store "" and numeric text; only words are refused.
                let word = value
                    .value
                    .as_str()
                    .is_some_and(|t| !t.trim().is_empty() && t.trim().parse::<f64>().is_err());
                if word && always_numeric(key) {
                    return Err(Error::at(
                        value.referenced,
                        format!("{key} must be a JSON number"),
                    ));
                }
            }
            props.retain(|(k, _)| {
                body.props
                    .get(k)
                    .is_some_and(|v| !v.value.is_object() && !v.value.is_array())
                    && !matches!(
                        k.as_str(),
                        "ResponsiveLayout" | "LastContainer" | "__TypeDisplayName"
                    )
                    && (widget.defaults.contains_key(k)
                        || widget.property(k).is_some()
                        || widget.numbered_default(k).is_some())
            });
        }
    }
    let op = match node {
        Node::PageHeader(_) => Some("page-header"),
        Node::Toolbar(_) => Some("toolbar"),
        Node::KpiRow(_) => Some("page-kpi-row"),
        Node::Card(_) | Node::TrendTile(_) => Some("page-card"),
        _ => None,
    };
    match node {
        Node::Row(_) => props.push(("flex-direction".into(), "row".into())),
        Node::Column(_) => props.push(("flex-direction".into(), "column".into())),
        Node::DateRange(_) => props.push(("DateRange".into(), "true".into())),
        Node::Tabs(_) => {
            props.push(("NumberOfTabs".into(), body.tabs.len().to_string()));
            for (i, tab) in body.tabs.iter().enumerate() {
                props.push((format!("Tab{}Name", i + 1), tab.label.value.clone()));
            }
        }
        _ => {}
    }
    if let Some(hint) = &body.hint {
        props.push(("HintText".into(), hint.clone()));
    }
    if matches!(node, Node::Heading(_)) {
        props.push(("LabelType".into(), "header".into()));
    }
    if matches!(node, Node::Text(_)) {
        props.push(("LabelType".into(), "body".into()));
    }
    let (parent, previous) = position;
    let raw_parent = session
        .document
        .find_exact(parent)
        .map_err(|e| Error::at(loc, e))?
        .type_id
        != "flexcontainer";
    let placement = if page.layout.is_some() || raw_parent {
        Placement::Append(parent.into())
    } else if let Some(anchor) = previous {
        let parent = session
            .document
            .find_exact(parent)
            .map_err(|e| Error::at(loc, e))?;
        if session.document.at(&parent.path)["Properties"]["flex-direction"] == "row" {
            Placement::Beside {
                anchor: anchor.into(),
                side: Side::Right,
            }
        } else {
            Placement::BelowInColumn(anchor.into())
        }
    } else {
        Placement::In(parent.into())
    };
    let old = session
        .document
        .widgets()
        .into_iter()
        .map(|w| w.id)
        .collect();
    let placed = if let Some(child) = &body.native {
        let catalog = crate::core::widgets::catalog()
            .by_type("mashupcontainer")
            .expect("contained widget");
        let (props, settings): (Vec<_>, Vec<_>) = props
            .into_iter()
            .partition(|(k, _)| catalog.defaults.contains_key(k) || catalog.property(k).is_some());
        let id = session
            .contained_with(solution, parent, child, settings)
            .map_err(|e| Error::at(loc, e))?;
        if !props.is_empty() {
            session
                .set(
                    solution,
                    &authoring::SetRequest {
                        mashup: page.name.value.clone(),
                        widget: id.clone(),
                        properties: props
                            .into_iter()
                            .map(|(k, v)| (k, serde_json::from_str(&v).unwrap_or(Value::String(v))))
                            .collect(),
                        style_from: None,
                        styles: vec![],
                    },
                )
                .map_err(|e| Error::at(loc, e))?;
        }
        id
    } else if let Some(widget) = raw_unknown(node) {
        // Raw widgets without a catalogue entry retain their type and stored properties.
        let parent_path = session
            .document
            .find_exact(parent)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let number = session.document.allocate();
        let mut properties = serde_json::Map::new();
        for (key, value) in &body.props {
            if !value.value.is_null() && binding(&value.value).is_none() {
                properties.insert(key.clone(), value.value.clone());
            }
        }
        properties.insert("Id".into(), json!(format!("{widget}-{number}")));
        properties.insert("Type".into(), json!(widget));
        properties
            .entry("DisplayName")
            .or_insert_with(|| json!(format!("{widget}-{number}")));
        let id = format!("{widget}-{number}");
        session.document.push_child(
            &parent_path,
            json!({"Properties": properties, "Widgets": []}),
        );
        id
    } else {
        session
            .add(
                solution,
                &authoring::AddRequest {
                    mashup: page.name.value.clone(),
                    widget: node.widget()?.into(),
                    inputs: body.inputs.clone(),
                    placement: if is_function(node)? {
                        None
                    } else {
                        Some(placement)
                    },
                    label: body
                        .text
                        .as_ref()
                        .and_then(|v| v.value.as_str())
                        .map(str::to_owned),
                    properties: props,
                },
            )
            .map_err(|e| Error::at(key_location(body, &e.to_string(), loc), e))?
    };
    // The configuration must precede binding checks: collections and menu buttons create
    // their dynamic properties from it. As-built properties are also restored after layout.
    if page.layout.is_some() {
        let path = session
            .document
            .find_exact(&placed)
            .map_err(|e| Error::at(loc, e))?
            .path;
        for (key, value) in &body.props {
            if binding(&value.value).is_none() {
                super::elements::set_or_remove(&mut session.document, &path, key, &value.value);
            }
        }
    }
    if !dynamic_values.is_empty() {
        let path = session
            .document
            .find_exact(&placed)
            .map_err(|e| Error::at(loc, e))?
            .path;
        for (key, value) in dynamic_values {
            session.document.set_property(&path, &key, value);
        }
    }
    if let Some(op) = op {
        let context = crate::core::mashup::layout::context::Node::op(
            op,
            crate::core::mashup::layout::Element::Container,
        );
        let path = session
            .document
            .find_exact(&placed)
            .map_err(|e| Error::at(loc, e))?
            .path;
        for (key, value) in session
            .document
            .rules()
            .evaluate(&context.context())
            .patch()
        {
            session.document.set_property(&path, key, value);
        }
    }
    match node {
        Node::Toolbar(_) if body.band.unwrap_or(true) => session.recipe(
            solution,
            authoring::Recipe::ToolbarBand {
                container: placed.clone(),
            },
        ),
        Node::Card(_) if body.align.is_some() => session.recipe(
            solution,
            authoring::Recipe::Section {
                container: placed.clone(),
                title: body.title.clone().expect("validated title"),
                center: body.align.as_ref().is_some_and(|a| a.value == "center"),
            },
        ),
        Node::Card(_) | Node::TrendTile(_) => session.recipe(
            solution,
            authoring::Recipe::Card {
                container: placed.clone(),
                caption: body.title.clone().expect("validated title"),
                value: body
                    .value
                    .as_ref()
                    .filter(|_| matches!(node, Node::Card(_)))
                    .map(|v| {
                        if binding(&v.value).is_some() {
                            String::new()
                        } else {
                            scalar(&v.value)
                        }
                    }),
                status: body.status.as_ref().map(|v| {
                    if binding(&v.value).is_some() {
                        String::new()
                    } else {
                        scalar(&v.value)
                    }
                }),
                kind: body.chart_kind.clone().unwrap_or_else(|| "info".into()),
            },
        ),
        Node::FilterPanel(_) => session
            .template_children(&placed, "filter-panel")
            .and_then(|()| filter_chevron(solution, session, &placed, body.collapsible)),
        Node::SidePanel(_) => session.recipe(
            solution,
            authoring::Recipe::SidePanel {
                container: placed.clone(),
                title: body.title.clone().expect("validated title"),
                info: body.help.clone(),
                tab: body.tab.clone().unwrap_or_else(|| "Filters".into()),
            },
        ),
        _ => Ok(()),
    }
    .map_err(|e| Error::at(loc, e))?;
    if matches!(node, Node::FilterPanel(_)) {
        let root = session
            .document
            .find_exact(&placed)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let title = session
            .document
            .widgets()
            .into_iter()
            .find(|w| w.path.starts_with(&root) && w.display_name == "filter-title")
            .expect("filter title");
        session.document.set_property(
            &title.path,
            "LabelText",
            json!(body.title.as_deref().unwrap_or("Filter")),
        );
    }
    if (matches!(node, Node::Card(_)) && body.align.is_none()) || matches!(node, Node::TrendTile(_))
    {
        // Kit card titles use the regular label size, not the recipe's sub-header.
        let path = session
            .document
            .find_exact(&placed)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let caption = session
            .document
            .widgets()
            .into_iter()
            .find(|w| w.path.starts_with(&path) && w.type_id == "ptcslabel")
            .map(|w| w.path);
        if let Some(caption) = caption {
            let context = crate::core::mashup::layout::context::Node::op(
                "page-card-caption",
                crate::core::mashup::layout::Element::Container,
            );
            for (key, value) in session
                .document
                .rules()
                .evaluate(&context.context())
                .patch()
            {
                session.document.set_property(&caption, key, value);
            }
        }
    }
    let id = stabilize(session, allocator, &old, &placed, &body.id.value, loc)?;
    if matches!(node, Node::FilterPanel(_)) {
        // The recipe's title label takes the id and address the generated `<id>--title` heading
        // had, so a page rebuilt from an older spec keeps its title widget and references to it.
        let root = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        if let Some(title) = session
            .document
            .widgets()
            .into_iter()
            .find(|w| w.path.starts_with(&root) && w.display_name == "filter-title")
        {
            let node = format!("{}--title", body.id.value);
            let stable = allocator
                .widget("ptcslabel", &node, "widget")
                .map_err(|e| Error::at(loc, e))?;
            rename_widgets(
                session,
                &BTreeMap::from([(title.id.clone(), stable.clone())]),
                false,
            );
            ids.insert(node, stable);
        }
    }
    if !body.binding_types.is_empty() {
        if crate::core::widgets::catalog()
            .by_type(node.widget()?)
            .is_some_and(|w| w.has_contract())
        {
            return Err(Error::at(
                loc,
                "binding_types is only for widgets without a catalogue contract",
            ));
        }
        for (property, base) in &body.binding_types {
            session
                .document
                .binding_types
                .insert((id.clone(), property.clone()), base.value.clone());
        }
    }
    ids.insert(body.id.value.clone(), id.clone());
    apply_styles(solution, session, page, &id, body)?;
    if let Some(size) = &body.value_size {
        let path = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let value = session
            .document
            .widgets()
            .into_iter()
            .filter(|w| w.path.starts_with(&path) && w.type_id == "ptcslabel")
            .nth(1)
            .map(|w| w.id);
        if let Some(value) = value {
            session
                .set(
                    solution,
                    &authoring::SetRequest {
                        mashup: page.name.value.clone(),
                        widget: value,
                        properties: vec![],
                        style_from: None,
                        styles: vec![format!("label:font-size={size}")],
                    },
                )
                .map_err(|e| Error::at(loc, e))?;
        }
    }
    if matches!(node, Node::FilterPanel(_)) {
        let root = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let slot = |name: &str, session: &Session| {
            session
                .document
                .widgets()
                .into_iter()
                .find(|w| w.path.starts_with(&root) && w.display_name == name)
                .expect("template slot")
                .id
        };
        let filters = slot("filters", session);
        let header = slot("filter-header", session);
        let has_title =
            body.title.is_none() && matches!(body.fields.first(), Some(Node::Heading(_)));
        if has_title {
            let header_path = session
                .document
                .find_exact(&header)
                .map_err(|e| Error::at(loc, e))?
                .path;
            session.document.at_mut(&header_path)["Widgets"] = json!([]);
            add_children(
                solution,
                session,
                allocator,
                ids,
                page,
                &body.fields[..1],
                &header,
            )?;
        }
        let footer = slot("filter-footer", session);
        let content = slot("content", session);
        add_children(
            solution,
            session,
            allocator,
            ids,
            page,
            &body.fields[usize::from(has_title)..],
            &filters,
        )?;
        for child in &body.children {
            let target = if body
                .footer
                .iter()
                .any(|f| child.body().id.value == format!("{}--{}", body.id.value, f.value))
            {
                &footer
            } else {
                &content
            };
            add_children(
                solution,
                session,
                allocator,
                ids,
                page,
                std::slice::from_ref(child),
                target,
            )?;
        }
    } else if matches!(node, Node::SidePanel(_)) {
        // The recipe's one tab holds the panel's content.
        let root = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let tabset = session
            .document
            .widgets()
            .into_iter()
            .find(|w| w.path.starts_with(&root) && w.type_id == "ptcstabset")
            .expect("side panel tab strip");
        let content = session.document.at(&tabset.path)["Widgets"][0]["Properties"]["Id"]
            .as_str()
            .expect("side panel tab content")
            .to_string();
        add_children(
            solution,
            session,
            allocator,
            ids,
            page,
            &body.children,
            &content,
        )?;
    } else if matches!(node, Node::Card(_)) && body.align.is_some() {
        // The section recipe's last container is the body its content goes in.
        let root = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        let content = session.document.at(&root)["Widgets"]
            .as_array()
            .and_then(|items| items.last())
            .and_then(|item| item["Properties"]["Id"].as_str())
            .expect("section body")
            .to_string();
        add_children(
            solution,
            session,
            allocator,
            ids,
            page,
            &body.children,
            &content,
        )?;
    } else if matches!(node, Node::Tabs(_)) {
        let path = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?
            .path;
        for (i, tab) in body.tabs.iter().enumerate() {
            let tab_path = [path.as_slice(), &[i]].concat();
            let parent = session.document.at(&tab_path)["Properties"]["Id"]
                .as_str()
                .expect("tab id")
                .to_string();
            let old: BTreeSet<_> = session
                .document
                .widgets()
                .into_iter()
                .map(|w| w.id)
                .collect();
            let role = format!("{}--tab-{}", body.id.value, tab.label.value);
            for (key, value) in &tab.props {
                super::elements::set_or_remove(&mut session.document, &tab_path, key, &value.value);
            }
            if page.layout.is_some() {
                ids.insert(role.clone(), parent.clone());
            }
            if let Some(mashup) = &tab.mashup {
                let held = session
                    .contained(solution, &parent, &mashup.value)
                    .map_err(|e| Error::at(mashup.referenced, e))?;
                stabilize(session, allocator, &old, &held, &role, tab.label.referenced)?;
            } else if page.layout.is_some() {
                // As built, a tab's content sits in the tab's own container.
                add_children(
                    solution,
                    session,
                    allocator,
                    ids,
                    page,
                    tab.content.as_deref().unwrap_or_default(),
                    &parent,
                )?;
            } else {
                session
                    .template_children(&parent, "tab-content")
                    .map_err(|e| Error::at(tab.label.referenced, e))?;
                let body_slot = session
                    .document
                    .widgets()
                    .into_iter()
                    .find(|w| w.path.starts_with(&tab_path) && w.display_name == "body")
                    .expect("tab body")
                    .id;
                let stable = stabilize(
                    session,
                    allocator,
                    &old,
                    &body_slot,
                    &role,
                    tab.label.referenced,
                )?;
                // Inline specs supply their own toolbar; do not reserve a second band.
                let toolbar = session
                    .document
                    .widgets()
                    .into_iter()
                    .find(|w| w.path.starts_with(&tab_path) && w.display_name == "toolbar")
                    .expect("tab toolbar");
                let index = *toolbar.path.last().expect("toolbar parent");
                session.document.at_mut(&tab_path)["Widgets"]
                    .as_array_mut()
                    .expect("tab children")
                    .remove(index);
                add_children(
                    solution,
                    session,
                    allocator,
                    ids,
                    page,
                    tab.content.as_deref().unwrap_or_default(),
                    &stable,
                )?;
            }
        }
    } else {
        add_children(solution, session, allocator, ids, page, &body.children, &id)?;
    }
    Ok(id)
}

/// The filter column's chevron: `collapsible: true` is the collapsible recipe with the tab inside
/// the column's top-right edge, as the kits draw it; `false` removes the template's chevron.
fn filter_chevron(
    solution: &Solution,
    session: &mut Session,
    panel: &str,
    collapsible: Option<bool>,
) -> Result<(), authoring::AuthoringError> {
    let Some(collapsible) = collapsible else {
        return Ok(());
    };
    let root = session
        .document
        .find_exact(panel)
        .map_err(authoring::AuthoringError::Edit)?
        .path;
    let column = session
        .document
        .widgets()
        .into_iter()
        .find(|w| w.path.starts_with(&root) && w.display_name == "filter-column")
        .expect("filter-panel template column");
    if collapsible {
        session.recipe(
            solution,
            authoring::Recipe::Collapsible {
                container: column.id,
                toggle: None,
                add_toggle_in: None,
                tab: "inside".into(),
                collapsed: false,
            },
        )
    } else {
        for (key, value) in [
            ("EnableExpandCollapse", json!(false)),
            ("ShowExpandCollapseTab", json!(false)),
        ] {
            session.document.set_property(&column.path, key, value);
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn add_children(
    solution: &Solution,
    session: &mut Session,
    allocator: &mut Ids,
    ids: &mut BTreeMap<String, String>,
    page: &Page,
    children: &[Node],
    parent: &str,
) -> Result<(), Error> {
    let mut previous = None;
    for child in children {
        let added = add(
            solution,
            session,
            allocator,
            ids,
            page,
            child,
            (parent, previous.as_deref()),
        )?;
        if !is_function(child)? {
            previous = Some(added);
        }
    }
    Ok(())
}

pub(super) fn descendants(node: &Node) -> impl Iterator<Item = &Node> {
    let b = node.body();
    b.children
        .iter()
        .chain(&b.fields)
        .chain(b.tabs.iter().flat_map(|t| t.content.iter().flatten()))
}

/// `page.css`: the file's text is the `custom.css` sidecar, which ends in one newline.
fn read_css(base: &std::path::Path, css: &serde_saphyr::Spanned<String>) -> Result<String, Error> {
    let relative = std::path::Path::new(&css.value);
    if css.value.is_empty() || relative.is_absolute() {
        return Err(Error::at(
            css.referenced,
            "page.css must be a file path relative to the spec",
        ));
    }
    let path = base.join(relative);
    let bytes = std::fs::read(&path).map_err(|e| {
        Error::at(
            css.referenced,
            format!("page.css: cannot read {}: {e}", css.value),
        )
    })?;
    let mut text = String::from_utf8(bytes).map_err(|_| {
        Error::at(
            css.referenced,
            format!("page.css: {} is not UTF-8 text", css.value),
        )
    })?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// A node's `styles:` replace the stored style parts of its widget, as written.
fn apply_stored_styles(
    session: &mut Session,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    if let Some(styles) = &body.styles {
        if !styles.value.is_object() {
            return Err(Error::at(
                styles.referenced,
                "styles must be a map of part to state to property",
            ));
        }
        let Some(id) = ids.get(&body.id.value) else {
            return Err(Error::at(
                styles.referenced,
                "styles: this node builds no widget of its own",
            ));
        };
        // Composer records the widget's styler beside its parts; write it the same way.
        let mut entry = json!({"parts": styles.value});
        if let Some(styler) = session
            .document
            .widgets()
            .into_iter()
            .find(|w| &w.id == id)
            .and_then(|w| crate::core::mashup::authoring::set::styler_name(&w.type_id))
        {
            entry["widget"] = json!(styler);
        }
        let data = session
            .document
            .root_properties_mut()
            .entry("StyleProperties")
            .or_insert_with(|| json!({}));
        if !data.is_object() {
            *data = json!({});
        }
        data.as_object_mut()
            .expect("object")
            .entry("meta")
            .or_insert_with(|| json!({"type": "WidgetStyler", "version": "1.0"}));
        data.as_object_mut()
            .expect("object")
            .entry("data")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| Error::at(styles.referenced, "StyleProperties.data is not a map"))?
            .insert(id.clone(), entry);
    }
    for child in descendants(node) {
        apply_stored_styles(session, ids, child)?;
    }
    Ok(())
}

fn styles(body: &schema::Body) -> Vec<String> {
    let mut styles: Vec<_> = body.style.iter().map(|s| s.value.clone()).collect();
    if let Some(surface) = body.surface {
        styles.push(format!(
            "background={}",
            if surface { "page-card" } else { "transparent" }
        ));
    }
    styles
}

fn apply_styles(
    solution: &Solution,
    session: &mut Session,
    page: &Page,
    id: &str,
    body: &schema::Body,
) -> Result<(), Error> {
    let styles = styles(body);
    if !styles.is_empty() {
        session
            .set(
                solution,
                &authoring::SetRequest {
                    mashup: page.name.value.clone(),
                    widget: id.into(),
                    properties: vec![],
                    style_from: None,
                    styles,
                },
            )
            .map_err(|e| Error::at(body.id.referenced, e))?;
    }
    Ok(())
}

fn apply_size(
    solution: &Solution,
    session: &mut Session,
    page: &Page,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    for child in descendants(node) {
        apply_size(solution, session, page, ids, child)?;
    }
    if matches!(node, Node::Toolbar(_)) && body.band.unwrap_or(true) {
        session
            .recipe(
                solution,
                authoring::Recipe::ToolbarBand {
                    container: ids[&body.id.value].clone(),
                },
            )
            .map_err(|e| Error::at(body.id.referenced, e))?;
        apply_styles(solution, session, page, &ids[&body.id.value], body)?;
    }
    let id = ids[&body.id.value].clone();
    if matches!(node, Node::Spacer(_)) {
        session
            .set(
                solution,
                &authoring::SetRequest {
                    mashup: page.name.value.clone(),
                    widget: id.clone(),
                    properties: session
                        .document
                        .rules()
                        .evaluate(
                            &crate::core::mashup::layout::context::Node::op(
                                "page-spacer",
                                crate::core::mashup::layout::Element::Container,
                            )
                            .context(),
                        )
                        .patch()
                        .into_iter()
                        .map(|(k, v)| (k.into(), v))
                        .collect(),
                    style_from: None,
                    styles: vec![],
                },
            )
            .map_err(|e| Error::at(body.id.referenced, e))?;
    }
    let loc = body.id.referenced;
    let mut properties = vec![];
    if let Some(size) = &body.size {
        let widget = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(loc, e))?;
        let container = widget.type_id == "flexcontainer";
        let sizing_id = if container {
            id.clone()
        } else {
            let parent_path = &widget.path[..widget.path.len() - 1];
            session.document.at(parent_path)["Properties"]["Id"]
                .as_str()
                .expect("parent id")
                .to_string()
        };
        let mut sizing = vec![];
        match size.value {
            Size::Fixed(px) if px > 0 && container => {
                let parent_path = &widget.path[..widget.path.len() - 1];
                let dimension =
                    if session.document.at(parent_path)["Properties"]["flex-direction"] == "row" {
                        "width"
                    } else {
                        "height"
                    };
                sizing.extend([
                    ("flex-size".into(), json!("fixed")),
                    ("flex-grow".into(), json!(0)),
                    (format!("flex-min-{dimension}"), json!(format!("{px}px"))),
                    (format!("flex-max-{dimension}"), json!(format!("{px}px"))),
                ]);
            }
            Size::Fixed(px) if px > 0 => properties.push(("Height".into(), json!(px))),
            Size::Fixed(_) => {
                return Err(Error::at(size.referenced, "fixed size must be above zero"))
            }
            Size::Sm | Size::Md | Size::Lg | Size::Xl | Size::Display => {}
            Size::Fit => sizing.push(("flex-grow".into(), json!(0))),
            Size::Fill => sizing.extend([
                ("flex-size".into(), json!("range")),
                ("flex-max-height".into(), json!("")),
                ("flex-grow".into(), json!(1)),
                ("flex-shrink".into(), json!(1)),
                ("flex-basis".into(), json!("0px")),
            ]),
        }
        if container && matches!(size.value, Size::Fill) {
            sizing.push(("flex-min-height".into(), json!("0px")));
        }
        if !sizing.is_empty() {
            session
                .set(
                    solution,
                    &authoring::SetRequest {
                        mashup: page.name.value.clone(),
                        widget: sizing_id,
                        properties: sizing,
                        style_from: None,
                        styles: vec![],
                    },
                )
                .map_err(|e| Error::at(size.referenced, e))?;
        }
    }
    if !properties.is_empty() {
        session
            .set(
                solution,
                &authoring::SetRequest {
                    mashup: page.name.value.clone(),
                    widget: id.clone(),
                    properties,
                    style_from: None,
                    styles: vec![],
                },
            )
            .map_err(|e| Error::at(loc, e))?;
    }
    Ok(())
}

fn stabilize(
    session: &mut Session,
    allocator: &mut Ids,
    old: &BTreeSet<String>,
    placed: &str,
    node: &str,
    loc: Location,
) -> Result<String, Error> {
    let mut mapping = BTreeMap::new();
    let new = session
        .document
        .widgets()
        .into_iter()
        .filter(|w| !old.contains(&w.id))
        .collect::<Vec<_>>();
    let main = new.iter().find(|w| w.id == placed).expect("placed widget");
    // A derived spec names nodes by their source widget ids (`ptcsbutton-13`, or a hand-made
    // `wrapper-1`); keep such ids so id-based custom CSS and test automation still match. Ids
    // only need to be unique, and the allocator reserves the number either way.
    // Generated sub-nodes (`menu--item-1-1`, `cards--row-2`) are named by twaco, not by a
    // source mashup, and always contain `--`; they keep allocated ids.
    let explicit = !node.contains("--")
        && node.rsplit_once('-').is_some_and(|(_, suffix)| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        });
    let id = if let Some(id) = allocator.preserved(node) {
        allocator.explicit_as(node, "widget", &id)
    } else if explicit {
        allocator.explicit(node, "widget")
    } else {
        allocator.widget(&main.type_id, node, "widget")
    }
    .map_err(|e| Error::at(loc, format!("{e} (assigning {node})")))?;
    mapping.insert(placed.to_string(), id.clone());
    // Wrapper roles name the enclosed widget, so unrelated siblings cannot move their ids.
    for widget in new.iter().rev().filter(|w| w.id != placed) {
        let child = session.document.at(&widget.path)["Widgets"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v["Properties"]["Id"].as_str())
            .unwrap_or(placed);
        let child = mapping.get(child).map_or(child, String::as_str);
        let role = if let Some((_, tab)) = widget.display_name.rsplit_once(" #") {
            let properties = &session.document.at(&main.path)["Properties"];
            let label = tab
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|n| properties["Items"].get(n))
                .and_then(|item| item["name"].as_str())
                .or_else(|| properties[format!("Tab{tab}Name")].as_str())
                .unwrap_or(tab);
            format!("tab:{label}")
        } else {
            if crate::core::widgets::catalog()
                .by_type(&widget.type_id)
                .is_some_and(|w| w.function)
            {
                format!(
                    "function:{}",
                    session.document.at(&widget.path)["Properties"]["Expression"]
                        .as_str()
                        .unwrap_or(&widget.type_id)
                )
            } else if session.document.at(&widget.path)["twacoTemplateSlot"] == true
                && !widget.display_name.starts_with("divider-")
            {
                format!("slot:{}", widget.display_name)
            } else if widget.display_name.starts_with("divider-") {
                let parent = &widget.path[..widget.path.len() - 1];
                let next = widget.path.last().expect("divider index") + 1;
                let next_slot = session.document.at(parent)["Widgets"]
                    .as_array()
                    .and_then(|items| items.get(next))
                    .and_then(|v| v["Properties"]["DisplayName"].as_str())
                    .unwrap_or("end");
                let parent_slot = session.document.at(parent)["Properties"]["DisplayName"]
                    .as_str()
                    .filter(|_| session.document.at(parent)["twacoTemplateSlot"] == true)
                    .unwrap_or("root");
                format!("divider:{parent_slot}:before:{next_slot}")
            } else if !widget.path.starts_with(&main.path) {
                format!("wrapper:{child}")
            } else if widget.path.len() > main.path.len() + 1
                && session.document.at(&widget.path)["Widgets"]
                    .as_array()
                    .is_none_or(Vec::is_empty)
            {
                // Leaves nested in a recipe's own containers are told apart by position.
                format!(
                    "leaf:{}",
                    widget.path[main.path.len()..]
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(".")
                )
            } else if widget.type_id == "ptcslabel" && widget.path.len() == main.path.len() + 1 {
                format!(
                    "label:{}",
                    session.document.at(&widget.path)["Properties"]["LabelType"]
                        .as_str()
                        .unwrap_or("body")
                )
            } else {
                format!("wrapper:{child}")
            }
        };
        let pinned = role
            .strip_prefix("tab:")
            .and_then(|label| allocator.preserved(&format!("{node}--tab-container-{label}")));
        let stable = if let Some(pinned) = pinned {
            allocator.explicit_as(node, &role, &pinned)
        } else {
            allocator.widget(&widget.type_id, node, &role)
        }
        .map_err(|e| Error::at(loc, format!("{e} (assigning {node}, role {role})")))?;
        mapping.insert(widget.id.clone(), stable);
    }
    rename_widgets(session, &mapping, false);
    Ok(id)
}

/// Give widgets new ids and follow them through wiring, styles and pagination links.
fn rename_widgets(session: &mut Session, mapping: &BTreeMap<String, String>, tab_names: bool) {
    for widget in session.document.widgets() {
        let Some(stable) = mapping.get(&widget.id) else {
            continue;
        };
        let number = stable.rsplit('-').next().expect("numeric suffix");
        let old_number = widget.id.rsplit('-').next().expect("numeric suffix");
        let properties = session.document.at_mut(&widget.path)["Properties"]
            .as_object_mut()
            .expect("widget properties");
        properties.insert("Id".into(), json!(stable));
        if let Some(display) = properties.get("DisplayName").and_then(Value::as_str) {
            if let Some(prefix) = display.strip_suffix(&format!("-{old_number}")) {
                properties.insert("DisplayName".into(), json!(format!("{prefix}-{number}")));
            }
        }
    }
    // A tab's container is named after its tab set's number.
    let numbers: Vec<(String, String)> = mapping
        .iter()
        .filter(|_| tab_names)
        .filter_map(|(old, new)| {
            Some((
                format!("-{} #", old.rsplit('-').next()?),
                format!("-{} #", new.rsplit('-').next()?),
            ))
        })
        .collect();
    for widget in session.document.widgets() {
        let properties = session.document.at_mut(&widget.path)["Properties"]
            .as_object_mut()
            .expect("widget properties");
        let Some(display) = properties.get("DisplayName").and_then(Value::as_str) else {
            continue;
        };
        for (old, new) in &numbers {
            if display.contains(old.as_str()) {
                let renamed = display.replacen(old.as_str(), new, 1);
                properties.insert("DisplayName".into(), json!(renamed));
                break;
            }
        }
    }
    fn remap(v: &mut Value, mapping: &BTreeMap<String, String>) {
        match v {
            Value::Object(m) => {
                for (key, value) in m.iter_mut() {
                    if matches!(
                        key.as_str(),
                        "SourceId"
                            | "TargetId"
                            | "EventTriggerId"
                            | "EventHandlerId"
                            | "twacoPaginationFor"
                    ) {
                        if let Some(id) = value.as_str().and_then(|s| mapping.get(s)) {
                            *value = json!(id);
                        }
                    } else {
                        remap(value, mapping);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    remap(item, mapping);
                }
            }
            _ => {}
        }
    }
    for value in session.document.root_mut().values_mut() {
        remap(value, mapping);
    }
    fn binding_ids(items: &mut [Value], mapping: &BTreeMap<String, String>) {
        for item in items {
            let uses_generated = ["SourceId", "TargetId", "EventTriggerId", "EventHandlerId"]
                .iter()
                .any(|k| {
                    item[*k]
                        .as_str()
                        .is_some_and(|id| mapping.values().any(|stable| stable == id))
                });
            if uses_generated {
                let mut semantic = item.clone();
                semantic
                    .as_object_mut()
                    .expect("binding object")
                    .remove("Id");
                item["Id"] = json!(super::ids::uuid(&format!("remapped:{semantic}")));
            }
        }
    }
    binding_ids(session.document.bindings_mut(), mapping);
    binding_ids(session.document.events_mut(), mapping);
    if let Some(styles) = session
        .document
        .root_properties_mut()
        .get_mut("StyleProperties")
        .and_then(|v| v.get_mut("data"))
        .and_then(Value::as_object_mut)
    {
        let before = std::mem::take(styles);
        for (key, value) in before {
            styles.insert(mapping.get(&key).cloned().unwrap_or(key), value);
        }
    }
    session
        .document
        .root_properties_mut()
        .insert("id_index".into(), json!(0));
    session.document.reserve_ids();
}

pub(super) fn call(page: &Page, alias: &str) -> String {
    if let Some((source, field)) = alias.split_once('.') {
        return format!("{}.{field}", call(page, source));
    }
    if page.data[alias].value.expression.is_some() {
        format!("ui:{}.Output", expression_id(page, alias))
    } else if let Some(kind) = &page.data[alias].value.session {
        format!("session:{}", kind.value)
    } else {
        format!(
            "data:{}[as {alias}]",
            page.data[alias]
                .value
                .service
                .as_ref()
                .expect("service")
                .value
        )
    }
}

fn expression_id(page: &Page, alias: &str) -> String {
    if let Some(id) = &page.data[alias].value.widget_id {
        return id.value.clone();
    }
    Ids::new(std::iter::empty())
        .widget("expression2", &format!("$data:{alias}"), "widget")
        .expect("empty allocator")
}

#[allow(clippy::too_many_arguments)]
fn add_expressions(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    allocator: &mut Ids,
    ids: &mut BTreeMap<String, String>,
) -> Result<(), Error> {
    for (alias, source) in &page.data {
        let source = &source.value;
        let Some(expression) = &source.expression else {
            continue;
        };
        let at = expression.referenced;
        if let Some(id) = &source.widget_id {
            allocator.preserve(&format!("$data:{alias}"), &id.value);
        }
        if alias.is_empty() || !alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::at(at, "data name must contain letters, digits or _"));
        }
        let output = source.output.as_ref().expect("validated output");
        let mut inputs = vec![];
        for (name, input) in &source.inputs {
            let base = match &input.value {
                schema::ExpressionInput::Typed(typed) => typed.base_type.clone(),
                schema::ExpressionInput::Reference(reference) => {
                    let alias = reference.split('.').next().unwrap_or(reference);
                    if let Some(other) = page
                        .data
                        .get(alias)
                        .filter(|s| s.value.expression.is_some())
                    {
                        other
                            .value
                            .output
                            .as_ref()
                            .expect("validated output")
                            .value
                            .clone()
                    } else {
                        let from = if let Some((grid, field)) = reference.split_once(".selected.") {
                            let data = find_grid(page, grid).ok_or_else(|| {
                                Error::at(
                                    input.referenced,
                                    "selection input needs a grid with data",
                                )
                            })?;
                            format!("{}.{field}", call(page, data))
                        } else {
                            address(page, ids, reference, input.referenced)?
                        };
                        session
                            .source_type(solution, model, platform, &from)
                            .map_err(|e| Error::at(input.referenced, e))?
                    }
                }
            };
            inputs.push(format!("{name}:{base}"));
        }
        authoring::validate_expression_type(&output.value)
            .map_err(|e| Error::at(output.referenced, e))?;
        let script = &expression.value;
        let script = if script.contains(';') || script.contains("\n") {
            script.clone()
        } else {
            format!("result = ({script});")
        };
        let mut properties = vec![
            ("Expression".into(), script),
            ("OutputBaseType".into(), output.value.clone()),
            (
                "AutoEvaluate".into(),
                source.auto_evaluate.unwrap_or(true).to_string(),
            ),
            (
                "FireOnFirstValue".into(),
                source.fire_on_first_value.unwrap_or(false).to_string(),
            ),
        ];
        if let Some(change) = &source.data_change {
            properties.push(("DataChangeType".into(), change.value.clone()));
        }
        let old = session
            .document
            .widgets()
            .into_iter()
            .map(|w| w.id)
            .collect();
        let placed = session
            .add(
                solution,
                &authoring::AddRequest {
                    mashup: page.name.value.clone(),
                    widget: "expression2".into(),
                    inputs,
                    placement: None,
                    label: None,
                    properties,
                },
            )
            .map_err(|e| Error::at(at, e))?;
        let id = stabilize(
            session,
            allocator,
            &old,
            &placed,
            &format!("$data:{alias}"),
            at,
        )?;
        ids.insert(format!("$data:{alias}"), id);
    }
    Ok(())
}

fn wire_expressions(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    ids: &BTreeMap<String, String>,
) -> Result<(), Error> {
    for (alias, source) in &page.data {
        let source = &source.value;
        if source.expression.is_none() {
            continue;
        }
        let id = expression_id(page, alias);
        for (name, input) in &source.inputs {
            let value = match &input.value {
                schema::ExpressionInput::Reference(reference) => Some(json!({"bind": reference})),
                schema::ExpressionInput::Typed(typed) => typed.value.clone(),
            };
            if let Some(value) = value {
                if let Some(from) = binding(&value) {
                    let from = address(page, ids, from, input.referenced)?;
                    session
                        .bind(solution, model, platform, &from, &format!("ui:{id}.{name}"))
                        .map_err(|e| Error::at(input.referenced, e))?;
                } else {
                    session
                        .set(
                            solution,
                            &authoring::SetRequest {
                                mashup: page.name.value.clone(),
                                widget: id.clone(),
                                properties: vec![(name.clone(), value)],
                                style_from: None,
                                styles: vec![],
                            },
                        )
                        .map_err(|e| Error::at(input.referenced, e))?;
                }
            }
        }
        if let Some(on) = &source.on {
            let from = address(page, ids, &on.value, on.referenced)?;
            session
                .on(model, platform, &from, &format!("ui:{id}.Evaluate"))
                .map_err(|e| Error::at(on.referenced, e))?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn navigate_event(
    solution: &Solution,
    session: &mut Session,
    page: &Page,
    ids: &BTreeMap<String, String>,
    allocator: &mut Ids,
    on: &serde_saphyr::Spanned<String>,
    navigate: &serde_saphyr::Spanned<schema::Navigate>,
) -> Result<String, Error> {
    let nav = &navigate.value;
    let mode = nav.mode.as_ref().map_or("page", |m| m.value.as_str());
    let window = match mode {
        "page" | "replace" => "replace",
        "new_tab" | "new" => "new",
        "popup" => "popup",
        "modal-popup" => "modal-popup",
        _ => {
            return Err(Error::at(
                nav.mode.as_ref().expect("mode").referenced,
                "navigate mode is page, new_tab, popup or modal-popup",
            ))
        }
    };
    let from = address(page, ids, &on.value, on.referenced)?;
    let pass = nav
        .params
        .iter()
        .filter_map(|(name, value)| binding(&value.value).map(|from| (name, value, from)))
        .map(|(name, value, from)| Ok((name.clone(), address(page, ids, from, value.referenced)?)))
        .collect::<Result<Vec<_>, Error>>()?;
    let old: BTreeSet<_> = session
        .document
        .widgets()
        .into_iter()
        .map(|w| w.id)
        .collect();
    session
        .recipe(
            solution,
            authoring::Recipe::Popup {
                trigger: from,
                target: nav.mashup.value.clone(),
                window: window.into(),
                size: if nav.width.is_some() || nav.height.is_some() {
                    Some((nav.width.unwrap_or(0), nav.height.unwrap_or(0)))
                } else {
                    None
                },
                title: nav.title.clone(),
                pass,
                refresh: None,
            },
        )
        .map_err(|e| Error::at(navigate.referenced, e))?;
    let placed = session
        .document
        .widgets()
        .into_iter()
        .find(|w| !old.contains(&w.id) && w.type_id == "navigationfunction")
        .expect("navigation recipe")
        .id;
    let id = stabilize(
        session,
        allocator,
        &old,
        &placed,
        nav.id
            .as_ref()
            .map(|s| s.value.clone())
            .unwrap_or_else(|| format!("$navigate:{}:{}", on.value, nav.mashup.value))
            .as_str(),
        navigate.referenced,
    )?;
    if matches!(
        authoring::load_existing(solution, &nav.mashup.value),
        Err(authoring::AuthoringError::Unknown(_))
    ) {
        let path = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(navigate.referenced, e))?
            .path;
        for (name, value) in &nav.params {
            if binding(&value.value).is_some() {
                continue;
            }
            let base = match &value.value {
                Value::Bool(_) => "BOOLEAN",
                Value::Number(_) => "NUMBER",
                Value::Object(_) | Value::Array(_) => "JSON",
                _ => "STRING",
            };
            let entry = json!({"ParameterName": name, "BaseType": base, "ParmDef": {"aspects": {"bindingDirection": "IN"}}});
            session.document.at_mut(&path)["Properties"]["MashupParameters"]
                .as_array_mut()
                .expect("navigation cache")
                .push(entry);
        }
    }
    let properties = nav
        .params
        .iter()
        .chain(&nav.props)
        .filter(|(_, v)| binding(&v.value).is_none())
        .map(|(k, v)| (k.clone(), v.value.clone()))
        .collect::<Vec<_>>();
    // The name is not one `set` may change; an as-built page keeps the source's.
    let (names, properties): (Vec<_>, Vec<_>) = properties
        .into_iter()
        .partition(|(k, _)| k == "DisplayName");
    if let Some((_, name)) = names.into_iter().next() {
        let path = session
            .document
            .find_exact(&id)
            .map_err(|e| Error::at(navigate.referenced, e))?
            .path;
        session.document.set_property(&path, "DisplayName", name);
    }
    if !properties.is_empty() {
        session
            .set(
                solution,
                &authoring::SetRequest {
                    mashup: page.name.value.clone(),
                    widget: id.clone(),
                    properties,
                    style_from: None,
                    styles: vec![],
                },
            )
            .map_err(|e| Error::at(navigate.referenced, e))?;
    }
    Ok(id)
}

fn wire_sparklines(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let b = node.body();
    if let Some(sparkline) = &b.sparkline {
        let loc = sparkline.referenced;
        let spark = &sparkline.value;
        let data = spark
            .data
            .as_deref()
            .or_else(|| {
                b.value
                    .as_ref()
                    .and_then(|v| binding(&v.value))
                    .and_then(|v| v.split_once('.').map(|v| v.0))
            })
            .filter(|data| page.data.contains_key(*data))
            .ok_or_else(|| Error::at(loc, "sparkline needs data naming a page data source"))?;
        for field in [&spark.x, &spark.field] {
            let base = session
                .source_type(
                    solution,
                    model,
                    platform,
                    &format!("{}.{field}", call(page, data)),
                )
                .map_err(|e| Error::at(loc, e))?;
            if field == &spark.field && !["NUMBER", "INTEGER", "LONG"].contains(&base.as_str()) {
                return Err(Error::at(
                    loc,
                    format!("sparkline field {field} must be numeric"),
                ));
            }
        }
        session
            .bind(
                solution,
                model,
                platform,
                &call(page, data),
                &format!("ui:{}.Data", ids[&format!("{}--sparkline", b.id.value)]),
            )
            .map_err(|e| Error::at(loc, e))?;
    }
    for child in descendants(node) {
        wire_sparklines(solution, session, model, platform, page, ids, child)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn wire_selections(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    allocator: &mut Ids,
    ids: &mut BTreeMap<String, String>,
) -> Result<(), Error> {
    fn collect(node: &Node, refs: &mut BTreeMap<String, Location>) {
        let b = node.body();
        for value in b
            .props
            .values()
            .chain(b.value.iter())
            .chain(b.status.iter())
        {
            if let Some(text) = binding(&value.value).filter(|s| s.contains(".selected.")) {
                refs.insert(text.to_string(), value.referenced);
            }
        }
        for child in descendants(node) {
            collect(child, refs);
        }
    }
    let mut refs = BTreeMap::new();
    for binding in &page.bindings {
        let from = &binding.value.from;
        if from.value.contains(".selected.") {
            refs.insert(from.value.clone(), from.referenced);
        }
    }
    for node in page.slots.values().flat_map(|s| &s.value) {
        collect(node, &mut refs);
    }
    for source in page.data.values() {
        for value in source.value.params.values() {
            if let Some(text) = binding(&value.value).filter(|s| s.contains(".selected.")) {
                refs.insert(text.to_string(), value.referenced);
            }
        }
    }
    for source in page.data.values() {
        for input in source.value.inputs.values() {
            let reference = match &input.value {
                schema::ExpressionInput::Reference(s) => Some(s.as_str()),
                schema::ExpressionInput::Typed(t) => t.value.as_ref().and_then(binding),
            };
            if let Some(reference) = reference.filter(|s| s.contains(".selected.")) {
                refs.insert(reference.into(), input.referenced);
            }
        }
    }
    for event in &page.events {
        if let Some(navigate) = &event.value.navigate {
            for value in navigate.value.params.values() {
                if let Some(reference) = binding(&value.value).filter(|s| s.contains(".selected."))
                {
                    refs.insert(reference.into(), value.referenced);
                }
            }
        }
    }
    for (text, loc) in refs {
        let (grid, field) = text.split_once(".selected.").expect("selection");
        let data = find_grid(page, grid)
            .ok_or_else(|| Error::at(loc, format!("{text} needs a grid with data")))?;
        if field.is_empty() || field.contains('.') {
            return Err(Error::at(loc, format!("{text} needs one field")));
        }
        let grid_body = find_grid_body(page, grid).expect("grid with data");
        let path = session
            .document
            .find_exact(&ids[grid])
            .map_err(|e| Error::at(loc, e))?
            .path;
        if !grid_body.props.contains_key("DefaultSelectedRows") {
            session.document.set_property(
                &path,
                "DefaultSelectedRows",
                json!(if grid_body.auto_select.unwrap_or(true) {
                    "1"
                } else {
                    ""
                }),
            );
        }
        let base = session
            .source_type(
                solution,
                model,
                platform,
                &format!("{}.{field}", call(page, data)),
            )
            .map_err(|e| Error::at(loc, e))?;
        let old = session
            .document
            .widgets()
            .into_iter()
            .map(|w| w.id)
            .collect();
        let field_key = serde_json::to_string(field).expect("field string");
        let expression =
            format!("var selected = rows && (rows.rows || rows); result = selected && selected.length ? selected[0][{field_key}] : undefined;");
        let placed = session
            .add(
                solution,
                &authoring::AddRequest {
                    mashup: page.name.value.clone(),
                    widget: "expression2".into(),
                    inputs: vec!["rows:INFOTABLE".into()],
                    placement: None,
                    label: None,
                    properties: vec![
                        ("Expression".into(), expression),
                        ("OutputBaseType".into(), base),
                        ("AutoEvaluate".into(), "true".into()),
                    ],
                },
            )
            .map_err(|e| Error::at(loc, e))?;
        let id = stabilize(
            session,
            allocator,
            &old,
            &placed,
            &format!("{grid}--selected-{field}"),
            loc,
        )?;
        session
            .bind(
                solution,
                model,
                platform,
                &format!("ui:{}.SelectedRows", ids[grid]),
                &format!("ui:{id}.rows"),
            )
            .map_err(|e| Error::at(loc, e))?;
        ids.insert(text, id);
    }
    Ok(())
}

pub(super) fn address(
    page: &Page,
    ids: &BTreeMap<String, String>,
    text: &str,
    loc: Location,
) -> Result<String, Error> {
    if text == "load" {
        return Ok("mashup:Loaded".into());
    }
    if let Some(service) = text.strip_prefix("mashup.") {
        if !service.is_empty() && !service.contains('.') {
            return Ok(format!("mashup:{service}"));
        }
    }
    if let Some(name) = text
        .strip_prefix("param.")
        .filter(|name| page.parameters.contains_key(*name))
    {
        return Ok(format!("mashup:{name}"));
    }
    if page.data.contains_key(text) {
        return Ok(call(page, text));
    }
    // `page.<property>`: a property of the mashup root widget, unless a node or data source is
    // itself named `page`.
    if let Some(property) = text.strip_prefix("page.") {
        if !property.is_empty()
            && !property.contains('.')
            && !ids.contains_key("page")
            && !page.data.contains_key("page")
        {
            return Ok(format!("ui:mashup-root.{property}"));
        }
    }
    if text.contains(".selected.") {
        return ids
            .get(text)
            .map(|id| format!("ui:{id}.Output"))
            .ok_or_else(|| Error::at(loc, format!("unknown selection reference {text}")));
    }
    if let Some((node, property)) = text.split_once('.') {
        if let Some(source) = page.data.get(node).filter(|s| s.value.session.is_some()) {
            let base = source.value.fields.get(property).ok_or_else(|| {
                Error::at(
                    loc,
                    format!("session source {node} needs fields.{property}: TYPE"),
                )
            })?;
            return Ok(format!(
                "session:{}.{}:{}",
                source.value.session.as_ref().expect("session").value,
                property,
                base.value
            ));
        }
        if let Some(id) = ids.get(node) {
            return Ok(format!("ui:{id}.{property}"));
        }
        if page
            .data
            .get(node)
            .is_some_and(|s| s.value.expression.is_some())
        {
            return Ok(format!("ui:{}.{}", expression_id(page, node), property));
        }
        if page.data.contains_key(node) {
            return Ok(if property == "SelectedRows" {
                format!("{}[SelectedRows]", call(page, node))
            } else if let Some(field) = property.strip_prefix("SelectedRows.") {
                format!("{}[SelectedRows].{field}", call(page, node))
            } else {
                format!("{}.{property}", call(page, node))
            });
        }
    }
    Err(Error::at(loc, format!("unknown reference {text}")))
}

fn find_grid_body<'a>(page: &'a Page, name: &str) -> Option<&'a schema::Body> {
    fn find<'a>(node: &'a Node, name: &str) -> Option<&'a schema::Body> {
        if matches!(node, Node::Grid(_)) && node.body().id.value == name {
            return Some(node.body());
        }
        descendants(node).find_map(|n| find(n, name))
    }
    page.slots
        .values()
        .flat_map(|s| &s.value)
        .find_map(|n| find(n, name))
}

/// A list node with data, by id.
pub(super) fn find_list<'a>(page: &'a Page, name: &str) -> Option<&'a schema::Body> {
    fn find<'a>(node: &'a Node, name: &str) -> Option<&'a schema::Body> {
        if matches!(node, Node::List(_)) && node.body().id.value == name {
            return Some(node.body());
        }
        descendants(node).find_map(|n| find(n, name))
    }
    page.slots
        .values()
        .flat_map(|s| &s.value)
        .find_map(|n| find(n, name))
        .filter(|b| b.data.is_some())
}

/// What a `data:` field names as a binding source: a declared call, or the selected items of
/// another list (`<list>.selected`).
pub(super) fn data_address(page: &Page, ids: &BTreeMap<String, String>, text: &str) -> String {
    match text.strip_suffix(".selected") {
        Some(list) => format!("ui:{}.SelectedItems", ids[list]),
        None => call(page, text),
    }
}

pub(super) fn find_grid<'a>(page: &'a Page, name: &str) -> Option<&'a str> {
    find_grid_body(page, name)?
        .data
        .as_ref()
        .map(|d| d.value.as_str())
}

fn wire(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    ids: &mut BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    if matches!(node, Node::Pagination(_)) {
        return Ok(());
    }
    let id = &ids[&body.id.value];
    if let Some(auto) = body.auto_select {
        if auto && !body.props.contains_key("RowSelection") {
            let path = session
                .document
                .find_exact(id)
                .map_err(|e| Error::at(body.id.referenced, e))?
                .path;
            session
                .document
                .set_property(&path, "RowSelection", json!("single"));
        }
        if !body.props.contains_key("DefaultSelectedRows") {
            let path = session
                .document
                .find_exact(id)
                .map_err(|e| Error::at(body.id.referenced, e))?
                .path;
            session.document.set_property(
                &path,
                "DefaultSelectedRows",
                json!(if auto { "1" } else { "" }),
            );
        }
    }
    super::elements::wire(solution, session, model, platform, page, ids, node)?;
    if let Some(data) = body.data.as_ref().filter(|_| matches!(node, Node::Grid(_))) {
        let from = call(page, &data.value);
        session
            .bind(solution, model, platform, &from, &format!("ui:{id}.Data"))
            .map_err(|e| Error::at(data.referenced, e))?;
        if !body.columns.is_empty() {
            for column in &body.columns {
                if column
                    .value
                    .editor
                    .as_deref()
                    .is_some_and(|editor| !matches!(editor, "textfield" | "textarea"))
                {
                    return Err(Error::at(
                        column.referenced,
                        "grid column editor must be textfield or textarea",
                    ));
                }
            }
            let statuses: Vec<_> = body
                .columns
                .iter()
                .filter_map(|c| {
                    c.value
                        .status
                        .as_ref()
                        .map(|s| (c.value.field.clone(), s.clone()))
                })
                .collect();
            let statuses = if statuses.is_empty() {
                vec![None]
            } else {
                statuses.into_iter().map(Some).collect()
            };
            for status in statuses {
                session
                    .recipe(
                        solution,
                        authoring::Recipe::Columns {
                            auto_select_first: false,
                            widget: id.clone(),
                            data: None,
                            columns: body
                                .columns
                                .iter()
                                .map(|c| {
                                    format!(
                                        "{}:{}:{}:{}",
                                        c.value.field,
                                        c.value.title.as_deref().unwrap_or(&c.value.field),
                                        c.value.width.map_or(String::new(), |w| w.to_string()),
                                        c.value.format.as_deref().unwrap_or("")
                                    )
                                })
                                .collect(),
                            date_format: None,
                            editable: body
                                .columns
                                .iter()
                                .filter(|c| c.value.editable)
                                .map(|c| c.value.field.clone())
                                .collect(),
                            column_editors: body
                                .columns
                                .iter()
                                .filter_map(|c| {
                                    c.value
                                        .editor
                                        .as_ref()
                                        .map(|editor| (c.value.field.clone(), editor.clone()))
                                })
                                .collect(),
                            preserve_whitespace: body
                                .columns
                                .iter()
                                .filter(|c| c.value.preserve_whitespace)
                                .map(|c| c.value.field.clone())
                                .collect(),
                            status,
                        },
                    )
                    .map_err(|e| Error::at(body.columns[0].referenced, e))?;
            }
        }
    } else if !body.columns.is_empty() {
        return Err(Error::at(
            body.columns[0].referenced,
            "grid columns need data",
        ));
    }
    for child in descendants(node) {
        wire(solution, session, model, platform, page, ids, child)?;
    }
    Ok(())
}

/// `enabled_when: <grid>.selected`: the kit's expression from the grid's `RowSelected` into the
/// button's `Disabled`, through the enable-when recipe.
fn wire_enabled(
    solution: &Solution,
    session: &mut Session,
    allocator: &mut Ids,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    let gate = body.enabled_when.as_ref().expect("gated button");
    let grid = gate
        .value
        .strip_suffix(".selected")
        .expect("validated gate");
    let old: BTreeSet<_> = session
        .document
        .widgets()
        .into_iter()
        .map(|w| w.id)
        .collect();
    session
        .recipe(
            solution,
            authoring::Recipe::EnableWhen {
                widget: ids[&body.id.value].clone(),
                condition: "selected".into(),
                inputs: vec![("selected".into(), format!("ui:{}.RowSelected", ids[grid]))],
            },
        )
        .map_err(|e| Error::at(gate.referenced, e))?;
    let placed = session
        .document
        .widgets()
        .into_iter()
        .find(|w| !old.contains(&w.id) && w.type_id == "expression2")
        .expect("enable-when adds its expression")
        .id;
    stabilize(
        session,
        allocator,
        &old,
        &placed,
        &format!("{}--enabled", body.id.value),
        gate.referenced,
    )?;
    Ok(())
}

/// A collection: its cell mashup and mapping through the `mashup collection add` code. With
/// `cell:` the cell is a small page of its own, compiled by this same compiler and written beside
/// this page; the builder checks ownership before refreshing files on disk.
#[allow(clippy::too_many_arguments)]
fn wire_collection(
    solution: &Solution,
    project: &str,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    ids: &BTreeMap<String, String>,
    cells: &mut Vec<GeneratedCell>,
    node: &Node,
) -> Result<(), Error> {
    use authoring::session::CellSource;
    let body = node.body();
    let loc = body.id.referenced;
    let data = &body.data.as_ref().expect("validated data").value;
    let source = call(page, data);
    let (cell, mappings, preset) = if let Some(existing) = &body.mashup {
        (
            CellSource::Existing(existing.value.clone()),
            body.map
                .iter()
                .map(|(field, parameter)| (field.clone(), parameter.value.clone()))
                .collect(),
            body.preset.as_ref().map(|p| p.value.clone()),
        )
    } else {
        let spec = body.cell.as_ref().expect("validated cell");
        let base = page
            .name
            .value
            .strip_suffix("_MU")
            .unwrap_or(&page.name.value);
        let name = format!("{base}Cell_MU");
        let mut fields = vec![];
        for (field, role) in [
            (spec.value.title.as_ref(), "title"),
            (spec.value.kpi_dial.as_ref(), "kpi_dial"),
            (spec.value.image.as_ref(), "image"),
            (spec.value.status.as_ref().map(|s| &s.field), "status"),
        ] {
            let Some(field) = field else { continue };
            let base_type = session
                .source_type(solution, model, platform, &format!("{source}.{field}"))
                .map_err(|e| Error::at(spec.referenced, e))?;
            if role == "kpi_dial" && !["NUMBER", "INTEGER", "LONG"].contains(&base_type.as_str()) {
                return Err(Error::at(
                    spec.referenced,
                    format!("cell kpi_dial field {field} must be numeric"),
                ));
            }
            if role == "image" && !["STRING", "IMAGELINK"].contains(&base_type.as_str()) {
                return Err(Error::at(
                    spec.referenced,
                    format!("cell image field {field} must be STRING or IMAGELINK"),
                ));
            }
            fields.push((role, field.clone(), base_type));
        }
        if fields.is_empty() {
            return Err(Error::at(
                spec.referenced,
                "cell needs title, kpi_dial, image or status",
            ));
        }
        let mappings = fields
            .iter()
            .map(|(_, f, _)| (f.clone(), f.clone()))
            .collect();
        // A generated cell is stored at src/<cell>/page.yaml and rebuilt from there, so it
        // compiles from that folder with the page's token selection rebased onto it.
        let cell_dir = solution.src_root().join(&name);
        let tokens = session
            .token_source
            .as_ref()
            .map(|(value, base)| rebase_tokens(value, base, &cell_dir));
        let text = cell_spec(
            page,
            tokens.as_deref(),
            &name,
            &fields,
            spec.value.status.as_ref(),
        );
        let built = compile_pinned(solution, project, &text, &cell_dir, &Pins::default())?;
        let cell = CellSource::Generated {
            name: name.clone(),
            entity: built.entity.clone(),
            content: built.content.clone(),
            css: built.css.clone(),
        };
        if !cells.iter().any(|c| c.name == name) {
            cells.push(GeneratedCell {
                name,
                spec: text,
                artifacts: built,
            });
        }
        (cell, mappings, Some("cards".to_string()))
    };
    let cell_name = match &cell {
        CellSource::Existing(name) | CellSource::Generated { name, .. } => name.clone(),
    };
    let preset = match preset.as_deref() {
        None => None,
        Some("cards") => Some(authoring::CollectionPreset::Cards),
        Some(_) => return Err(Error::at(loc, "preset is cards")),
    };
    session
        .collection(
            solution,
            &authoring::CollectionAddRequest {
                mashup: page.name.value.clone(),
                cell: cell_name,
                data: source,
                mappings,
                uid: None,
                width: None,
                height: None,
                columns: body.per_row.as_ref().map(|n| n.value),
                across: body.across.as_ref().map(|n| n.value),
                preset,
                menu: None,
                placement: None,
            },
            &cell,
            &ids[&body.id.value],
        )
        .map_err(|e| Error::at(loc, e))
}

/// The page spec of a generated KPI cell: a title over a dial, each bound to a parameter.
// CompareCell has image, text-pair and status bands, but no kit has a title/dial-only cell.
// Keep the KPI template for those two roles; its growing value uses the kit's LastContainer.
/// A `tokens:` value as seen from `to`: preset names stay, a file path is re-expressed
/// relative to `to`.
pub(super) fn rebase_tokens(value: &str, base: &std::path::Path, to: &std::path::Path) -> String {
    if crate::core::mashup::tokens::is_preset(value) {
        return value.to_string();
    }
    let file = crate::core::mashup::tokens::lexical(&base.join(value));
    let file: Vec<_> = file.components().collect();
    let to = crate::core::mashup::tokens::lexical(to);
    let to: Vec<_> = to.components().collect();
    let common = file.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out: Vec<String> = vec!["..".into(); to.len() - common];
    out.extend(
        file[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    out.join("/")
}

fn cell_spec(
    page: &Page,
    tokens: Option<&str>,
    name: &str,
    fields: &[(&str, String, String)],
    status: Option<&schema::CellStatus>,
) -> String {
    let parameters: serde_json::Map<_, _> = fields
        .iter()
        .map(|(_, field, base)| (field.clone(), json!(base)))
        .collect();
    let mut slots = serde_json::Map::new();
    for (role, field, _) in fields {
        let (slot, widget) = if *role == "title" {
            (
                "title",
                json!({"label": {"id": "title", "props": {"LabelText": {"bind": format!("param.{field}")}}}}),
            )
        } else {
            (
                "value",
                json!({"kpi_dial": {"id": "dial", "value": {"bind": format!("param.{field}")}}}),
            )
        };
        slots.insert(slot.into(), json!([widget]));
    }
    let mut spec = json!({"version": 1, "rules": "v1", "page": {
        "name": name, "template": "kpi-tile", "parameters": parameters, "slots": slots}});
    if fields
        .iter()
        .any(|(role, _, _)| ["image", "status"].contains(role))
    {
        let mut children = Vec::new();
        for role in ["image", "title", "kpi_dial", "status"] {
            let Some((_, field, _)) = fields.iter().find(|(r, _, _)| *r == role) else {
                continue;
            };
            let bind = json!({"bind":format!("param.{field}")});
            children.push(match role {
                "image" => json!({"image":{"id":"image", "source":bind, "scaling":"contain", "position":"center", "props":{"Height":96,"Margin":"0"}}}),
                "title" => json!({"label":{"id":"title", "props":{"LabelText":bind}}}),
                "kpi_dial" => json!({"kpi_dial":{"id":"dial", "value":bind}}),
                _ => json!({"detail_pair":{"id":"status", "label":"Status", "value":bind, "state":status.expect("status role").state}}),
            });
        }
        spec["page"]["template"] = json!("blank");
        spec["page"]["slots"] =
            json!({"center":[{"column":{"id":"cell", "size":"fill", "children":children}}]});
    }

    if let Some(theme) = &page.theme {
        spec["page"]["theme"] = json!(theme.value);
    }
    if let Some(tokens) = tokens {
        spec["tokens"] = json!(tokens);
    }
    serde_json::to_string_pretty(&spec).expect("spec json")
        + "
"
}

/// `empty: <text>`: the empty-state recipe. A list has no such property, so the recipe adds a
/// notice beside it, which takes stable ids like every other generated widget.
fn wire_empty(
    solution: &Solution,
    session: &mut Session,
    allocator: &mut Ids,
    page: &Page,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    let loc = body.id.referenced;
    let text = body.empty.clone().expect("empty text");
    let data = body
        .data
        .as_ref()
        .map(|d| data_address(page, ids, &d.value));
    let old: BTreeSet<_> = session
        .document
        .widgets()
        .into_iter()
        .map(|w| w.id)
        .collect();
    session
        .recipe(
            solution,
            authoring::Recipe::EmptyState {
                widget: ids[&body.id.value].clone(),
                text,
                data,
            },
        )
        .map_err(|e| Error::at(loc, e))?;
    if matches!(node, Node::List(_)) {
        let placed = session
            .document
            .widgets()
            .into_iter()
            .find(|w| !old.contains(&w.id) && w.type_id == "ptcsbutton")
            .expect("empty-state adds its notice")
            .id;
        stabilize(
            session,
            allocator,
            &old,
            &placed,
            &format!("{}--empty", body.id.value),
            loc,
        )?;
    }
    Ok(())
}

fn wire_pagination(
    solution: &Solution,
    session: &mut Session,
    allocator: &mut Ids,
    page: &Page,
    ids: &mut BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    let grid = body.grid.as_ref().expect("validated grid");
    let target = ids
        .get(&grid.value)
        .ok_or_else(|| Error::at(grid.referenced, "unknown pagination grid"))?;
    let data = find_grid(page, &grid.value)
        .ok_or_else(|| Error::at(grid.referenced, "pagination for must name a grid with data"))?;
    let old: BTreeSet<_> = session
        .document
        .widgets()
        .into_iter()
        .map(|w| w.id)
        .collect();
    session
        .recipe(
            solution,
            authoring::Recipe::Pagination {
                widget: target.clone(),
                data: call(page, data),
            },
        )
        .map_err(|e| Error::at(grid.referenced, e))?;
    let placed = session
        .document
        .widgets()
        .into_iter()
        .find(|w| !old.contains(&w.id) && w.type_id == "ptcspagination")
        .ok_or_else(|| Error::at(grid.referenced, "grid already has pagination"))?
        .id;
    let id = stabilize(
        session,
        allocator,
        &old,
        &placed,
        &body.id.value,
        body.id.referenced,
    )?;
    ids.insert(body.id.value.clone(), id);
    let id = &ids[&body.id.value];
    session
        .set(
            solution,
            &authoring::SetRequest {
                mashup: page.name.value.clone(),
                widget: id.clone(),
                properties: body
                    .props
                    .iter()
                    .filter(|(_, v)| binding(&v.value).is_none())
                    .map(|(k, v)| (k.clone(), v.value.clone()))
                    .collect(),
                style_from: None,
                styles: styles(body),
            },
        )
        .map_err(|e| Error::at(body.id.referenced, e))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn wire_values(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    ids: &BTreeMap<String, String>,
    node: &Node,
) -> Result<(), Error> {
    let body = node.body();
    let id = &ids[&body.id.value];
    super::elements::wire_values(solution, session, model, platform, page, ids, node)?;
    if body.initialize {
        session
            .on(
                model,
                platform,
                "mashup:Loaded",
                &format!("ui:{id}.Evaluate"),
            )
            .map_err(|e| Error::at(body.id.referenced, e))?;
    }
    for (prop, value) in &body.props {
        if let Some(from) = binding(&value.value) {
            let from = address(page, ids, from, value.referenced)?;
            session
                .bind(solution, model, platform, &from, &format!("ui:{id}.{prop}"))
                .map_err(|e| Error::at(value.referenced, e))?;
        }
    }
    for child in descendants(node) {
        wire_values(solution, session, model, platform, page, ids, child)?;
    }
    Ok(())
}

fn validate_contracts(
    solution: &Solution,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
) -> Result<(), Error> {
    for contract in &page.contracts {
        if !page.data.contains_key(&contract.data) {
            return Err(Error::at(
                contract.location,
                format!("component contract: unknown data {}", contract.data),
            ));
        }
        for (field, expected) in &contract.fields {
            let address = format!("{}.{}", call(page, &contract.data), field);
            let actual = session
                .source_type(solution, model, platform, &address)
                .map_err(|e| {
                    Error::at(
                        contract.location,
                        format!("component contract {field}: {e}"),
                    )
                })?;
            if expected != "ANY" && &actual != expected {
                return Err(Error::at(
                    contract.location,
                    format!("component contract {field}: expected {expected}, got {actual}"),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn component_recipe(
    solution: &Solution,
    mashup: &str,
    container: &str,
    name: &str,
    settings: &[String],
    loaded: authoring::ExistingMashup,
) -> Result<authoring::RecipePlan, authoring::AuthoringError> {
    super::deep(|| component_recipe_nested(solution, mashup, container, name, settings, loaded))
}

fn component_recipe_nested(
    solution: &Solution,
    mashup: &str,
    container: &str,
    name: &str,
    settings: &[String],
    mut loaded: authoring::ExistingMashup,
) -> Result<authoring::RecipePlan, authoring::AuthoringError> {
    let invalid = |e: Error| authoring::AuthoringError::Invalid(e.to_string());
    let mut invocation = json!({"use":name});
    for setting in settings {
        let (key, value) = setting
            .split_once('=')
            .filter(|(k, _)| !k.is_empty())
            .ok_or_else(|| {
                authoring::AuthoringError::Invalid("component --set needs prop=value".into())
            })?;
        if invocation.get(key).is_some() {
            return Err(authoring::AuthoringError::Invalid(format!(
                "component repeats prop {key}"
            )));
        }
        invocation[key] = serde_json::from_str(value).unwrap_or_else(|_| json!(value));
    }
    let project = crate::core::entity::parse(&loaded.entity_before)
        .map_err(|e| authoring::AuthoringError::Invalid(format!("{e:?}")))?
        .project;
    loaded.deterministic = true;
    let document = authoring::parse_loaded(&loaded)?;
    let mut data = serde_json::Map::new();
    let mut ambiguous = BTreeSet::new();
    for section in crate::core::bindings::data_sections(document.value()).values() {
        for call in &section.services {
            let service = if section.collection == "Things" {
                format!("{}.{}", section.entity, call.target)
            } else {
                format!("{}/{}.{}", section.collection, section.entity, call.target)
            };
            if data
                .insert(call.instance.clone(), json!({"service":service}))
                .is_some()
            {
                ambiguous.insert(call.instance.clone());
            }
        }
    }
    let parameters: serde_json::Map<_, _> =
        crate::core::mashup::describe::mashup_parameters(&loaded.entity_before)
            .map_err(authoring::AuthoringError::Unreadable)?
            .into_iter()
            .map(|p| (p.name, json!(p.base_type)))
            .collect();
    let mut source = json!({"version":1,"rules":"v1","page":{"name":mashup,"template":"blank","parameters":parameters,"data":data,"slots":{"center":[{"component":invocation}]}}});
    if let Some(theme) = document
        .value()
        .pointer("/UI/Properties/StyleTheme")
        .and_then(Value::as_str)
    {
        source["page"]["theme"] = json!(theme);
    }
    let text = source.to_string();
    let spec = schema::parse_solution(&text, solution).map_err(invalid)?;
    let page = &spec.page.value;
    fn uses_alias(node: &Node, alias: &str) -> bool {
        let body = node.body();
        let names_alias = |text: &str| {
            text == alias || text.strip_prefix(alias).is_some_and(|s| s.starts_with('.'))
        };
        body.data.as_ref().is_some_and(|s| s.value == alias)
            || body.options.as_ref().is_some_and(|s| s.value == alias)
            || body
                .sparkline
                .as_ref()
                .and_then(|s| s.value.data.as_deref())
                .is_some_and(|s| s == alias)
            || body
                .props
                .values()
                .chain(body.value.iter())
                .chain(body.status.iter())
                .chain(body.delta.iter())
                .any(|v| binding(&v.value).is_some_and(names_alias))
            || descendants(node).any(|child| uses_alias(child, alias))
    }
    for alias in &ambiguous {
        if page.contracts.iter().any(|c| &c.data == alias)
            || page
                .slots
                .values()
                .flat_map(|s| &s.value)
                .any(|n| uses_alias(n, alias))
        {
            return Err(authoring::AuthoringError::Ambiguous(format!(
                "component: data alias {alias} names several calls; use unique aliases"
            )));
        }
    }

    let parent = document
        .find(container)
        .map_err(authoring::AuthoringError::Edit)?
        .id;
    let before: BTreeSet<_> = document.widgets().into_iter().map(|w| w.id).collect();
    let mut session = Session::existing(
        mashup,
        loaded.entity_before.clone(),
        document,
        loaded.assets.css.clone(),
    );
    let mut allocator = Ids::new(before.iter().cloned());
    let mut ids: BTreeMap<_, _> = before.iter().map(|id| (id.clone(), id.clone())).collect();
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    for node in &page.slots["center"].value {
        add(
            solution,
            &mut session,
            &mut allocator,
            &mut ids,
            page,
            node,
            (&parent, None),
        )
        .map_err(invalid)?;
    }
    let mut cells = Vec::new();
    wire_nodes(
        solution,
        &project,
        &mut session,
        &model,
        platform.as_ref(),
        page,
        &mut allocator,
        &mut ids,
        &mut cells,
    )
    .map_err(invalid)?;
    let generated = cells
        .iter()
        .map(|cell| super::build::generated_cell(solution, &project, cell))
        .collect::<Result<Vec<_>, _>>()?;
    let notes = cells
        .iter()
        .map(|cell| {
            format!(
                "{}: generated collection cell (XML, sidecars, spec and baseline)",
                cell.name
            )
        })
        .collect();
    for node in &page.slots["center"].value {
        apply_size(solution, &mut session, page, &ids, node).map_err(invalid)?;
    }
    super::elements::finish_layout(&mut session, page, &ids).map_err(invalid)?;
    let added = session
        .document
        .widgets()
        .into_iter()
        .filter(|w| !before.contains(&w.id))
        .map(|w| w.id)
        .collect();
    Ok(authoring::RecipePlan {
        edit: authoring::finish_edit_with_css(
            loaded,
            session.document,
            "mashup recipe",
            Some(session.css),
        )?,
        name: "component",
        generated,
        added,
        removed: vec![],
        wired: vec![],
        notes,
        warnings: vec![],
    })
}

#[allow(clippy::too_many_arguments)]
fn wire_nodes(
    solution: &Solution,
    project: &str,
    session: &mut Session,
    model: &types::Model,
    platform: Option<&types::Platform>,
    page: &Page,
    allocator: &mut Ids,
    ids: &mut BTreeMap<String, String>,
    cells: &mut Vec<GeneratedCell>,
) -> Result<(), Error> {
    validate_contracts(solution, session, model, platform, page)?;
    for nodes in page.slots.values() {
        for node in &nodes.value {
            wire(solution, session, model, platform, page, ids, node)?;
        }
    }
    fn collections<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        if matches!(node, Node::Collection(_)) {
            out.push(node);
        }
        for child in descendants(node) {
            collections(child, out);
        }
    }
    let mut shown = vec![];
    for node in page.slots.values().flat_map(|s| &s.value) {
        collections(node, &mut shown);
    }
    for node in shown {
        wire_collection(
            solution, project, session, model, platform, page, ids, cells, node,
        )?;
    }
    fn pagers<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        if matches!(node, Node::Pagination(_)) {
            out.push(node);
        }
        for child in descendants(node) {
            pagers(child, out);
        }
    }
    let mut pagination = vec![];
    for node in page.slots.values().flat_map(|s| &s.value) {
        pagers(node, &mut pagination);
    }
    for node in pagination {
        wire_pagination(solution, session, allocator, page, ids, node)?;
    }
    fn gated<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        if matches!(node, Node::Button(_)) && node.body().enabled_when.is_some() {
            out.push(node);
        }
        for child in descendants(node) {
            gated(child, out);
        }
    }
    let mut gates = vec![];
    for node in page.slots.values().flat_map(|s| &s.value) {
        gated(node, &mut gates);
    }
    for node in gates {
        wire_enabled(solution, session, allocator, ids, node)?;
    }
    fn emptied<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        if node.body().empty.is_some() {
            out.push(node);
        }
        for child in descendants(node) {
            emptied(child, out);
        }
    }
    let mut empties = vec![];
    for node in page.slots.values().flat_map(|s| &s.value) {
        emptied(node, &mut empties);
    }
    for node in empties {
        wire_empty(solution, session, allocator, page, ids, node)?;
    }
    for node in page.slots.values().flat_map(|s| &s.value) {
        wire_sparklines(solution, session, model, platform, page, ids, node)?;
    }
    wire_selections(solution, session, model, platform, page, allocator, ids)?;
    for node in page.slots.values().flat_map(|s| &s.value) {
        wire_values(solution, session, model, platform, page, ids, node)?;
    }
    // The root container is no node, so its bound props are wired here.
    if let Some(root) = &page.root {
        if root
            .value
            .props
            .values()
            .any(|v| binding(&v.value).is_some())
        {
            let id = session
                .document
                .find("center")
                .map_err(|e| Error::at(root.referenced, e))
                .map(|found| {
                    session.document.at(&found.path)["Properties"]["Id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })?;
            for (prop, value) in &root.value.props {
                if let Some(from) = binding(&value.value) {
                    let from = address(page, ids, from, value.referenced)?;
                    session
                        .bind(solution, model, platform, &from, &format!("ui:{id}.{prop}"))
                        .map_err(|e| Error::at(value.referenced, e))?;
                }
            }
        }
    }
    Ok(())
}

/// Whether a binding reads or writes a property of the widget other than its `Name`.
fn touches_parameters(content: &Value, id: &str) -> bool {
    content["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|binding| {
            let ends = [
                (
                    &binding["SourceArea"],
                    &binding["SourceId"],
                    "SourceProperty",
                ),
                (
                    &binding["TargetArea"],
                    &binding["TargetId"],
                    "TargetProperty",
                ),
            ];
            ends.iter().any(|(area, widget, key)| {
                *area == "UI"
                    && *widget == id
                    && binding["PropertyMaps"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|map| map[*key].as_str().is_some_and(|name| name != "Name"))
            })
        })
}
