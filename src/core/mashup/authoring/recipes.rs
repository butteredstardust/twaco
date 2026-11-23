//! Recipes: the wiring real mashups repeat, each written in one edit from the same pieces
//! `mashup add`, `mashup bind` and `mashup on` use. Counts are from 1,084 reference mashups.

mod layout;
pub(crate) use layout::{grid_cell_props, grid_row_props, input_variant_styles};

use super::{
    bind_in, cache_entry, contained_parameters, finish_edit_with_css, load_existing,
    navigation_definition, on_in, parse_loaded, AuthoringContext, AuthoringError, EditPlan,
    ParameterDefinition,
};
use crate::core::config::Solution;
use crate::core::mashup;
use crate::core::mashup::address::parse_address;
use crate::core::mashup::describe::mashup_parameters;
use crate::core::mashup::edit::MashupDocument;
use crate::core::mashup::layout as layout_rules;
use crate::core::{bindings, types, widgets};
use serde_json::{json, Value};

/// The most series a combo chart holds (DataField1..DataField24 in the 10.2 widget).
const MAX_COMBO_SERIES: usize = 24;

/// One recipe and what it needs. Addresses are those `mashup bind` and `mashup on` take.
#[derive(Clone, Debug)]
pub enum Recipe {
    Component {
        container: String,
        name: String,
        settings: Vec<String>,
    },
    Panel {
        container: String,
        compact: Option<u32>,
        style: String,
        fit: String,
    },
    ToolbarBand {
        container: String,
    },
    Card {
        container: String,
        caption: String,
        value: Option<String>,
        status: Option<String>,
        kind: String,
    },
    SliderAction {
        slider: String,
        button: String,
    },
    Toolbar {
        container: String,
        then: String,
        search_param: String,
        hint: String,
        width: u32,
        run_on: String,
        buttons: Vec<(String, String)>,
    },
    LabelPairs {
        container: String,
        data: String,
        fields: Vec<String>,
        title: Option<String>,
        columns: Option<u32>,
        replace: Option<String>,
        row: Option<String>,
        layout: String,
        status: Option<String>,
        status_state: Option<String>,
    },
    Collapsible {
        container: String,
        toggle: Option<String>,
        add_toggle_in: Option<String>,
        tab: String,
        collapsed: bool,
    },
    /// A widget enabled only while a condition holds (an expression into `Disabled`; 146 seen).
    EnableWhen {
        widget: String,
        condition: String,
        inputs: Vec<(String, String)>,
    },
    /// A widget shown only while a condition holds (an expression into `Visible`; 100+ seen).
    ShowWhen {
        widget: String,
        condition: String,
        inputs: Vec<(String, String)>,
    },
    /// A label's text computed by an expression (55 seen).
    Text {
        widget: String,
        expression: String,
        inputs: Vec<(String, String)>,
    },
    /// Evaluate on a click and write a mashup parameter, without evaluating on input changes.
    SetOnClick {
        trigger: String,
        parameter: String,
        expression: String,
        inputs: Vec<(String, String)>,
    },
    /// An interactive progress tracker whose field maps follow the result DataShape.
    Tracker {
        widget: String,
        data: String,
        field_maps: Vec<(String, String)>,
        initial_step: u32,
        step_size: String,
        host: Option<String>,
    },
    Tree {
        widget: String,
        data: String,
        field_maps: Vec<(String, String)>,
    },
    /// Status messages after service success and/or failure.
    Message {
        after: String,
        text: Option<String>,
        from: Option<String>,
        kind: String,
        on_failure: Option<String>,
    },
    /// A confirmation before a click runs its handler (44 seen).
    Confirm {
        trigger: String,
        then: String,
        title: Option<String>,
        message: Option<String>,
        action_label: Option<String>,
        cancel_label: Option<String>,
    },
    /// A popup (or page) opened by an event, given values, and a refresh when it closes (165/166 seen).
    Popup {
        trigger: String,
        target: String,
        window: String,
        size: Option<(u32, u32)>,
        title: Option<String>,
        pass: Vec<(String, String)>,
        refresh: Option<String>,
    },
    /// A popup closing itself once its save completes (232 seen).
    CloseAfter {
        after: String,
    },
    /// A service run again every so often (48 seen).
    AutoRefresh {
        call: String,
        every: Option<u32>,
        interval: Option<String>,
    },
    /// A chart drawing a service result: `Data` bound, the category field named (pie: label and
    /// value fields). Every other numeric column of a single source becomes a series.
    Sparkline {
        container: String,
        data: String,
        x: String,
        value: String,
    },
    Chart {
        series: Vec<String>,
        widget: String,
        data: String,
        x: String,
        value: Option<String>,
        kind: Option<String>,
        title: Option<String>,
        x_title: Option<String>,
        y_title: Option<String>,
        stacked: bool,
    },
    /// A grid's columns, in order, with titles and widths, rendered by each field's base type.
    Columns {
        auto_select_first: bool,
        widget: String,
        data: Option<String>,
        columns: Vec<String>,
        date_format: Option<String>,
        /// Listed columns made editable; the grid is made editable with them.
        editable: Vec<String>,
        column_editors: Vec<(String, String)>,
        preserve_whitespace: Vec<String>,
        status: Option<(String, String)>,
    },
    Pagination {
        widget: String,
        data: String,
    },
    /// A dropdown or list filled from a service result, showing one field.
    Picker {
        widget: String,
        data: String,
        display: String,
        value: Option<String>,
        selected: Option<String>,
        /// ClearSelectionItem off: no "(None)" row.
        no_none_row: bool,
        /// AutoSelectFirstRow on.
        auto_select_first: bool,
        /// A list that selects several rows (MultiSelect on).
        multiple: bool,
    },
    /// The kit's grey left column: fixed 276px, a title row over a one-tab strip whose tab holds
    /// the content.
    SidePanel {
        container: String,
        title: String,
        info: Option<String>,
        tab: String,
    },
    /// A kit section: a title row over a body row, both in the container.
    Section {
        container: String,
        title: String,
        center: bool,
    },
    /// A kit gallery: rows of equal height holding equal growing cells.
    CardGrid {
        container: String,
        per_row: u32,
        cells: u32,
    },
    /// One of the kit's text field looks.
    InputVariant {
        widget: String,
        variant: String,
    },
    /// The kit's "No results." empty state of a collection, grid or list.
    EmptyState {
        widget: String,
        text: String,
        data: Option<String>,
    },
    /// A menu bar showing a Menu of sub-Menus as an accordion, in a column of a fixed width.
    NestedMenu {
        menubar: String,
        menu: String,
        auto_collapse: bool,
        width: u32,
    },
    /// A click on a collection cell or grid row opens the mashup its selected row names.
    NavigateOnClick {
        widget: String,
        field: String,
        window: String,
    },
    /// A check before an action: the handler runs when it holds, a message shows when not.
    Validate {
        trigger: String,
        condition: String,
        inputs: Vec<(String, String)>,
        then: String,
        otherwise: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct RecipeRequest {
    pub mashup: String,
    pub recipe: Recipe,
}

#[derive(Debug)]
pub struct RecipePlan {
    pub(crate) edit: EditPlan,
    pub(crate) generated: Vec<mashup::page::build::Plan>,
    pub name: &'static str,
    /// The widgets the recipe added.
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// What it wired, one line each, in `mashup describe` address form.
    pub wired: Vec<String>,
    /// What the wiring changed besides, such as a grid's selection turned on.
    pub notes: Vec<String>,
}

impl RecipePlan {
    pub(crate) fn unchanged(&self) -> bool {
        self.edit.unchanged()
            && self
                .generated
                .iter()
                .all(|p| p.report["files"].as_array().is_some_and(|f| f.is_empty()))
    }
}

pub(crate) fn recipe_transaction<'a>(
    solution: &'a Solution,
    plan: &RecipePlan,
) -> Result<crate::core::transaction::Transaction<'a>, AuthoringError> {
    let mut tx = super::edit_transaction(solution, &plan.edit)?;
    for generated in &plan.generated {
        generated.append_to(solution, &mut tx)?;
    }
    Ok(tx)
}

pub(crate) fn apply_recipe(
    solution: &Solution,
    plan: &RecipePlan,
    lock: &crate::core::lock::WorkspaceLock,
) -> Result<(), AuthoringError> {
    recipe_transaction(solution, plan)?
        .apply(lock)
        .map_err(AuthoringError::Write)?;
    Ok(())
}

impl Recipe {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Component { .. } => "component",
            Self::Panel { .. } => "panel",
            Self::Card { .. } => "card",
            Self::SliderAction { .. } => "slider-action",
            Self::Toolbar { .. } | Self::ToolbarBand { .. } => "toolbar",
            Self::LabelPairs { .. } => "label-pairs",
            Self::Collapsible { .. } => "collapsible",
            Self::EnableWhen { .. } => "enable-when",
            Self::ShowWhen { .. } => "show-when",
            Self::Text { .. } => "text",
            Self::SetOnClick { .. } => "set-on-click",
            Self::Tracker { .. } => "tracker",
            Self::Tree { .. } => "tree",
            Self::Message { .. } => "message",
            Self::Confirm { .. } => "confirm",
            Self::Popup { .. } => "popup",
            Self::CloseAfter { .. } => "close-after",
            Self::AutoRefresh { .. } => "auto-refresh",
            Self::Validate { .. } => "validate",
            Self::NestedMenu { .. } => "nested-menu",
            Self::NavigateOnClick { .. } => "navigate-on-click",
            Self::Sparkline { .. } => "sparkline",
            Self::Chart { .. } => "chart",
            Self::Columns { .. } => "columns",
            Self::Pagination { .. } => "pagination",
            Self::Picker { .. } => "picker",
            Self::SidePanel { .. } => "side-panel",
            Self::EmptyState { .. } => "empty-state",
            Self::Section { .. } => "section",
            Self::CardGrid { .. } => "card-grid",
            Self::InputVariant { .. } => "input-variant",
        }
    }
}

struct Work<'a> {
    document: MashupDocument,
    solution: &'a Solution,
    tokens: &'a mashup::tokens::Tokens,
    context: AuthoringContext<'a>,
    added: Vec<String>,
    removed: Vec<String>,
    wired: Vec<String>,
    notes: Vec<String>,
}

fn untriggered_calls(content: &Value, mashup: &str) -> Vec<String> {
    let mut notes = Vec::new();
    for (section_name, section) in bindings::data_sections(content) {
        for call in section.services {
            let bound = content["DataBindings"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|binding| {
                    binding["SourceArea"] == "Data"
                        && binding["SourceSection"] == section_name
                        && binding["SourceId"] == call.instance
                });
            let triggered = content["Events"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|event| {
                    event["EventHandlerArea"] == "Data"
                        && event["EventHandlerId"] == section_name
                        && event["EventHandlerService"] == call.instance
                });
            if bound && !triggered {
                let alias = if call.instance == call.target {
                    String::new()
                } else {
                    format!("[as {}]", call.instance)
                };
                let address = format!("data:{}.{}{alias}", section.entity, call.target);
                let invoke = if address.contains(char::is_whitespace) {
                    format!("\"{address}\"")
                } else {
                    address.clone()
                };
                notes.push(format!("{address} is bound but has no trigger; run twaco mashup on {mashup} --event mashup:Loaded --invoke {invoke} --apply"));
            }
        }
    }
    notes
}

impl Work<'_> {
    fn add_in(&mut self, parent: &[usize], type_id: &str, properties: &[(&str, Value)]) -> String {
        let definition = widgets::catalog().by_type(type_id).expect("recipe widget");
        let number = self.document.allocate();
        let mut widget = mashup::edit::new_widget(definition, number, false);
        for (key, value) in properties {
            mashup::edit::insert_sorted(
                widget["Properties"].as_object_mut().unwrap(),
                key,
                value.clone(),
            );
        }
        let id = widget["Properties"]["Id"].as_str().unwrap().to_string();
        self.wired.push(format!(
            "add {id}: {}",
            properties
                .iter()
                .map(|(key, _)| (*key, &widget["Properties"][*key]))
                .collect::<std::collections::BTreeMap<_, _>>()
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        self.document.push_child(parent, widget);
        self.added.push(id.clone());
        id
    }

    fn pagination(&mut self, widget: &str, data: &str) -> Result<(), AuthoringError> {
        let grid = widget_id(&self.document, widget)?;
        if grid.type_id != "ptcsgrid" || self.source_type(data)? != "INFOTABLE" {
            return Err(AuthoringError::Invalid(
                "pagination needs a grid and --data <INFOTABLE service result>".into(),
            ));
        }
        if let Some(pager) =
            self.document.widgets().iter().find(|candidate| {
                self.document.at(&candidate.path)["twacoPaginationFor"] == grid.id
            })
        {
            if self.document.at(&pager.path)["twacoPaginationData"] != data {
                return Err(AuthoringError::Refused(format!(
                    "{} already pages {}; remove its pagination before changing --data",
                    pager.id, grid.id
                )));
            }
            return Ok(());
        }
        self.bind(data, &format!("ui:{}.Data", grid.id))?;
        let pager = self
            .document
            .place(
                widgets::catalog()
                    .by_type("ptcspagination")
                    .expect("pagination catalogue"),
                &mashup::edit::Placement::BelowInColumn(grid.id.clone()),
            )
            .map_err(AuthoringError::Edit)?;
        self.added.push(pager.id.clone());
        let pager_ref = widget_id(&self.document, &pager.id)?;
        self.document.at_mut(&pager_ref.path)["twacoPaginationFor"] = json!(grid.id);
        self.document.at_mut(&pager_ref.path)["twacoPaginationData"] = json!(data);
        let parent = &pager_ref.path[..pager_ref.path.len() - 1];
        self.document
            .set_property(parent, "justify-content", json!("center"));
        let inputs = vec![("table".into(), data.into())];
        let count = self.computed(
            "expression2",
            "result = table && table.rows ? table.rows.length : 0;",
            Some("NUMBER"),
            &inputs,
            true,
        )?;
        self.bind(
            &format!("ui:{count}.Output"),
            &format!("ui:{}.ResultsNumber", pager.id),
        )?;
        let mut inputs = inputs;
        inputs.push(("size".into(), format!("ui:{}.PageSize", pager.id)));
        let reset = self.computed("expression2", "result = 1;", Some("NUMBER"), &inputs, true)?;
        let reset_ref = widget_id(&self.document, &reset)?;
        self.set(&reset_ref, &[("DataChangeType", json!("ALWAYS"))]);
        self.bind(
            &format!("ui:{reset}.Output"),
            &format!("ui:{}.PageNumber", pager.id),
        )?;
        inputs.push(("page".into(), format!("ui:{}.PageNumber", pager.id)));
        let slice = self.computed("expression2", "result = table && table.rows ? {dataShape: table.dataShape, rows: table.rows.slice((Math.max(1, page || 1) - 1) * Math.max(1, size || 10), Math.max(1, page || 1) * Math.max(1, size || 10))} : {dataShape: {}, rows: []};", Some("INFOTABLE"), &inputs, true)?;
        let mut replaced = Vec::new();
        for binding in self.document.bindings_mut() {
            if binding["TargetId"] == grid.id {
                let source = binding["SourceId"].as_str().unwrap_or_default().to_string();
                if let Some(maps) = binding["PropertyMaps"].as_array_mut() {
                    maps.retain(|map| {
                        let data = map["TargetProperty"] == "Data";
                        if data {
                            let property = map["SourceProperty"].as_str().unwrap_or_default();
                            replaced.push(format!("{source}.{property}"));
                        }
                        !data
                    });
                }
            }
        }
        if !replaced.is_empty() {
            self.notes.push(format!(
                "{}.Data now comes from the page slice; replaced binding(s) from {}",
                grid.id,
                replaced.join(", ")
            ));
        }
        self.document.bindings_mut().retain(|binding| {
            binding["PropertyMaps"]
                .as_array()
                .is_none_or(|maps| !maps.is_empty())
        });
        self.bind(
            &format!("ui:{slice}.Output"),
            &format!("ui:{}.Data", grid.id),
        )?;
        self.notes.push(format!("{} pages {} locally (10 rows by default); new results or page size reset to page 1; verify expression evaluation in Runtime", pager.id, grid.id));
        Ok(())
    }

    fn toolbar(
        &mut self,
        container: &str,
        then: &str,
        search_param: &str,
        options: (&str, u32, &str),
        buttons: &[(String, String)],
    ) -> Result<(), AuthoringError> {
        let (hint, width, run_on) = options;
        if width == 0 || !["change", "enter", "both"].contains(&run_on) {
            return Err(AuthoringError::Invalid(
                "toolbar: --width must be above 0; --run-on must be change, enter or both".into(),
            ));
        }
        let target = widget_id(&self.document, container)?;
        if target.type_id != "flexcontainer"
            || self.document.at(&target.path)["Properties"]["ResponsiveLayout"] != true
            || self.document.at(&target.path)["Properties"]["positioning"] == "static"
        {
            return Err(AuthoringError::Invalid(format!(
                "{container} must be a responsive flexcontainer"
            )));
        }
        if self.document.at(&target.path)["Widgets"]
            .as_array()
            .is_some_and(|children| !children.is_empty())
        {
            return Err(AuthoringError::Refused(format!(
                "{container} is not empty; toolbar needs an empty container"
            )));
        }
        let end = super::on::resolve_handler(
            &self.document,
            self.context.model,
            self.context.platform,
            then,
        )?;
        let whole_call = match (&end, parse_address(then)) {
            (
                super::end::End::Data {
                    entity, service, ..
                },
                Ok(mashup::address::Address::Data { entity_service, .. }),
            ) => entity_service == format!("{entity}.{service}"),
            _ => false,
        };
        if !whole_call {
            return Err(AuthoringError::Invalid(
                "toolbar: --then must name a whole existing data call".into(),
            ));
        }
        let (section_name, call_name) = super::bind::existing_data_call(&self.document, &end)?
            .ok_or_else(|| {
                AuthoringError::Unknown(format!(
                    "{then} is not in the mashup; add the data call first"
                ))
            })?;
        let sections = bindings::data_sections(self.document.value());
        let (section, call) = bindings::service_at(&sections, &section_name, &call_name)
            .ok_or_else(|| AuthoringError::Unknown(format!("{then} is not in the mashup")))?;
        let resolver = bindings::Resolver::new(self.context.model, self.context.platform);
        let service = resolver
            .service(section, &call.target)
            .service
            .ok_or_else(|| {
                AuthoringError::Unknown(format!("{then}: service definition is unavailable"))
            })?;
        let strings = service
            .parameters
            .iter()
            .filter(|p| p.value.base_type == "STRING")
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>();
        if !strings.contains(&search_param) {
            return Err(AuthoringError::Invalid(format!(
                "{then}: --search-param {search_param} must name a STRING input parameter; STRING parameters: {}",
                if strings.is_empty() { "none".into() } else { strings.join(", ") }
            )));
        }
        for (label, handler) in buttons {
            if label.trim().is_empty() {
                return Err(AuthoringError::Invalid(
                    "toolbar: --button needs Label=<handler>".into(),
                ));
            }
            super::on::resolve_handler(
                &self.document,
                self.context.model,
                self.context.platform,
                handler,
            )?;
        }
        let alias = if call.instance == call.target {
            String::new()
        } else {
            format!("[as {}]", call.instance)
        };
        let parameter = format!(
            "data:{}.{}{alias}.{search_param}",
            section.entity, call.target
        );
        let rules = self.document.rules();
        let group = |role: &str| {
            rules
                .evaluate(
                    &layout_rules::context::Node::op("toolbar", layout_rules::Element::Container)
                        .slot(role)
                        .context(),
                )
                .patch()
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect::<Vec<_>>()
        };
        let row = self.add_in(&target.path, "flexcontainer", &as_pairs(&group("row")));
        self.toolbar_band(&row)?;
        let row_path = widget_id(&self.document, &row)?.path;
        let left = self.add_in(&row_path, "flexcontainer", &as_pairs(&group("left")));
        let left_path = widget_id(&self.document, &left)?.path;
        let field = self.add_in(
            &left_path,
            "ptcstextfield",
            &[
                ("Icon", json!("cds:icon_search")),
                ("HintText", json!(hint)),
                ("Width", json!(width)),
                ("ShowClearText", json!(true)),
                ("Margin", json!("5 12 5 5")),
                ("TextAlignment", json!("left")),
                ("UseTheme", json!(true)),
            ],
        );
        let right = self.add_in(&row_path, "flexcontainer", &as_pairs(&group("right")));
        let right_path = widget_id(&self.document, &right)?.path;
        self.bind(&format!("ui:{field}.Text"), &parameter)?;
        if run_on != "enter" {
            self.on(&format!("ui:{field}.Changed"), then)?;
            self.notes.push(format!(
                "{then} runs on each keystroke (Changed); no debounce; the clear button fires Changed, so clearing reruns it"
            ));
        }
        if run_on != "change" {
            self.on(&format!("ui:{field}.EnterKeyPressed"), then)?;
        }
        for (label, handler) in buttons {
            let button = self.add_in(
                &right_path,
                "ptcsbutton",
                &[
                    ("Label", json!(label)),
                    ("LabelAlignment", json!("center")),
                    ("ButtonType", json!("transparent")),
                    ("Margin", json!("0")),
                    ("UseTheme", json!(true)),
                ],
            );
            self.on(&format!("ui:{button}.Clicked"), handler)?;
        }
        Ok(())
    }

    fn label_pairs(
        &mut self,
        container: &str,
        data: &str,
        fields: &[String],
        options: (Option<&str>, Option<u32>, Option<&str>),
        presentation: (Option<&str>, &str, Option<&str>, Option<&str>),
    ) -> Result<(), AuthoringError> {
        let (title, columns, replace) = options;
        let (row_mode, layout, status, status_state) = presentation;
        let inline = layout == "inline";
        if columns == Some(0) {
            return Err(AuthoringError::Invalid(
                "label-pairs: --columns must be above 0".into(),
            ));
        }
        let target = widget_id(&self.document, container)?;
        if target.type_id != "flexcontainer"
            || self.document.at(&target.path)["Properties"]["ResponsiveLayout"] != true
            || self.document.at(&target.path)["Properties"]["positioning"] == "static"
        {
            return Err(AuthoringError::Invalid(format!(
                "{container} must be a responsive flexcontainer"
            )));
        }
        let address = parse_address(data).map_err(|e| AuthoringError::Invalid(e.to_string()))?;
        if !matches!(
            &address,
            mashup::address::Address::Data {
                member: None,
                selected_rows: false,
                entity_name: false,
                ..
            }
        ) {
            return Err(AuthoringError::Invalid(
                "label-pairs: --data must name a whole existing data call".into(),
            ));
        }
        let end = super::resolve_end(
            &mut self.document,
            Some(self.solution),
            &self.context,
            address,
            true,
            &mut Vec::new(),
        )?;
        if !matches!(&end, super::end::End::Data { member: None, base, .. } if base == "INFOTABLE")
        {
            return Err(AuthoringError::Invalid(
                "label-pairs: --data must name a whole existing data call".into(),
            ));
        }
        let (section_name, call_name) = super::bind::existing_data_call(&self.document, &end)?
            .ok_or_else(|| {
                AuthoringError::Unknown(format!(
                    "{data} is not in the mashup; add the data call first"
                ))
            })?;
        let sections = bindings::data_sections(self.document.value());
        let (section, call) = bindings::service_at(&sections, &section_name, &call_name)
            .ok_or_else(|| AuthoringError::Unknown(format!("{data} is not in the mashup")))?;
        let resolver = bindings::Resolver::new(self.context.model, self.context.platform);
        let service = resolver
            .service(section, &call.target)
            .service
            .ok_or_else(|| {
                AuthoringError::Unknown(format!("{data}: service definition is unavailable"))
            })?;
        let shape = service
            .result
            .data_shape
            .as_deref()
            .and_then(|name| resolver.checkable_shape(name))
            .ok_or_else(|| {
                AuthoringError::Unknown(format!(
                    "{data}: result DataShape is unavailable; add it to the solution"
                ))
            })?;
        if let Some(status) = status {
            self.field(
                &Some(
                    shape
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), f.base_type.clone()))
                        .collect(),
                ),
                status,
            )?;
        }
        let known = Some(
            shape
                .fields
                .iter()
                .map(|f| (f.name.clone(), f.base_type.clone()))
                .collect::<Vec<_>>(),
        );
        let requested = if fields.is_empty() {
            shape
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
        } else {
            fields.to_vec()
        };
        let mut picked = Vec::new();
        for field in requested {
            let (name, caption) = field
                .split_once(':')
                .map_or((field.as_str(), None), |(n, c)| (n, Some(c)));
            self.field(&known, name)?;
            if caption.is_some_and(str::is_empty) || picked.iter().any(|(n, _)| n == name) {
                return Err(AuthoringError::Invalid(format!(
                    "label-pairs: field {name} needs a non-empty caption and must be given once"
                )));
            }
            picked.push((
                name.to_string(),
                caption
                    .map(str::to_string)
                    .unwrap_or_else(|| field_caption(name)),
            ));
        }
        if picked.is_empty() {
            return Err(AuthoringError::Invalid(format!(
                "{data}: result DataShape has no fields"
            )));
        }
        if inline {
            if let Some(styles) = self.document.root_properties().get("StyleProperties") {
                if !styles.is_object() || !styles.get("data").is_some_and(Value::is_object) {
                    return Err(AuthoringError::Refused(
                        "mashup-root.StyleProperties.data must be an object; repair it in Composer"
                            .into(),
                    ));
                }
            }
        }
        let replacement = replace
            .map(|name| widget_id(&self.document, name))
            .transpose();
        let children = self.document.at(&target.path)["Widgets"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let missing_replacement = matches!(
            &replacement,
            Err(AuthoringError::Edit(mashup::edit::EditError::NoWidget(_)))
        );
        if !children.is_empty() && (replace.is_none() || missing_replacement) {
            let mut document = self.document.clone();
            let ids = document
                .widgets()
                .into_iter()
                .filter(|w| w.path.starts_with(&target.path) && w.path != target.path)
                .map(|w| w.id)
                .collect();
            let (bindings, events) = super::remove::dependencies(&document, &ids);
            if events.is_empty()
                && super::remove::unknown_references(document.value(), &ids).is_empty()
            {
                super::remove::remove_indexes(document.bindings_mut(), &bindings);
                document.at_mut(&target.path)["Widgets"] = json!([]);
                let mut probe = Work {
                    document,
                    solution: self.solution,
                    tokens: self.tokens,
                    context: AuthoringContext {
                        parameters: self.context.parameters,
                        model: self.context.model,
                        platform: self.context.platform,
                    },
                    added: Vec::new(),
                    removed: Vec::new(),
                    wired: Vec::new(),
                    notes: Vec::new(),
                };
                probe.label_pairs(
                    container,
                    data,
                    fields,
                    (title, columns, None),
                    presentation,
                )?;
                if detail_output(&self.document, &target.path)
                    == detail_output(&probe.document, &target.path)
                {
                    self.notes.extend(probe.notes);
                    self.notes
                        .push(format!("{}: label-pairs already matches", target.id));
                    return Ok(());
                }
            }
        }
        let replacement = replacement?;
        if let Some(found) = &replacement {
            let beside = found.path.len() == target.path.len()
                && found.path.split_last().map(|(_, p)| p)
                    == target.path.split_last().map(|(_, p)| p);
            if found.path == target.path || !(found.path.starts_with(&target.path) || beside) {
                return Err(AuthoringError::Refused(format!(
                    "{} must be inside or beside {container}",
                    found.id
                )));
            }
        }
        if !children.is_empty()
            && !replacement.as_ref().is_some_and(|r| {
                r.path.len() == target.path.len() + 1
                    && r.path.starts_with(&target.path)
                    && children.len() == 1
            })
        {
            return Err(AuthoringError::Refused(format!(
                "{container} is not empty; --replace must remove its only child"
            )));
        }
        if let Some(found) = replacement {
            let ids = self
                .document
                .widgets()
                .into_iter()
                .filter(|w| w.path.starts_with(&found.path))
                .map(|w| w.id)
                .collect();
            let unknown = super::remove::unknown_references(self.document.value(), &ids);
            if !unknown.is_empty() {
                return Err(AuthoringError::Refused(format!(
                    "{} has unknown references at {}; remove them in Composer first",
                    found.id,
                    unknown.join(", ")
                )));
            }
            let (bindings, events) = super::remove::dependencies(&self.document, &ids);
            super::remove::remove_indexes(self.document.bindings_mut(), &bindings);
            super::remove::remove_indexes(self.document.events_mut(), &events);
            // Keep the destination even when its only child was removed.
            let (&index, parent) = found.path.split_last().unwrap();
            self.document.at_mut(parent)["Widgets"]
                .as_array_mut()
                .unwrap()
                .remove(index);
            self.removed.extend(ids);
            self.wired.push(format!(
                "remove {} binding(s), {} event(s)",
                bindings.len(),
                events.len()
            ));
        }
        let target = widget_id(&self.document, &target.id)?;
        let rules = self.document.rules();
        let decided = |role: &str| {
            rules
                .evaluate(
                    &layout_rules::context::Node::op(
                        "label-pairs",
                        layout_rules::Element::Container,
                    )
                    .slot(role)
                    .context(),
                )
                .patch()
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect::<Vec<_>>()
        };
        self.set(&target, &as_pairs(&decided("target")));
        let row_decided = decided("row");
        let row_properties = as_pairs(&row_decided);
        if let Some(title) = title {
            let header = self.add_in(&target.path, "flexcontainer", &as_pairs(&decided("header")));
            let path = widget_id(&self.document, &header)?.path;
            self.add_in(
                &path,
                "ptcslabel",
                &[
                    ("LabelText", json!(title)),
                    (
                        "LabelType",
                        json!(if inline { "title" } else { "sub-header" }),
                    ),
                    ("Margin", json!(if inline { "0" } else { "5 0 5 5" })),
                    ("UseTheme", json!(true)),
                ],
            );
        }
        let row = self.add_in(&target.path, "flexcontainer", &row_properties);
        let row_path = widget_id(&self.document, &row)?.path;
        let mut paths = Vec::new();
        // Like the kit, one column per pair by default; never more columns than pairs.
        let count = columns.map_or(picked.len(), |n| picked.len().min(n as usize));
        for _ in 0..count {
            let id = self.add_in(&row_path, "flexcontainer", &as_pairs(&decided("column")));
            paths.push(widget_id(&self.document, &id)?.path);
        }
        let alias = if call.instance == call.target {
            String::new()
        } else {
            format!("[as {}]", call.instance)
        };
        let selecting =
            bindings::has_row_selector(self.document.value(), &section_name, &call_name);
        let selected = row_mode.map_or(selecting, |mode| mode == "selected");
        let source = format!(
            "data:{}.{}{alias}{}",
            section.entity,
            call.target,
            if selected { "[SelectedRows]" } else { "" }
        );
        self.notes.push(format!(
            "{source}: row {} ({})",
            if selected { "selected" } else { "first" },
            if row_mode.is_some() {
                "explicit --row"
            } else if selecting {
                "a selecting widget is bound to this call's All Data"
            } else {
                "no selecting widget is bound to this call's All Data"
            }
        ));
        let status_format = status_state
            .map(|name| {
                let file = crate::core::workspace::entities(self.solution)
                    .into_iter()
                    .find(|file| {
                        file.info.collection == "StateDefinitions" && file.info.name == name
                    })
                    .ok_or_else(|| {
                        AuthoringError::Unknown(format!(
                            "no StateDefinition {name}; add it to the solution first"
                        ))
                    })?;
                let bytes = std::fs::read(&file.path)
                    .map_err(|e| AuthoringError::Unreadable(e.to_string()))?;
                super::state_format::inline(
                    &super::state_format::read_definition(&bytes)?,
                    name,
                    status.unwrap_or("Data"),
                )
            })
            .transpose()?;
        for (index, (field, caption)) in picked.iter().enumerate() {
            let column = &paths[index % paths.len()];
            let mut left = column.clone();
            let mut right = column.clone();
            if inline {
                if index >= paths.len() {
                    let divider = self.add_in(
                        column,
                        "flexcontainer",
                        &[
                            ("flex-size", json!("fixed")),
                            ("flex-min-height", json!("1px")),
                            ("flex-max-height", json!("1px")),
                            ("flex-grow", json!(0)),
                            ("Margin", json!("")),
                            ("CustomClass", json!("twaco-detail-divider")),
                        ],
                    );
                    super::set::copy_style_entry(
                        &mut self.document,
                        &divider,
                        json!({"widget":"flex-container","parts":{"$":{"$":{"background":self.tokens.colour("line").map_err(AuthoringError::Invalid)?,"border-color":"inherit","border-radius":"initial","border-style":"none","border-width":"0px","padding-bottom":"","padding-left":"","padding-right":"","padding-top":""}}}}),
                        None,
                        &mut Vec::new(),
                    );
                    self.notes.push(format!("{divider}: 1px divider"));
                }
                let pair = self.add_in(
                    column,
                    "flexcontainer",
                    &[
                        ("flex-direction", json!("row")),
                        ("flex-wrap", json!("nowrap")),
                        ("align-items", json!("center")),
                        ("justify-content", json!("space-between")),
                        ("flex-grow", json!(0)),
                        ("Margin", json!("")),
                    ],
                );
                let path = widget_id(&self.document, &pair)?.path;
                let caption_box = self.add_in(
                    &path,
                    "flexcontainer",
                    &[
                        ("flex-direction", json!("row")),
                        ("align-items", json!("center")),
                        ("Margin", json!("")),
                    ],
                );
                left = widget_id(&self.document, &caption_box)?.path;
                let value_box = self.add_in(
                    &path,
                    "flexcontainer",
                    &[
                        ("flex-direction", json!("row")),
                        ("align-items", json!("center")),
                        ("justify-content", json!("flex-end")),
                        ("Margin", json!("")),
                    ],
                );
                right = widget_id(&self.document, &value_box)?.path;
            }
            if let Some(status) = status {
                let dot = self.add_in(
                    &left,
                    "shape",
                    &[
                        ("ShapeType", json!("circle")),
                        ("Width", json!(12)),
                        ("Height", json!(12)),
                        ("Margin", json!("0 5")),
                        (
                            "ShapeFormatter",
                            status_format.clone().expect("status needs a state"),
                        ),
                    ],
                );
                self.bind(&format!("{source}.{status}"), &format!("ui:{dot}.Data"))?;
            }
            self.add_in(
                &left,
                "ptcslabel",
                &[
                    ("LabelText", json!(caption)),
                    ("LabelType", json!(if inline { "body" } else { "label" })),
                    ("HorizontalAlignment", json!("left")),
                    ("Margin", json!(if inline { "0 5" } else { "5" })),
                    ("UseTheme", json!(true)),
                ],
            );
            let value = self.add_in(
                &right,
                "ptcslabel",
                &[
                    ("LabelText", json!("")),
                    ("LabelType", json!("body")),
                    (
                        "HorizontalAlignment",
                        json!(if inline { "right" } else { "left" }),
                    ),
                    ("Margin", json!(if inline { "0 18 0 5" } else { "0 5 5" })),
                    ("UseTheme", json!(true)),
                ],
            );
            self.bind(
                &format!("{source}.{field}"),
                &format!("ui:{value}.LabelText"),
            )?;
        }
        for (_, message) in bindings::selected_rows_hints(self.document.value()) {
            self.notes
                .push(format!("{}: {message}", bindings::SELECTED_ROWS_RULE));
        }
        self.notes.push(format!("{row}: wrapping uses range sizing with no maximum height. On 10.2.1 set height:auto !important; min-height:0 !important; max-height:none !important on the row bounding box, inner .widget-flexcontainer and every parent up to the panel (guide quirks: wrapping responsive rows)"));
        Ok(())
    }

    // Address resolution can refresh contained parameters or add data calls. Probe a copy so
    // detecting existing wiring cannot change the document, and reuse bind/on's address rules.
    fn has_binding(&self, from: &str, to: &str) -> bool {
        matches!(
            bind_in(
                &mut self.document.clone(),
                Some(self.solution),
                &self.context,
                from,
                to
            ),
            Err(AuthoringError::Exists(_))
        )
    }

    fn has_handler(&self, event: &str, invoke: &str) -> bool {
        matches!(
            on_in(&mut self.document.clone(), &self.context, event, invoke),
            Err(AuthoringError::Exists(_))
        )
    }

    fn has_set_on_click(
        &self,
        trigger: &str,
        destination: &str,
        expression: &str,
        base: &str,
        inputs: &[(String, String)],
    ) -> bool {
        let mut names: Vec<_> = inputs.iter().map(|(name, _)| name.as_str()).collect();
        names.sort_unstable();
        self.document.widgets().iter().any(|widget| {
            let props = &self.document.at(&widget.path)["Properties"];
            if widget.type_id != "expression2"
                || props["Expression"] != expression
                || props["OutputBaseType"] != base
                || props["AutoEvaluate"] != false
                || props["DisableInitialOutput"] != true
                || props["DataChangeType"] != "ALWAYS"
            {
                return false;
            }
            let mut existing_names: Vec<_> = props["_currentParameterDefs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|definition| definition["ParameterName"].as_str())
                .collect();
            existing_names.sort_unstable();
            existing_names == names
                && self.has_handler(trigger, &format!("ui:{}.Evaluate", widget.id))
                && self.has_binding(&format!("ui:{}.Output", widget.id), destination)
                && inputs.iter().all(|(name, address)| {
                    self.has_binding(address, &format!("ui:{}.{}", widget.id, name))
                })
        })
    }

    fn message(
        &mut self,
        event: &str,
        text: Option<&str>,
        from: Option<&str>,
        kind: &str,
    ) -> Result<(), AuthoringError> {
        let exists = self.document.widgets().iter().any(|widget| {
            let props = &self.document.at(&widget.path)["Properties"];
            widget.type_id == "statusmessage"
                && props["MessageType"] == kind
                && text.is_none_or(|text| {
                    props["Message"] == text
                        && !self.document.value()["DataBindings"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|binding| {
                                binding["TargetArea"] == "UI"
                                    && binding["TargetId"] == widget.id
                                    && binding["PropertyMaps"].as_array().is_some_and(|maps| {
                                        maps.iter().any(|map| map["TargetProperty"] == "Message")
                                    })
                            })
                })
                && from
                    .is_none_or(|from| self.has_binding(from, &format!("ui:{}.Message", widget.id)))
                && self.has_handler(event, &format!("ui:{}.ShowMessage", widget.id))
        });
        if exists {
            return Ok(());
        }
        let mut properties = vec![("MessageType", json!(kind))];
        if let Some(text) = text {
            properties.push(("Message", json!(text)));
        }
        let id = self.add("statusmessage", &properties);
        if let Some(from) = from {
            self.bind(from, &format!("ui:{id}.Message"))?;
        }
        self.on(event, &format!("ui:{id}.ShowMessage"))
    }

    /// A widget under the mashup root, beside the layout, with these properties.
    fn add(&mut self, type_id: &str, properties: &[(&str, Value)]) -> String {
        let widget = widgets::catalog()
            .by_type(type_id)
            .expect("recipes name catalogue widgets; a test checks each");
        let number = self.document.allocate();
        let mut value = mashup::edit::new_widget(widget, number, false);
        let map = value["Properties"]
            .as_object_mut()
            .expect("a widget has properties");
        for (key, property) in properties {
            mashup::edit::insert_sorted(map, key, property.clone());
        }
        let id = map["Id"].as_str().unwrap_or_default().to_string();
        self.document.push_child(&[], value);
        self.added.push(id.clone());
        id
    }

    fn bind(&mut self, from: &str, to: &str) -> Result<(), AuthoringError> {
        match bind_in(
            &mut self.document,
            Some(self.solution),
            &self.context,
            from,
            to,
        ) {
            Ok(bound) => {
                self.wired.push(format!("{from} -> {to}"));
                self.notes.extend(bound.warnings);
            }
            // The wiring a recipe needs is already there: nothing to add.
            Err(AuthoringError::Exists(_)) => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }

    /// An expression into `Disabled` (enable-when) or `Visible` (show-when) of a widget,
    /// evaluated on load and whenever its inputs change.
    fn condition_gate(
        &mut self,
        target: &str,
        enable: bool,
        condition: &str,
        inputs: &[(String, String)],
    ) -> Result<(), AuthoringError> {
        let expression = if enable {
            format!("Output = !({condition});")
        } else {
            format!("Output = ({condition});")
        };
        let id = self.computed("expression2", &expression, Some("BOOLEAN"), inputs, true)?;
        self.bind(
            &format!("ui:{id}.Output"),
            &format!(
                "ui:{target}.{}",
                if enable { "Disabled" } else { "Visible" }
            ),
        )?;
        self.on("mashup:Loaded", &format!("ui:{id}.Evaluate"))
    }

    fn on(&mut self, event: &str, invoke: &str) -> Result<(), AuthoringError> {
        on_in(&mut self.document, &self.context, event, invoke)?;
        self.wired.push(format!("{event} runs {invoke}"));
        Ok(())
    }

    /// The fields (name, base type) of the DataShape a whole service result has, when the
    /// repository knows it: `data:<Entity>.<Service>` with nothing after it.
    fn result_fields(&self, address: &str) -> Option<Vec<(String, String)>> {
        let parsed = parse_address(address).ok()?;
        let crate::core::mashup::address::Address::Data {
            entity_service,
            member: None,
            selected_rows: false,
            ..
        } = parsed
        else {
            return None;
        };
        let (entity, service) =
            super::resolve_service(self.context.model, self.context.platform, &entity_service)
                .ok()?;
        if entity_service != format!("{}.{service}", entity.name) {
            return None;
        }
        let found = crate::core::catalog::lookup_service(
            entity,
            &self.context.model.entities,
            self.context.platform,
            &service,
        )
        .service?;
        let shape_name = found.result.data_shape.as_deref()?;
        let shape = self
            .context
            .model
            .data_shapes
            .iter()
            .find(|s| s.name == shape_name)?;
        Some(
            shape
                .fields
                .iter()
                .map(|f| (f.name.clone(), f.base_type.clone()))
                .collect(),
        )
    }

    /// Resolve the existing binding too, including a named call's underlying service.
    fn bound_fields(&self, widget: &str) -> Option<Vec<(String, String)>> {
        let sections = bindings::data_sections(self.document.value());
        let resolver = bindings::Resolver::new(self.context.model, self.context.platform);
        let mut bindings = bindings::whole_data_bindings(self.document.value())
            .into_iter()
            .filter(|binding| binding.widget == widget);
        let binding = bindings.next()?;
        if bindings.next().is_some() {
            return None;
        }
        let (section, call) = bindings::service_at(&sections, &binding.section, &binding.call)?;
        let service = resolver.service(section, &call.target).service?;
        let shape = resolver.checkable_shape(service.result.data_shape.as_deref()?)?;
        Some(
            shape
                .fields
                .iter()
                .map(|f| (f.name.clone(), f.base_type.clone()))
                .collect(),
        )
    }

    /// Refuse a field the result's DataShape does not have, when the shape is known.
    fn field(
        &self,
        fields: &Option<Vec<(String, String)>>,
        name: &str,
    ) -> Result<String, AuthoringError> {
        match fields {
            Some(fields) => fields
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, base)| base.clone())
                .ok_or_else(|| {
                    AuthoringError::Unknown(format!(
                        "the result has no field {name}; its fields are {}",
                        fields
                            .iter()
                            .map(|(f, _)| f.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }),
            None => Ok(String::new()),
        }
    }

    /// Set properties on a widget already in the layout.
    fn set(&mut self, widget: &mashup::edit::WidgetRef, properties: &[(&str, Value)]) {
        for (key, value) in properties {
            self.document.set_property(&widget.path, key, value.clone());
        }
        let names: Vec<&str> = properties.iter().map(|(key, _)| *key).collect();
        self.wired
            .push(format!("set {} on {}", names.join(", "), widget.id));
    }

    /// The base type an address gives as a binding source.
    fn source_type(&mut self, address: &str) -> Result<String, AuthoringError> {
        let parsed =
            parse_address(address).map_err(|error| AuthoringError::Invalid(error.to_string()))?;
        let end = super::resolve_end(
            &mut self.document,
            Some(self.solution),
            &self.context,
            parsed,
            true,
            &mut Vec::new(),
        )?;
        Ok(super::base(&end).to_string())
    }

    /// An expression or validator whose inputs are fed by these addresses, as `name=address`.
    fn computed(
        &mut self,
        type_id: &str,
        expression: &str,
        output: Option<&str>,
        inputs: &[(String, String)],
        auto_evaluate: bool,
    ) -> Result<String, AuthoringError> {
        let mut definitions = Vec::new();
        for (name, address) in inputs {
            super::validate_parameter(name, "STRING", "IN", None)?;
            if definitions
                .iter()
                .any(|d: &ParameterDefinition| d.name == *name)
            {
                return Err(AuthoringError::Invalid(format!(
                    "input {name} is given twice"
                )));
            }
            let base_type = self.source_type(address)?;
            definitions.push(ParameterDefinition {
                name: name.clone(),
                base_type,
                direction: "IN".into(),
                description: String::new(),
                default: None,
                data_shape: None,
                visible: true,
            });
        }
        let entries: Vec<Value> = definitions
            .iter()
            .map(|def| navigation_definition(def, None))
            .collect();
        let mut properties = vec![
            ("Expression", json!(expression)),
            ("AutoEvaluate", json!(auto_evaluate)),
            ("_currentParameterDefs", Value::Array(entries)),
        ];
        if let Some(output) = output {
            properties.push(("OutputBaseType", json!(output)));
            properties.push(("DataChangeType", json!("VALUE")));
        }
        let id = self.add(type_id, &properties);
        for (name, address) in inputs {
            self.bind(address, &format!("ui:{id}.{name}"))?;
        }
        Ok(id)
    }
}

/// `Output = (...)`, unless the text already assigns Output.
fn assigning(expression: &str) -> String {
    if expression.contains("Output") {
        expression.to_string()
    } else {
        format!("Output = ({expression});")
    }
}

/// A data call's event: `data:E.S` becomes `data:E.S.<event>`.
fn call_event(call: &str, event: &str) -> Result<String, AuthoringError> {
    if !call.starts_with("data:") {
        return Err(AuthoringError::Invalid(format!(
            "{call} is not a data call; give data:<Entity>.<Service>"
        )));
    }
    Ok(format!("{call}.{event}"))
}

/// A widget event: a bare widget means its click.
fn widget_event(trigger: &str) -> String {
    if trigger.starts_with("mashup:") || trigger.starts_with("data:") {
        return trigger.into();
    }
    let rest = trigger.strip_prefix("ui:").unwrap_or(trigger);
    if rest.contains('.') {
        format!("ui:{rest}")
    } else {
        format!("ui:{rest}.Clicked")
    }
}

fn detail_output(document: &MashupDocument, path: &[usize]) -> Value {
    let ids: std::collections::BTreeMap<_, _> = document
        .widgets()
        .into_iter()
        .filter(|w| w.path.starts_with(path))
        .enumerate()
        .map(|(index, w)| (w.id, format!("widget-{index}")))
        .collect();
    fn normalize(value: &mut Value, ids: &std::collections::BTreeMap<String, String>) {
        match value {
            Value::Object(map) => {
                map.remove("Id");
                map.remove("DisplayName");
                for value in map.values_mut() {
                    normalize(value, ids);
                }
            }
            Value::Array(values) => {
                for value in values {
                    normalize(value, ids);
                }
            }
            Value::String(text) => {
                if let Some(id) = ids.get(text) {
                    *text = id.clone();
                }
            }
            _ => {}
        }
    }
    let bindings: Vec<_> = document.value()["DataBindings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| {
            b["TargetId"]
                .as_str()
                .is_some_and(|id| ids.contains_key(id))
        })
        .cloned()
        .collect();
    let styles: std::collections::BTreeMap<_, _> = ids
        .iter()
        .filter_map(|(id, normalized)| {
            document
                .root_properties()
                .get("StyleProperties")
                .and_then(|s| s.get("data"))
                .and_then(|s| s.get(id))
                .map(|style| (normalized, style))
        })
        .collect();
    let mut output = json!({"tree":document.at(path),"bindings":bindings,"styles":styles});
    normalize(&mut output, &ids);
    output
}

fn widget_id(
    document: &MashupDocument,
    word: &str,
) -> Result<mashup::edit::WidgetRef, AuthoringError> {
    document
        .find_exact(word.strip_prefix("ui:").unwrap_or(word))
        .map_err(AuthoringError::Edit)
}

/// Plan one recipe in one mashup. Nothing is written.
pub(crate) fn plan_recipe(
    solution: &Solution,
    request: &RecipeRequest,
) -> Result<RecipePlan, AuthoringError> {
    let loaded = load_existing(solution, &request.mashup)?;
    plan_recipe_loaded(solution, request, loaded, None)
}

pub(super) fn plan_recipe_loaded(
    solution: &Solution,
    request: &RecipeRequest,
    loaded: super::ExistingMashup,
    tokens: Option<&mashup::tokens::Tokens>,
) -> Result<RecipePlan, AuthoringError> {
    if let Recipe::Component {
        container,
        name,
        settings,
    } = &request.recipe
    {
        return mashup::page::compile::component_recipe(
            solution,
            &request.mashup,
            container,
            name,
            settings,
            loaded,
        );
    }
    let document = parse_loaded(&loaded)?;
    let parameters =
        mashup_parameters(&loaded.entity_before).map_err(AuthoringError::Unreadable)?;
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let active;
    let tokens = match tokens {
        Some(tokens) => tokens,
        None => {
            active = mashup::tokens::Tokens::active(solution).map_err(AuthoringError::Invalid)?;
            &active
        }
    };
    let mut work = Work {
        tokens,
        document,
        solution,
        context: AuthoringContext {
            parameters: &parameters,
            model: &model,
            platform: platform.as_ref(),
        },
        added: Vec::new(),
        removed: Vec::new(),
        wired: Vec::new(),
        notes: Vec::new(),
    };
    let mut css = None;
    match &request.recipe {
        Recipe::Component { .. } => unreachable!("component handled above"),
        Recipe::Panel {
            container,
            compact,
            style,
            fit,
        } => {
            let original_css = loaded
                .sidecars
                .as_ref()
                .map(|(_, _, _, bytes)| {
                    crate::core::workspace::without_bom(
                        std::str::from_utf8(bytes).expect("CSS read as UTF-8"),
                    )
                })
                .unwrap_or(&loaded.assets.css);
            css = Some(work.panel(container, *compact, style, fit, original_css)?);
        }
        Recipe::ToolbarBand { container } => work.toolbar_band(container)?,
        Recipe::Card {
            container,
            caption,
            value,
            status,
            kind,
        } => work.card(
            container,
            caption,
            value.as_deref(),
            status.as_deref(),
            kind,
        )?,
        Recipe::SliderAction { slider, button } => work.slider_action(slider, button)?,
        Recipe::Toolbar {
            container,
            then,
            search_param,
            hint,
            width,
            run_on,
            buttons,
        } => {
            work.toolbar(
                container,
                then,
                search_param,
                (hint, *width, run_on),
                buttons,
            )?;
        }
        Recipe::LabelPairs {
            container,
            data,
            fields,
            title,
            columns,
            replace,
            row,
            layout,
            status,
            status_state,
        } => {
            work.label_pairs(
                container,
                data,
                fields,
                (title.as_deref(), *columns, replace.as_deref()),
                (
                    row.as_deref(),
                    layout,
                    status.as_deref(),
                    status_state.as_deref(),
                ),
            )?;
        }
        Recipe::Collapsible {
            container,
            toggle,
            add_toggle_in,
            tab,
            collapsed,
        } => {
            let target = work
                .document
                .find_exact(container)
                .map_err(AuthoringError::Edit)?;
            if target.type_id != "flexcontainer" {
                return Err(AuthoringError::Refused(format!(
                    "{container}: collapsible needs a flexcontainer"
                )));
            }
            let Some((_, parent)) = target.path.split_last() else {
                return Err(AuthoringError::Refused(format!(
                    "{container}: the root cannot collapse; choose a container with a sibling"
                )));
            };
            if !mashup::edit::children(work.document.at(parent))
                .iter()
                .enumerate()
                .any(|(index, sibling)| {
                    Some(&index) != target.path.last()
                        && sibling["Properties"]["Type"] == "flexcontainer"
                })
            {
                return Err(AuthoringError::Refused(format!("{container}: expand/collapse requires a sibling in its parent; add a sibling container first")));
            }
            let invoke = format!("ui:{}.ToggleExpandCollapse", target.id);
            let mut button = toggle.clone();
            let has_tab =
                work.document.at(&target.path)["Properties"]["ShowExpandCollapseTab"] == true;
            if add_toggle_in.is_some() && has_tab {
                work.notes.push(format!(
                    "{container}: reusing its existing expand/collapse tab; no button added"
                ));
            }
            if let Some(destination) = add_toggle_in.as_ref().filter(|_| !has_tab) {
                let header = work
                    .document
                    .find_exact(destination)
                    .map_err(AuthoringError::Edit)?;
                if header.type_id != "flexcontainer" {
                    return Err(AuthoringError::Refused(format!(
                        "{destination}: --in needs a flexcontainer"
                    )));
                }
                if header.path.starts_with(&target.path) {
                    return Err(AuthoringError::Refused(format!("{destination}: the toggle must be outside {container}, or it vanishes when the container collapses")));
                }
                button = work
                    .document
                    .widgets()
                    .iter()
                    .find(|widget| {
                        widget.type_id == "ptcsbutton"
                            && widget.path.starts_with(&header.path)
                            && work.has_handler(&format!("ui:{}.Clicked", widget.id), &invoke)
                    })
                    .map(|widget| widget.id.clone());
                if button.is_none() {
                    let contract = widgets::catalog()
                        .by_type("ptcsbutton")
                        .expect("button catalogue");
                    let placed = work
                        .document
                        .place(contract, &mashup::edit::Placement::In(header.id))
                        .map_err(AuthoringError::Edit)?;
                    let found = work
                        .document
                        .find_exact(&placed.id)
                        .map_err(AuthoringError::Edit)?;
                    if contract.property("Icon").is_some() {
                        work.document.set_property(
                            &found.path,
                            "Icon",
                            json!("cds:icon_hamburger"),
                        );
                        work.document.set_property(&found.path, "Label", json!(""));
                    } else {
                        work.document
                            .set_property(&found.path, "Label", json!("Menu"));
                    }
                    work.added.push(placed.id.clone());
                    button = Some(placed.id);
                }
            }
            if let Some(button) = button {
                let button = work
                    .document
                    .find_exact(&button)
                    .map_err(AuthoringError::Edit)?;
                if button.type_id != "ptcsbutton" {
                    return Err(AuthoringError::Refused(format!(
                        "{}: --toggle needs a ptcsbutton",
                        button.id
                    )));
                }
                let target_path = work
                    .document
                    .find_exact(&target.id)
                    .map_err(AuthoringError::Edit)?
                    .path;
                if button.path.starts_with(&target_path) {
                    return Err(AuthoringError::Refused(format!("{}: the toggle must be outside {container}, or it vanishes when the container collapses", button.id)));
                }
                let event = format!("ui:{}.Clicked", button.id);
                if !work.has_handler(&event, &invoke) {
                    work.on(&event, &invoke)?;
                }
            }
            let target = work
                .document
                .find_exact(&target.id)
                .map_err(AuthoringError::Edit)?;
            for (name, value) in [
                ("EnableExpandCollapse", json!(true)),
                ("Expanded", json!(!collapsed)),
                (
                    "ShowExpandCollapseTab",
                    json!(tab != "none" || (has_tab && add_toggle_in.is_some())),
                ),
            ] {
                work.document.set_property(&target.path, name, value);
            }
            if tab != "none" {
                for (name, value) in [
                    ("ExpandCollapseTabLocation", tab.as_str()),
                    ("ExpandTabIcon", "cds:icon_chevron_right"),
                    ("CollapseTabIcon", "cds:icon_chevron_left"),
                ] {
                    work.document.set_property(&target.path, name, json!(value));
                }
            }
            // A nested menu that is not always open renders an empty rail when its column is narrow.
            let bars: Vec<_> = work
                .document
                .widgets()
                .into_iter()
                .filter(|bar| bar.type_id == "ptcsmenubar" && bar.path.starts_with(&target.path))
                .collect();
            for bar in bars {
                let properties = &work.document.at(&bar.path)["Properties"];
                if properties["SubMenuType"] == "nested" && properties["AlwaysOpen"] != true {
                    work.set(&bar, &[("AlwaysOpen", json!(true))]);
                    work.notes.push(format!(
                        "{} is a nested menu; AlwaysOpen is now true so it keeps its labels in the column that collapses",
                        bar.id
                    ));
                }
            }
        }
        Recipe::EnableWhen {
            widget,
            condition,
            inputs,
        }
        | Recipe::ShowWhen {
            widget,
            condition,
            inputs,
        } => {
            let target = widget_id(&work.document, widget)?;
            let enable = matches!(request.recipe, Recipe::EnableWhen { .. });
            work.condition_gate(&target.id, enable, condition, inputs)?;
        }
        Recipe::Text {
            widget,
            expression,
            inputs,
        } => {
            let target = widget_id(&work.document, widget)?;
            let catalog = widgets::catalog().by_type(&target.type_id);
            let property = ["LabelText", "Label", "Text", "Title"]
                .into_iter()
                .find(|name| catalog.is_some_and(|w| w.defaults.contains_key(*name)))
                .ok_or_else(|| {
                    AuthoringError::Invalid(format!("{} has no text property", target.id))
                })?;
            let id = work.computed(
                "expression2",
                &assigning(expression),
                Some("STRING"),
                inputs,
                true,
            )?;
            work.bind(
                &format!("ui:{id}.Output"),
                &format!("ui:{}.{property}", target.id),
            )?;
            work.on("mashup:Loaded", &format!("ui:{id}.Evaluate"))?;
        }
        Recipe::SetOnClick {
            trigger,
            parameter,
            expression,
            inputs,
        } => {
            let destination = format!("mashup:{parameter}");
            let base = work.source_type(&destination)?;
            if work.has_set_on_click(
                &widget_event(trigger),
                &destination,
                &assigning(expression),
                &base,
                inputs,
            ) {
                work.notes.push("set-on-click wiring already exists".into());
            } else {
                let id = work.computed(
                    "expression2",
                    &assigning(expression),
                    Some(&base),
                    inputs,
                    false,
                )?;
                let expression_widget = widget_id(&work.document, &id)?;
                work.set(
                    &expression_widget,
                    &[
                        ("AutoEvaluate", json!(false)),
                        ("DisableInitialOutput", json!(true)),
                        ("DataChangeType", json!("ALWAYS")),
                    ],
                );
                work.bind(&format!("ui:{id}.Output"), &destination)?;
                work.on(&widget_event(trigger), &format!("ui:{id}.Evaluate"))?;
            }
        }
        Recipe::Tracker {
            widget,
            data,
            field_maps,
            initial_step,
            step_size,
            host,
        } => {
            // Resolve and validate every map before creating a visual widget.
            let fields = work.result_fields(data).ok_or_else(|| AuthoringError::Invalid(
                format!("tracker needs {data} to return a DataShape available in the repository; add its DataShape before mapping {}", TRACKER_MAPS.iter().map(|(name, _, _)| *name).collect::<Vec<_>>().join(", "))
            ))?;
            let maps = tracker_maps(&fields, field_maps)?;
            let target = if widget == "new" {
                let catalog = widgets::catalog()
                    .by_type("ptcsprogresstracker")
                    .expect("catalogue tracker");
                let placed = work
                    .document
                    .place(catalog, &mashup::edit::Placement::In("center".into()))
                    .map_err(AuthoringError::Edit)?;
                work.added.push(placed.id.clone());
                widget_id(&work.document, &placed.id)?
            } else {
                widget_id(&work.document, widget)?
            };
            if target.type_id != "ptcsprogresstracker" {
                return Err(AuthoringError::Invalid(format!(
                    "{} is a {}, not a progress tracker",
                    target.id, target.type_id
                )));
            }
            if !["small", "medium", "large"].contains(&step_size.as_str()) {
                return Err(AuthoringError::Invalid(
                    "tracker: --step-size is small, medium or large".into(),
                ));
            }
            if work.document.at_mut(&target.path)["Properties"]
                .as_object_mut()
                .unwrap()
                .remove("CurrentStepMashup")
                .is_some()
            {
                work.notes.push(format!("{}: removed stored CurrentStepMashup output; initialise selection through CurrentStepNumber", target.id));
            }
            let mut properties = vec![
                ("IsInteractive", json!(true)),
                ("StepSize", json!(step_size)),
                ("MinStepSpacing", json!(24)),
                ("CurrentStepNumber", json!(initial_step)),
            ];
            properties.extend(maps.iter().map(|(name, field)| (*name, json!(field))));
            work.set(&target, &properties);
            work.notes.push(format!("tracker initial CurrentStepNumber={initial_step}; StepSize={step_size}; MinStepSpacing=24; initial selection and narrow-width fit require runtime verification"));
            if let Some(host) = host {
                let container = widget_id(&work.document, host)?;
                if container.type_id != "mashupcontainer" {
                    return Err(AuthoringError::Invalid(format!(
                        "{}: --host needs a mashupcontainer",
                        container.id
                    )));
                }
                work.set(&container, &[("stepNumber", json!(initial_step))]);
                work.bind(
                    &format!("ui:{}.stepNumber", container.id),
                    &format!("ui:{}.CurrentStepNumber", target.id),
                )?;
                work.bind(
                    &format!("ui:{}.CurrentStepNumber", target.id),
                    &format!("ui:{}.stepNumber", container.id),
                )?;
                work.bind(
                    &format!("ui:{}.CurrentStepMashup", target.id),
                    &format!("ui:{}.Name", container.id),
                )?;
                work.notes.push("host binding order: host.stepNumber -> tracker.CurrentStepNumber; tracker.CurrentStepNumber -> host.stepNumber; tracker.CurrentStepMashup -> host.Name; Data last. Keep the host's default first-step Name; child stepNumber must be a bindable NUMBER parameter".into());
            }
            work.bind(data, &format!("ui:{}.Data", target.id))?;
        }
        Recipe::Tree {
            widget,
            data,
            field_maps,
        } => {
            let fields = work.result_fields(data).ok_or_else(|| AuthoringError::Invalid(format!("tree needs {data} to return a DataShape available in the repository; add its DataShape before mapping fields")))?;
            let maps = field_maps_for("tree", TREE_MAPS, TREE_REQUIRED, &fields, field_maps)?;
            let target = widget_id(&work.document, widget)?;
            if target.type_id != "ptcstreeselector" {
                return Err(AuthoringError::Invalid(format!(
                    "{} is a {}, not a tree selector",
                    target.id, target.type_id
                )));
            }
            work.set(
                &target,
                &maps
                    .iter()
                    .map(|(name, field)| (*name, json!(field)))
                    .collect::<Vec<_>>(),
            );
            work.bind(data, &format!("ui:{}.Data", target.id))?;
            work.notes.push(format!("tree: bind ui:{}.SelectedItem to an ID parameter, or SelectedData fields to detail widgets; run the data call on load. Empty results show the widget's NoData state", target.id));
        }
        Recipe::Message {
            after,
            text,
            from,
            kind,
            on_failure,
        } => {
            if !matches!(kind.as_str(), "info" | "warning" | "error") {
                return Err(AuthoringError::Invalid(
                    "the message type is info, warning or error".into(),
                ));
            }
            if text.is_some() && from.is_some()
                || text.is_none() && from.is_none() && on_failure.is_none()
            {
                return Err(AuthoringError::Invalid(
                    "give a success message as --say or --from, and/or an error message as --on-failure".into(),
                ));
            }
            if text.is_some() || from.is_some() {
                work.message(
                    &call_event(after, "ServiceInvokeCompleted")?,
                    text.as_deref(),
                    from.as_deref(),
                    kind,
                )?;
            }
            if let Some(text) = on_failure {
                if text.trim().is_empty() {
                    return Err(AuthoringError::Invalid(
                        "--on-failure needs nonempty error text".into(),
                    ));
                }
                work.message(&call_event(after, "Data_Error")?, Some(text), None, "error")?;
            }
        }
        Recipe::Confirm {
            trigger,
            then,
            title,
            message,
            action_label,
            cancel_label,
        } => {
            let mut properties = Vec::new();
            if let Some(title) = title {
                properties.push(("TitleText", json!(title)));
            }
            if let Some(message) = message {
                properties.push(("MessageText", json!(message)));
            }
            if let Some(label) = action_label {
                properties.push(("ActionButtonLabel", json!(label)));
            }
            if let Some(label) = cancel_label {
                properties.push(("CancelButtonLabel", json!(label)));
            }
            let id = work.add("ptcsconfirmation", &properties);
            work.on(&widget_event(trigger), &format!("ui:{id}.OpenConfirmation"))?;
            work.on(&format!("ui:{id}.ActionClick"), then)?;
        }
        Recipe::Popup {
            trigger,
            target,
            window,
            size,
            title,
            pass,
            refresh,
        } => {
            if !matches!(window.as_str(), "modal-popup" | "popup" | "replace" | "new") {
                return Err(AuthoringError::Invalid(
                    "the window is modal-popup, popup, replace or new".into(),
                ));
            }
            if *target == request.mashup {
                return Err(AuthoringError::Refused(
                    "a mashup cannot open itself".into(),
                ));
            }
            let child = load_existing(solution, target)?;
            let child_document = parse_loaded(&child)?;
            let definitions = contained_parameters(&child, &child_document)?;
            let cache: Vec<Value> = definitions
                .iter()
                .map(|def| cache_entry(def, def.default.clone()))
                .collect();
            let own: Vec<Value> = definitions
                .iter()
                .map(|def| navigation_definition(def, def.default.as_ref()))
                .collect();
            let (width, height) = size.unwrap_or((0, 0));
            let mut properties = vec![
                ("TargetMashup", json!(target)),
                ("TargetWindowType", json!(window)),
                ("PopupWidth", json!(width)),
                ("PopupHeight", json!(height)),
                ("MashupParameters", Value::Array(cache)),
                ("_currentParameterDefs", Value::Array(own)),
            ];
            if let Some(title) = title {
                properties.push(("PopupTitle", json!(title)));
            }
            let id = work.add("navigationfunction", &properties);
            for (name, address) in pass {
                work.bind(address, &format!("ui:{id}.{name}"))?;
            }
            work.on(&widget_event(trigger), &format!("ui:{id}.Navigate"))?;
            if let Some(refresh) = refresh {
                work.on(&format!("ui:{id}.WindowClosed"), refresh)?;
            }
        }
        Recipe::CloseAfter { after } => {
            work.on(
                &call_event(after, "ServiceInvokeCompleted")?,
                "mashup:CloseIfPopup",
            )?;
        }
        Recipe::AutoRefresh {
            call,
            every,
            interval,
        } => {
            if every.is_some() == interval.is_some() {
                return Err(AuthoringError::Invalid(
                    "give --every <seconds>, or bind the interval with --interval".into(),
                ));
            }
            let mut properties = vec![("AutoRefresh", json!(true))];
            if let Some(every) = every {
                properties.push(("RefreshInterval", json!(every)));
            }
            let id = work.add("autorefreshfunction", &properties);
            if let Some(interval) = interval {
                work.bind(interval, &format!("ui:{id}.RefreshInterval"))?;
            }
            work.on(&format!("ui:{id}.Refresh"), call)?;
        }
        Recipe::Sparkline {
            container,
            data,
            x,
            value,
        } => {
            let target = widget_id(&work.document, container)?;
            if target.type_id != "flexcontainer" {
                return Err(AuthoringError::Invalid(format!(
                    "{container}: sparkline needs a responsive container"
                )));
            }
            let fields = work.result_fields(data);
            work.field(&fields, x)?;
            let base = work.field(&fields, value)?;
            // An unknown result type (or a malformed address) is reported by the bind below.
            if !base.is_empty() && !["NUMBER", "INTEGER", "LONG"].contains(&base.as_str()) {
                return Err(AuthoringError::Invalid(format!(
                    "sparkline field {value} must be numeric"
                )));
            }
            let props = sparkline_props(x, value);
            let props: Vec<_> = props.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            let wrapper = work.add_in(&target.path, "flexcontainer", &[]);
            let wrapper = widget_id(&work.document, &wrapper)?;
            let node = layout_rules::context::Node::op(
                "sparkline-height",
                layout_rules::Element::Container,
            );
            for (key, value) in layout_rules::RuleSet::embedded(layout_rules::Version::V1)
                .evaluate(&node.context())
                .patch()
            {
                work.document.set_property(&wrapper.path, key, value);
            }
            let id = work.add_in(&wrapper.path, "ptcschartline", &props);
            let chart = widget_id(&work.document, &id)?;
            work.document
                .set_property(&chart.path, "ResponsiveLayout", json!(true));
            work.document.at_mut(&chart.path)["Properties"]
                .as_object_mut()
                .unwrap()
                .remove("Width");
            super::set::copy_style_entry(
                &mut work.document,
                &id,
                sparkline_style(),
                None,
                &mut vec![],
            );
            work.bind(data, &format!("ui:{id}.Data"))?;
        }
        Recipe::Chart {
            series: configured,
            widget,
            data,
            x,
            value,
            kind,
            title,
            x_title,
            y_title,
            stacked,
        } => {
            let target = widget_id(&work.document, widget)?;
            if target.type_id == "ptcschartschedule" {
                return Err(AuthoringError::Invalid("schedule uses nested resource intervals; configure DataField and ResourceField with mashup set and bind the nested Data explicitly".into()));
            }
            let pie = matches!(target.type_id.as_str(), "ptcschartpie");
            if !target.type_id.starts_with("ptcschart") {
                return Err(AuthoringError::Invalid(format!(
                    "{} is a {}, not a chart",
                    target.id, target.type_id
                )));
            }
            if value.is_some() != pie {
                return Err(AuthoringError::Invalid(if pie {
                    "a pie chart needs --value <field> beside --x <label field>".into()
                } else {
                    "--value is for a pie chart; other charts take every other numeric column as a series".into()
                }));
            }
            let fields = work.result_fields(data);
            work.field(&fields, x)?;
            // A combo chart draws only the series it names (DataField1..N on every kit combo);
            // the other charts take each numeric column on their own.
            let series: Vec<(String, String)> = if target.type_id == "ptcschartcombo" {
                fields
                    .iter()
                    .flatten()
                    .filter(|(name, base)| {
                        name != x && matches!(base.as_str(), "NUMBER" | "INTEGER" | "LONG")
                    })
                    .enumerate()
                    .map(|(index, (name, _))| (format!("DataField{}", index + 1), name.clone()))
                    .collect()
            } else {
                Vec::new()
            };
            if target.type_id == "ptcschartcombo" && fields.is_some() && series.is_empty() {
                return Err(AuthoringError::Invalid(format!(
                    "{data} has no numeric field besides {x} for the combo chart to draw"
                )));
            }
            // Series a previous run named beyond this run's count are cleared, so a field that
            // is now the x axis is not still drawn as a series.
            let surplus: Vec<String> = if target.type_id == "ptcschartcombo" {
                let current = &work.document.at(&target.path)["Properties"];
                (series.len() + 1..=MAX_COMBO_SERIES)
                    .map(|index| format!("DataField{index}"))
                    .filter(|key| {
                        current[key.as_str()]
                            .as_str()
                            .is_some_and(|field| !field.is_empty())
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let mut properties = Vec::new();
            if *stacked {
                if target.type_id != "ptcschartbar" {
                    return Err(AuthoringError::Invalid(format!(
                        "{}: --stacked needs a bar chart",
                        target.id
                    )));
                }
                properties.push(("SeriesDisplay", json!("auto")));
            }
            for (key, field) in &series {
                properties.push((key.as_str(), json!(field)));
            }
            for key in &surplus {
                properties.push((key.as_str(), json!("")));
            }
            if pie {
                let value = value.as_deref().expect("checked above");
                work.field(&fields, value)?;
                properties.push(("DataField", json!(x)));
                properties.push(("ValueField", json!(value)));
            } else {
                properties.push(("XAxisField", json!(x)));
            }
            if let Some(kind) = kind {
                let allowed: &[&str] = match target.type_id.as_str() {
                    "ptcschartbar" => &["bar", "column"],
                    "ptcschartline" => &["linechart", "areachart"],
                    _ => &[],
                };
                if !allowed.contains(&kind.as_str()) {
                    return Err(AuthoringError::Invalid(format!(
                        "{} takes the chart type {}",
                        target.type_id,
                        if allowed.is_empty() {
                            "it has".to_string()
                        } else {
                            allowed.join(" or ")
                        }
                    )));
                }
                properties.push(("ChartType", json!(kind)));
            }
            let catalog = widgets::catalog()
                .by_type(&target.type_id)
                .expect("catalogue chart");
            for (key, default) in &catalog.defaults {
                let y_axis =
                    key == "YAxisLabel" || (key.starts_with("YAxis") && key.ends_with("Label"));
                let explicit = if key == "Label" {
                    title.as_ref()
                } else if key == "XAxisLabel" {
                    x_title.as_ref()
                } else if y_axis {
                    y_title.as_ref()
                } else {
                    None
                };
                if key == "Label" || key == "XAxisLabel" || y_axis || key == "SecondYAxisLabel" {
                    let current = &work.document.at(&target.path)["Properties"][key];
                    if let Some(text) = explicit {
                        properties.push((key.as_str(), json!(text)));
                    } else if current == default {
                        properties.push((key.as_str(), json!("")));
                    }
                }
            }
            work.set(&target, &properties);
            if !configured.is_empty() {
                if target.type_id != "ptcschartcombo" {
                    return Err(AuthoringError::Invalid(
                        "--series needs a combo chart".into(),
                    ));
                }
                let mut selected = Vec::new();
                for text in configured {
                    let parts: Vec<_> = text.split(':').collect();
                    if !(2..=3).contains(&parts.len())
                        || parts[0].is_empty()
                        || !["bar", "line"].contains(&parts[1])
                        || parts
                            .get(2)
                            .is_some_and(|a| !["primary", "secondary"].contains(a))
                    {
                        return Err(AuthoringError::Invalid(format!(
                            "--series {text:?} must be field:bar|line[:primary|secondary]"
                        )));
                    }
                    let base = work.field(&fields, parts[0])?;
                    if !["NUMBER", "INTEGER", "LONG"].contains(&base.as_str()) {
                        return Err(AuthoringError::Invalid(format!(
                            "--series {} must be numeric",
                            parts[0]
                        )));
                    }
                    selected.push((
                        parts[0].to_string(),
                        parts[1].to_string(),
                        None,
                        parts.get(2).map(|a| a.to_string()),
                    ));
                }
                if selected.len() > MAX_COMBO_SERIES {
                    return Err(AuthoringError::Invalid(
                        "--series supports at most 24 series".into(),
                    ));
                }
                let props = combo_props(&selected, &work.document.at(&target.path)["Properties"]);
                work.set(
                    &target,
                    &props
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.clone()))
                        .collect::<Vec<_>>(),
                );
            }
            if work.document.rules().version() == "v1" {
                let count = if !configured.is_empty() {
                    configured.len()
                } else {
                    fields.as_ref().map_or(2, |fields| {
                        fields
                            .iter()
                            .filter(|(name, base)| {
                                name != x && matches!(base.as_str(), "NUMBER" | "INTEGER" | "LONG")
                            })
                            .count()
                    })
                };
                let mut defaults = vec![
                    ("LegendPosition", json!("top")),
                    ("LegendAlignment", json!("end")),
                    ("HideLegend", json!(!pie && count == 1)),
                ];
                if pie {
                    defaults.extend([
                        ("ShowValues", json!(true)),
                        ("ValuesPosition", json!("marker")),
                    ]);
                }
                if target.type_id == "ptcschartbar" {
                    defaults.extend([
                        ("HideValues", json!(false)),
                        ("ValuesPosition", json!("inside")),
                        ("ValuesAlignment", json!("end")),
                    ]);
                }
                work.set(&target, &defaults);
                if pie && !work.tokens.chart_palette().is_empty() {
                    let styles: Vec<_> = work
                        .tokens
                        .chart_palette()
                        .iter()
                        .enumerate()
                        .flat_map(|(i, color)| {
                            [
                                format!("slice@serie{}:fill={color}", i + 1),
                                format!("legend-marker@serie{}:background={color}", i + 1),
                            ]
                        })
                        .collect();
                    super::set::merge_widget_styles(
                        &mut work.document,
                        &target.id,
                        &target.type_id,
                        &styles,
                        "mashup",
                    )?;
                }
            }
            work.bind(data, &format!("ui:{}.Data", target.id))?;
        }
        Recipe::Columns {
            auto_select_first,
            widget,
            data,
            columns,
            date_format,
            editable,
            column_editors,
            preserve_whitespace,
            status,
        } => {
            let target = widget_id(&work.document, widget)?;
            if target.type_id != "ptcsgrid" {
                return Err(AuthoringError::Invalid(format!(
                    "{} is a {}, not a grid",
                    target.id, target.type_id
                )));
            }
            if *auto_select_first {
                work.set(
                    &target,
                    &[
                        ("DefaultSelectedRows", json!("1")),
                        ("RowSelection", json!("single")),
                    ],
                );
            }
            if columns.is_empty() {
                return Err(AuthoringError::Invalid(
                    "columns needs at least one --column".into(),
                ));
            }
            let status_format = status
                .as_ref()
                .map(|(field, name)| {
                    if !columns
                        .iter()
                        .any(|column| column.split(':').next() == Some(field))
                    {
                        return Err(AuthoringError::Invalid(format!(
                            "--status {field}={name}: list {field} with --column first"
                        )));
                    }
                    let file = crate::core::workspace::entities(solution)
                        .into_iter()
                        .find(|file| {
                            file.info.collection == "StateDefinitions" && file.info.name == *name
                        })
                        .ok_or_else(|| {
                            AuthoringError::Unknown(format!(
                                "no StateDefinition {name}; add it to the solution first"
                            ))
                        })?;
                    let bytes = std::fs::read(&file.path).map_err(|error| {
                        AuthoringError::Unreadable(format!("{}: {error}", file.path.display()))
                    })?;
                    let definition = super::state_format::read_definition(&bytes)?;
                    super::state_format::inline(&definition, name, field)
                })
                .transpose()?;
            let previous_fields = work.bound_fields(&target.id);
            if let Some(data) = data {
                work.bind(data, &format!("ui:{}.Data", target.id))?;
            }
            // Without a known DataShape the listed columns are written as given and the grid
            // keeps showing the fields it is not told about.
            let fields = data
                .as_deref()
                .and_then(|address| work.result_fields(address))
                .or_else(|| work.bound_fields(&target.id));
            if fields.is_none()
                && !bindings::whole_data_bindings(work.document.value())
                    .iter()
                    .any(|binding| binding.widget == target.id)
            {
                return Err(AuthoringError::Invalid(format!(
                    "columns for {} needs the grid's data; bind Data or give --data <data call>",
                    target.id
                )));
            }
            let existing = work.document.at(&target.path)["Properties"]["ColumnFormat"].clone();
            let existing_columns = existing["formatInfo"].as_array();
            let known = fields.clone();
            let mut format = Vec::new();
            for text in columns {
                let mut parts = text.splitn(4, ':');
                let name = parts.next().unwrap_or_default().trim().to_string();
                if name.is_empty() {
                    return Err(AuthoringError::Invalid(format!(
                        "--column {text:?} must be field[:Title[:width[:format]]]"
                    )));
                }
                let title = parts
                    .next()
                    .filter(|t| !t.is_empty())
                    .unwrap_or(&name)
                    .to_string();
                let width = match parts.next() {
                    None | Some("") => json!("auto"),
                    Some(w) => {
                        json!(w
                            .trim()
                            .parse::<u32>()
                            .map_err(|_| AuthoringError::Invalid(format!(
                                "--column {text:?}: the width is a number of pixels"
                            )))?)
                    }
                };
                if format
                    .iter()
                    .any(|column: &Value| column["FieldName"] == name)
                {
                    return Err(AuthoringError::Invalid(format!(
                        "--column repeats field {name}; list each field once"
                    )));
                }
                let base = work.field(&known, &name)?;
                let mut column = existing_columns
                    .and_then(|columns| columns.iter().find(|c| c["FieldName"] == name))
                    .cloned()
                    .unwrap_or_else(|| column_format(&name, &title, width.clone(), &base));
                refresh_column_type(&mut column, previous_fields.as_deref(), &name, &base);
                column["Title"] = json!(title);
                column["Width"] = width;
                column["inLayout"] = json!(true);
                column["hidden"] = json!(false);
                let per_column_format = parts.next().filter(|pattern| !pattern.is_empty());
                if let Some(pattern) =
                    per_column_format.or(date_format.as_deref().filter(|_| base == "DATETIME"))
                {
                    if base != "DATETIME" || pattern.trim().is_empty() {
                        return Err(AuthoringError::Invalid(format!("--column {name}: date format needs a DATETIME field and a non-empty pattern")));
                    }
                    column["FormatOptions"]["FormatString"] = json!(pattern);
                }
                if base == "DATETIME" && date_format.is_none() && per_column_format.is_none() {
                    work.notes.push(format!("{name}: choose a date presentation with twaco mashup recipe columns {} {} --column {name} --date-format dd-MM-yyyy", request.mashup, target.id));
                }
                if status.as_ref().is_some_and(|(field, _)| field == &name) {
                    let snapshot = status_format.as_ref().expect("status snapshot");
                    if snapshot["StateDefinitionType"] == "numeric"
                        && !matches!(base.as_str(), "NUMBER" | "INTEGER" | "LONG")
                    {
                        return Err(AuthoringError::Invalid(format!("--status {name}: numeric StateDefinition requires a numeric field, found {base}")));
                    }
                    column["FormatOptions"]["formatInfo"] = snapshot.clone();
                    column["FormatOptions"]["FormatString"] = json!("full");
                }
                let normalized = name.to_ascii_lowercase();
                if (normalized.contains("status")
                    || normalized.contains("state")
                    || normalized.contains("severity"))
                    && column["FormatOptions"]["formatInfo"]["StateDefinition"]
                        .as_str()
                        .is_none_or(str::is_empty)
                    && column["FormatOptions"]["formatInfo"]["StateFormats"]
                        .as_array()
                        .is_none_or(Vec::is_empty)
                {
                    work.notes.push(format!("{name}: add status icons with twaco mashup state-format set {} {} --column {name} --state <StateDefinition>", request.mashup, target.id));
                }
                format.push(column);
            }
            for name in editable {
                let column = format
                    .iter_mut()
                    .find(|column| column["FieldName"] == *name)
                    .ok_or_else(|| {
                        AuthoringError::Invalid(format!(
                            "--editable-column {name} is not one of the --column fields"
                        ))
                    })?;
                column["AllowEdit"] = json!(true);
                column["editable"] = json!(true);
            }
            for (name, editor) in column_editors {
                let column = format
                    .iter_mut()
                    .find(|column| column["FieldName"] == *name)
                    .ok_or_else(|| {
                        AuthoringError::Invalid(format!(
                            "--column-editor {name} is not one of the --column fields"
                        ))
                    })?;
                let base = work.field(&known, name)?;
                let allowed = column_editors_for(&base);
                if editor == "dropdown" {
                    return Err(AuthoringError::Invalid(format!(
                        "--column-editor {name}=dropdown: dropdown columns need a source and are not supported by this recipe"
                    )));
                }
                if !allowed.contains(&editor.as_str()) {
                    return Err(AuthoringError::Invalid(format!(
                        "--column-editor {name}={editor}: field {name} has base type {base}; allowed editors: {}",
                        allowed
                            .iter()
                            .filter(|editor| **editor != "dropdown")
                            .copied()
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
                column["cellEditor"] = json!(editor);
                if !editable.contains(name) {
                    work.notes.push(format!(
                        "{name}: cell editor {editor} is configured but the column is not editable; use --editable-column {name}"
                    ));
                }
            }
            for name in preserve_whitespace {
                let column = format
                    .iter_mut()
                    .find(|column| column["FieldName"] == *name)
                    .ok_or_else(|| {
                        AuthoringError::Invalid(format!(
                            "--preserve-whitespace {name} is not one of the --column fields"
                        ))
                    })?;
                column["preserveWhiteSpaces"] = json!(true);
                if editable.contains(name) && column["cellEditor"] != "textarea" {
                    work.notes.push(format!(
                        "{name}: newlines are kept when shown but not while editing; use --column-editor {name}=textarea"
                    ));
                }
            }
            // Composer keeps all fields in the configuration, even those outside the layout.
            for (name, base) in fields.iter().flatten() {
                if !format.iter().any(|column| column["FieldName"] == *name) {
                    let mut column = existing_columns
                        .and_then(|columns| columns.iter().find(|c| c["FieldName"] == *name))
                        .cloned()
                        .unwrap_or_else(|| column_format(name, name, json!("auto"), base));
                    refresh_column_type(&mut column, previous_fields.as_deref(), name, base);
                    column["inLayout"] = json!(false);
                    column["hidden"] = json!(true);
                    format.push(column);
                }
            }
            if fields.is_none() {
                for column in existing_columns.into_iter().flatten() {
                    if !format.iter().any(|c| c["FieldName"] == column["FieldName"]) {
                        format.push(column.clone());
                    }
                }
            }
            let mut configuration = if existing.is_object() {
                existing
            } else {
                json!({})
            };
            configuration["formatInfo"] = json!(format);
            work.set(&target, &[("ColumnFormat", configuration)]);
            super::set::style_properties(work.document.root_properties(), &request.mashup)?;
            let mut style = work
                .document
                .root_properties()
                .get("StyleProperties")
                .and_then(|styles| styles.get("data"))
                .and_then(|data| data.get(&target.id))
                .cloned()
                .unwrap_or(Value::Null);
            if style.is_null() {
                style = json!({"parts":{"$":{"$":{}}},"widget":"ptcs-grid"});
            }
            if !style["parts"]["$"]["$"].is_object() {
                return Err(AuthoringError::Refused(format!(
                    "{}: malformed grid style; repair StyleProperties first",
                    target.id
                )));
            }
            style["parts"]["$"]["$"]["--ptcs-header-vert-divider-width"] = json!("0px");
            if work.document.rules().version() == "v1" {
                if style["parts"]["core-grid"].is_null() {
                    style["parts"]["core-grid"] = json!({"$":{}});
                }
                if !style["parts"]["core-grid"]["$"].is_object() {
                    return Err(AuthoringError::Refused(format!(
                        "{}: malformed core-grid style; repair StyleProperties first",
                        target.id
                    )));
                }
                style["parts"]["core-grid"]["$"]["border-bottom-style"] = json!("none");
                style["parts"]["core-grid"]["$"]["border-bottom-width"] = json!("0px");
            }

            super::set::copy_style_entry(
                &mut work.document,
                &target.id,
                style,
                None,
                &mut Vec::new(),
            );
            // A column is only editable in Runtime when its grid is.
            if !editable.is_empty() {
                work.set(&target, &[("IsEditable", json!(true))]);
                work.notes.push(format!(
                    "{}.IsEditable set for its editable columns",
                    target.id
                ));
            }
        }
        Recipe::Pagination { widget, data } => work.pagination(widget, data)?,
        Recipe::Picker {
            widget,
            data,
            display,
            value,
            selected,
            no_none_row,
            auto_select_first,
            multiple,
        } => {
            let target = widget_id(&work.document, widget)?;
            if !matches!(target.type_id.as_str(), "ptcsdropdown" | "ptcslist") {
                return Err(AuthoringError::Invalid(format!(
                    "{} is a {}, not a dropdown or list",
                    target.id, target.type_id
                )));
            }
            let fields = work.result_fields(data);
            let display_base = work.field(&fields, display)?;
            let mut properties = vec![("DisplayField", json!(display))];
            if !display_base.is_empty() {
                properties.push(("DisplayFieldBaseType", json!(display_base)));
            }
            if let Some(value) = value {
                work.field(&fields, value)?;
                properties.push(("ValueField", json!(value)));
            }
            if *no_none_row {
                properties.push(("ClearSelectionItem", json!(false)));
            }
            if *auto_select_first {
                properties.push(("AutoSelectFirstRow", json!(true)));
            }
            if *multiple {
                if target.type_id != "ptcslist" {
                    return Err(AuthoringError::Invalid(format!(
                        "{} is a {}; --multiple needs a list",
                        target.id, target.type_id
                    )));
                }
                properties.push(("MultiSelect", json!(true)));
            }
            work.set(&target, &properties);
            work.bind(data, &format!("ui:{}.Data", target.id))?;
            if let Some(selected) = selected {
                work.bind(selected, &format!("ui:{}.SelectedText", target.id))?;
            }
        }
        Recipe::SidePanel {
            container,
            title,
            info,
            tab,
        } => work.side_panel(container, title, info.as_deref(), tab)?,
        Recipe::EmptyState { widget, text, data } => {
            work.empty_state(widget, text, data.as_deref())?
        }
        Recipe::Section {
            container,
            title,
            center,
        } => work.section(container, title, *center)?,
        Recipe::CardGrid {
            container,
            per_row,
            cells,
        } => work.card_grid(container, *per_row, *cells)?,
        Recipe::InputVariant { widget, variant } => work.input_variant(widget, variant)?,
        Recipe::NestedMenu {
            menubar,
            menu,
            auto_collapse,
            width,
        } => {
            let target = widget_id(&work.document, menubar)?;
            if target.type_id != "ptcsmenubar" {
                return Err(AuthoringError::Invalid(format!(
                    "{} is a {}, not a menu bar",
                    target.id, target.type_id
                )));
            }
            match crate::core::entity::edit::menu_links(solution, menu)
                .map_err(|error| AuthoringError::Invalid(error.to_string()))?
            {
                Some(links) if !links.iter().any(|(kind, _)| kind == "Menu") => {
                    return Err(AuthoringError::Refused(format!(
                        "Menus/{menu} has no sub-Menu items; build the groups first with `twaco menu nest {menu} --group \"<Label>=<icon>:<Title>=<Mashup>\"...`"
                    )));
                }
                Some(_) => {}
                None => work.notes.push(format!(
                    "Menus/{menu} is not in the solution; it was not checked for sub-Menu items"
                )),
            }
            let mut properties: Vec<(String, Value)> = vec![("PrimaryNavMenu".into(), json!(menu))];
            let notes = super::set::menubar_properties(
                &work.document,
                "ptcsmenubar",
                &target.id,
                &mut properties,
            )?;
            work.notes.extend(notes);
            for (name, value) in [
                ("SubMenuType", json!("nested")),
                ("AutoCollapseMenus", json!(*auto_collapse)),
                ("StayOpenOnSelection", json!(true)),
                ("AlwaysOpen", json!(true)),
                ("DisableResizing", json!(true)),
                ("PrimaryNavIcons", json!(true)),
                ("Width", json!(*width)),
                ("MaxWidth", json!(*width)),
            ] {
                properties.push((name.into(), value));
            }
            let properties: Vec<(&str, Value)> = properties
                .iter()
                .map(|(name, value)| (name.as_str(), value.clone()))
                .collect();
            work.set(&target, &properties);
            let column = &target.path[..target.path.len().saturating_sub(1)];
            let owned = column.len() > 1
                && work.document.at(column)["Properties"]["Type"] == "flexcontainer"
                && mashup::edit::children(work.document.at(column)).len() == 1
                && work.document.at(&column[..column.len() - 1])["Properties"]["flex-direction"]
                    == "row";
            if owned {
                let id = work.document.at(column)["Properties"]["Id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let column = widget_id(&work.document, &id)?;
                let px = format!("{width}px");
                work.set(
                    &column,
                    &[
                        ("flex-size", json!("fixed")),
                        ("flex-min-width", json!(px)),
                        ("flex-max-width", json!(px)),
                        ("iconClass", json!("widgets-flexcontainer-fixed")),
                    ],
                );
                if work.document.at(&column.path)["Properties"]["EnableExpandCollapse"] == true {
                    work.notes.push(format!(
                        "{id} also collapses; the menu keeps its labels while the column is open (AlwaysOpen is true)"
                    ));
                }
            } else {
                work.notes.push(format!(
                    "{} is not alone in a column of a row; give its container a fixed width of {width}px yourself",
                    target.id
                ));
            }
        }
        Recipe::NavigateOnClick {
            widget,
            field,
            window,
        } => {
            if !matches!(window.as_str(), "modal-popup" | "popup" | "replace" | "new") {
                return Err(AuthoringError::Invalid(
                    "the window is modal-popup, popup, replace or new".into(),
                ));
            }
            let target = widget_id(&work.document, widget)?;
            let event = match target.type_id.as_str() {
                "ptcscollection" => "CellClicked",
                "ptcsgrid" => "RowClicked",
                other => {
                    return Err(AuthoringError::Invalid(format!(
                        "{} is a {other}; navigate-on-click needs a collection or a grid",
                        target.id
                    )))
                }
            };
            let sections = bindings::data_sections(work.document.value());
            let mut shown = bindings::whole_data_bindings(work.document.value())
                .into_iter()
                .filter(|binding| binding.widget == target.id);
            let source = shown.next().ok_or_else(|| {
                AuthoringError::Invalid(format!(
                    "{} shows no service result; bind its Data first (mashup bind data:<Entity>.<Service> ui:{}.Data)",
                    target.id, target.id
                ))
            })?;
            if shown.next().is_some() {
                return Err(AuthoringError::Ambiguous(format!(
                    "{} shows several service results; the recipe needs one",
                    target.id
                )));
            }
            let (section, call) = bindings::service_at(&sections, &source.section, &source.call)
                .ok_or_else(|| {
                    AuthoringError::Unknown(format!(
                        "the data call {} of {} is not in the mashup",
                        source.call, target.id
                    ))
                })?;
            let alias = if call.instance == call.target {
                String::new()
            } else {
                format!("[as {}]", call.instance)
            };
            let address = format!(
                "data:{}.{}{alias}[SelectedRows].{field}",
                section.entity, call.target
            );
            let fields = work.bound_fields(&target.id);
            work.field(&fields, field)?;
            let click = format!("ui:{}.{event}", target.id);
            let existing = work
                .document
                .widgets()
                .iter()
                .filter(|candidate| candidate.type_id == "navigationfunction")
                .map(|candidate| candidate.id.clone())
                .find(|id| work.has_handler(&click, &format!("ui:{id}.Navigate")));
            match existing {
                Some(id) => {
                    let function = widget_id(&work.document, &id)?;
                    let to = format!("ui:{id}.TargetMashup");
                    if !work.has_binding(&address, &to) {
                        for binding in work.document.bindings_mut() {
                            if binding["TargetId"] == id.as_str() {
                                if let Some(maps) = binding["PropertyMaps"].as_array_mut() {
                                    maps.retain(|map| map["TargetProperty"] != "TargetMashup");
                                }
                            }
                        }
                        work.document.bindings_mut().retain(|binding| {
                            binding["PropertyMaps"]
                                .as_array()
                                .is_none_or(|maps| !maps.is_empty())
                        });
                        work.bind(&address, &to)?;
                        work.notes.push(format!(
                            "{id} already navigates on {click}; it now opens the mashup named by {field}"
                        ));
                    }
                    if work.document.at(&function.path)["Properties"]["TargetWindowType"]
                        != window.as_str()
                    {
                        work.set(&function, &[("TargetWindowType", json!(window))]);
                    }
                }
                None => {
                    let id = work.add("navigationfunction", &[("TargetWindowType", json!(window))]);
                    work.bind(&address, &format!("ui:{id}.TargetMashup"))?;
                    work.on(&click, &format!("ui:{id}.Navigate"))?;
                }
            }
            let selection = if target.type_id == "ptcscollection" {
                "SelectionMode"
            } else {
                "RowSelection"
            };
            let current = &work.document.at(&target.path)["Properties"][selection];
            if current.is_null() || current == "none" {
                work.set(&target, &[(selection, json!("single"))]);
                work.notes.push(format!(
                    "{} could not select rows; its {selection} is now single so the click has a selected row to read",
                    target.id
                ));
            }
            if work.document.at(&target.path)["Properties"]["AutoSelectFirstRow"] == true {
                work.notes.push(format!(
                    "{id} selects its first row when it loads, which navigates away at once; run twaco mashup set {} {id} --prop AutoSelectFirstRow=false --apply",
                    request.mashup,
                    id = target.id
                ));
            }
        }
        Recipe::Validate {
            trigger,
            condition,
            inputs,
            then,
            otherwise,
        } => {
            let id = work.computed("validator2", &assigning(condition), None, inputs, false)?;
            work.on(&widget_event(trigger), &format!("ui:{id}.Evaluate"))?;
            work.on(&format!("ui:{id}.True"), then)?;
            if let Some(text) = otherwise {
                let message = work.add(
                    "statusmessage",
                    &[("MessageType", json!("error")), ("Message", json!(text))],
                );
                work.on(
                    &format!("ui:{id}.False"),
                    &format!("ui:{message}.ShowMessage"),
                )?;
            }
        }
    }
    work.notes
        .extend(untriggered_calls(work.document.value(), &request.mashup));
    if let Some(css) = &css {
        if loaded.sidecars.is_some() {
            work.document.root_mut().remove("CustomMashupCss");
        } else {
            work.document.set_top(
                "CustomMashupCss",
                json!(css.strip_suffix('\n').unwrap_or(css)),
            );
        }
    }
    let Work {
        document,
        added,
        removed,
        wired,
        mut notes,
        ..
    } = work;
    // Two inputs from one source raise the same note; keep the first, in order.
    let mut seen = std::collections::BTreeSet::new();
    notes.retain(|note| seen.insert(note.clone()));
    Ok(RecipePlan {
        edit: finish_edit_with_css(loaded, document, "mashup recipe", css)?,
        name: request.recipe.name(),
        generated: Vec::new(),
        added,
        removed,
        wired,
        notes,
    })
}

/// What a recipe can be given, as the command line and MCP both pass it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecipeArgs {
    pub settings: Vec<String>,
    pub fit: Option<String>,
    pub stacked: bool,
    pub band: bool,
    pub compact: Option<u32>,
    pub style: Option<String>,
    pub initial_step: Option<u32>,
    pub step_size: Option<String>,
    pub host: Option<String>,
    pub button: Option<String>,
    pub search_param: Option<String>,
    pub hint: Option<String>,
    pub run_on: Option<String>,
    pub buttons: Vec<String>,
    pub fields: Vec<String>,
    pub label_columns: Option<u32>,
    pub replace: Option<String>,
    pub row: Option<String>,
    pub layout: Option<String>,
    pub status: Option<String>,
    pub status_state: Option<String>,
    pub x_title: Option<String>,
    pub y_title: Option<String>,
    pub toggle: Option<String>,
    pub add_toggle: bool,
    pub in_container: Option<String>,
    pub tab: Option<String>,
    pub start: Option<String>,
    /// The widget the recipe is about (enable-when, show-when, text) or its trigger (confirm, popup).
    pub widget: Option<String>,
    pub condition: Option<String>,
    pub expression: Option<String>,
    pub param: Option<String>,
    /// Tracker `Property=field` overrides.
    pub field_maps: Vec<String>,
    /// `name=address`, repeatable.
    pub inputs: Vec<String>,
    pub after: Option<String>,
    pub say: Option<String>,
    pub on_failure: Option<String>,
    pub from: Option<String>,
    pub kind: Option<String>,
    pub then: Option<String>,
    pub title: Option<String>,
    pub action_label: Option<String>,
    pub cancel_label: Option<String>,
    pub on: Option<String>,
    pub target: Option<String>,
    pub window: Option<String>,
    /// `WIDTHxHEIGHT`.
    pub size: Option<String>,
    /// `name=address`, repeatable.
    pub pass: Vec<String>,
    pub reload: Option<String>,
    pub every: Option<u32>,
    pub interval: Option<String>,
    pub else_say: Option<String>,
    /// The service result a chart, grid or picker shows.
    pub data: Option<String>,
    /// A chart's category field (a pie's label field), a picker's display field.
    pub x: Option<String>,
    /// A pie's value field, a picker's value field.
    pub value: Option<String>,
    /// `field[:Title[:width[:format]]]`, repeatable.
    pub columns: Vec<String>,
    pub date_format: Option<String>,
    /// What a picker's selection is bound from.
    pub selected: Option<String>,
    /// Columns a grid's users may edit, each one of `columns`.
    pub editable: Vec<String>,
    /// `field=editor`, repeatable.
    pub column_editors: Vec<String>,
    /// Listed columns that keep whitespace when shown.
    pub preserve_whitespace: Vec<String>,
    /// A picker without the "(None)" row.
    pub no_none_row: bool,
    /// A picker or grid that selects its first row.
    pub auto_select_first: bool,
    pub series: Vec<String>,
    /// nested-menu: the root Menu.
    pub menu: Option<String>,
    /// nested-menu: leave several groups open at once.
    pub no_auto_collapse: bool,
    /// nested-menu: the column's width in pixels.
    pub width: Option<u32>,
    /// navigate-on-click: the field of the selected row that names the mashup.
    pub field: Option<String>,
    /// side-panel: the tooltip of the information icon after the title.
    pub info: Option<String>,
    /// picker: a list that selects several rows.
    pub multiple: bool,
    /// section: `center` centres the body's content (`start` is the default).
    pub align: Option<String>,
    /// card-grid: cells in a row.
    pub per_row: Option<u32>,
    /// card-grid: cells in all.
    pub cells: Option<u32>,
    /// input-variant: the text field look.
    pub variant: Option<String>,
}

/// The recipe names, for help and errors.
pub const RECIPES: &[&str] = &[
    "panel",
    "card",
    "slider-action",
    "toolbar",
    "label-pairs",
    "collapsible",
    "enable-when",
    "show-when",
    "text",
    "message",
    "confirm",
    "popup",
    "close-after",
    "auto-refresh",
    "validate",
    "chart",
    "sparkline",
    "columns",
    "pagination",
    "picker",
    "tracker",
    "tree",
    "set-on-click",
    "nested-menu",
    "navigate-on-click",
    "side-panel",
    "empty-state",
    "section",
    "card-grid",
    "input-variant",
    "component",
];

fn field_caption(name: &str) -> String {
    let chars: Vec<_> = name.chars().collect();
    let mut words = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' {
            words.push(' ');
        } else {
            if i > 0
                && c.is_uppercase()
                && (chars[i - 1].is_lowercase()
                    || chars[i - 1].is_numeric()
                    || (chars[i - 1].is_uppercase()
                        && chars.get(i + 1).is_some_and(|c| c.is_lowercase())))
            {
                words.push(' ');
            }
            words.push(c);
        }
    }
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().collect::<String>() + chars.as_str()
    })
}

fn pairs(flag: &str, values: &[String]) -> Result<Vec<(String, String)>, AuthoringError> {
    values
        .iter()
        .map(|text| {
            text.split_once('=')
                .map(|(name, address)| (name.trim().to_string(), address.trim().to_string()))
                .filter(|(name, address)| !name.is_empty() && !address.is_empty())
                .ok_or_else(|| {
                    AuthoringError::Invalid(format!("{flag} {text:?} must be name=address"))
                })
        })
        .collect()
}

fn required_number(value: Option<u32>, what: &str, recipe: &str) -> Result<u32, AuthoringError> {
    value.ok_or_else(|| AuthoringError::Invalid(format!("{recipe} needs {what}")))
}

fn required(value: &Option<String>, what: &str, recipe: &str) -> Result<String, AuthoringError> {
    value
        .clone()
        .ok_or_else(|| AuthoringError::Invalid(format!("{recipe} needs {what}")))
}

impl Recipe {
    /// Build a recipe from its name and arguments, refusing what it does not take.
    pub fn from_args(name: &str, args: &RecipeArgs) -> Result<Recipe, AuthoringError> {
        for (given, flag, recipe) in [
            (!args.settings.is_empty(), "--set", "component"),
            (args.fit.is_some(), "--fit", "panel"),
            (args.stacked, "--stacked", "chart"),
            (args.band, "--band", "toolbar"),
        ] {
            if given && name != recipe {
                return Err(AuthoringError::Invalid(format!(
                    "{flag} is for recipe {recipe}"
                )));
            }
        }
        if args
            .fit
            .as_deref()
            .is_some_and(|fit| !["fill", "content"].contains(&fit))
        {
            return Err(AuthoringError::Invalid(
                "panel: --fit is fill or content".into(),
            ));
        }
        if args.band {
            let mut rest = args.clone();
            rest.band = false;
            rest.widget = None;
            if rest != RecipeArgs::default() {
                return Err(AuthoringError::Invalid("toolbar --band takes only a responsive container; add date picker and buttons with mashup add".into()));
            }
            return Ok(Recipe::ToolbarBand {
                container: required(&args.widget, "a responsive row", name)?,
            });
        }
        if args.compact.is_some() && name != "panel" {
            return Err(AuthoringError::Invalid(
                "--compact is for recipe panel".into(),
            ));
        }
        if args.button.is_some() && name != "slider-action" {
            return Err(AuthoringError::Invalid(
                "the fourth operand (button) is for recipe slider-action".into(),
            ));
        }
        if ["panel", "slider-action"].contains(&name) {
            let mut rest = args.clone();
            rest.widget = None;
            rest.button = None;
            rest.compact = None;
            if name == "panel" {
                rest.style = None;
                rest.fit = None;
            }
            if rest != RecipeArgs::default() {
                return Err(AuthoringError::Invalid(format!(
                    "{name}: unrelated recipe flags are not accepted"
                )));
            }
        }
        if args.on_failure.is_some() && name != "message" {
            return Err(AuthoringError::Invalid(
                "--on-failure is for recipe message; close-after closes only on success".into(),
            ));
        }
        if args.width.is_some() && !["toolbar", "nested-menu"].contains(&name) {
            return Err(AuthoringError::Invalid(
                "--width is for recipe nested-menu or toolbar".into(),
            ));
        }
        if args
            .row
            .as_deref()
            .is_some_and(|v| !["first", "selected"].contains(&v))
            || args
                .layout
                .as_deref()
                .is_some_and(|v| !["inline", "stacked"].contains(&v))
        {
            return Err(AuthoringError::Invalid(
                "label-pairs: --row is first or selected; --layout is inline or stacked".into(),
            ));
        }
        if !["columns", "card"].contains(&name)
            && args.status.is_some() != args.status_state.is_some()
        {
            return Err(AuthoringError::Invalid("label-pairs: --status <field> and --status-state <StateDefinition> are required together".into()));
        }
        for (given, flag, recipe) in [
            (args.row.is_some(), "--row", "label-pairs"),
            (args.layout.is_some(), "--layout", "label-pairs"),
            (
                args.status.is_some() && !["columns", "card"].contains(&name),
                "--status",
                "label-pairs",
            ),
            (args.status_state.is_some(), "--status-state", "label-pairs"),
            (args.x_title.is_some(), "--x-title", "chart"),
            (args.y_title.is_some(), "--y-title", "chart"),
            (args.search_param.is_some(), "--search-param", "toolbar"),
            (args.hint.is_some(), "--hint", "toolbar"),
            (args.run_on.is_some(), "--run-on", "toolbar"),
            (!args.buttons.is_empty(), "--button", "toolbar"),
            (!args.fields.is_empty(), "--field", "label-pairs"),
            (args.label_columns.is_some(), "--columns", "label-pairs"),
            (args.date_format.is_some(), "--date-format", "columns"),
            (args.replace.is_some(), "--replace", "label-pairs"),
            (args.toggle.is_some(), "--toggle", "collapsible"),
            (args.add_toggle, "--add-toggle", "collapsible"),
            (args.in_container.is_some(), "--in", "collapsible"),
            (
                args.tab.is_some() && name != "side-panel",
                "--tab",
                "collapsible",
            ),
            (args.info.is_some(), "--info", "side-panel"),
            (args.align.is_some(), "--align", "section"),
            (args.per_row.is_some(), "--per-row", "card-grid"),
            (args.cells.is_some(), "--cells", "card-grid"),
            (args.variant.is_some(), "--variant", "input-variant"),
            (args.multiple, "--multiple", "picker"),
            (args.start.is_some(), "--start", "collapsible"),
            (args.action_label.is_some(), "--action-label", "confirm"),
            (args.cancel_label.is_some(), "--cancel-label", "confirm"),
            (args.param.is_some(), "--param", "set-on-click"),
            (args.style.is_some(), "--style", "panel"),
            (args.initial_step.is_some(), "--initial-step", "tracker"),
            (args.step_size.is_some(), "--step-size", "tracker"),
            (args.host.is_some(), "--host", "tracker"),
            (
                !args.field_maps.is_empty() && name != "tree",
                "--field-map",
                "tracker",
            ),
            (!args.editable.is_empty(), "--editable-column", "columns"),
            (
                !args.column_editors.is_empty(),
                "--column-editor",
                "columns",
            ),
            (
                !args.preserve_whitespace.is_empty(),
                "--preserve-whitespace",
                "columns",
            ),
            (args.no_none_row, "--no-none-row", "picker"),
            (
                args.auto_select_first && name != "columns",
                "--auto-select-first",
                "picker",
            ),
            (!args.series.is_empty(), "--series", "chart"),
            (args.menu.is_some(), "--menu", "nested-menu"),
            (args.no_auto_collapse, "--no-auto-collapse", "nested-menu"),
            (args.field.is_some(), "--field", "navigate-on-click"),
        ] {
            if given && name != recipe {
                return Err(AuthoringError::Invalid(format!(
                    "{flag} is for recipe {recipe}"
                )));
            }
        }
        let inputs = pairs("--input", &args.inputs)?;
        let widget = |what: &str| required(&args.widget, what, name);
        Ok(match name {
            "component" => Recipe::Component {
                container: required(&args.widget, "<container>", name)?,
                name: required(&args.kind, "--kind <component>", name)?,
                settings: args.settings.clone(),
            },
            "panel" => {
                if args.compact.is_some_and(|n| ![768, 960].contains(&n)) {
                    return Err(AuthoringError::Invalid(
                        "panel: --compact must be 768 or 960".into(),
                    ));
                }
                Recipe::Panel {
                    container: widget("a responsive column")?,
                    compact: args.compact,
                    style: args.style.clone().unwrap_or_else(|| "grey".into()),
                    fit: args.fit.clone().unwrap_or_else(|| "fill".into()),
                }
            }
            "card" => Recipe::Card {
                container: widget("an empty responsive column")?,
                caption: required(&args.title, "--title <caption>", name)?,
                value: args.say.clone(),
                status: args.status.clone(),
                kind: args.kind.clone().unwrap_or_else(|| "info".into()),
            },
            "slider-action" => Recipe::SliderAction {
                slider: widget("a slider and button")?,
                button: required(&args.button, "the fourth operand (button)", name)?,
            },
            "toolbar" => {
                let run_on = args.run_on.clone().unwrap_or_else(|| "both".into());
                let width = args.width.unwrap_or(240);
                if width == 0 || !["change", "enter", "both"].contains(&run_on.as_str()) {
                    return Err(AuthoringError::Invalid(
                        "toolbar: --width must be above 0; --run-on must be change, enter or both"
                            .into(),
                    ));
                }
                Recipe::Toolbar {
                    container: widget("the responsive container")?,
                    then: required(&args.then, "--then <existing data call>", name)?,
                    search_param: required(
                        &args.search_param,
                        "--search-param <STRING param>",
                        name,
                    )?,
                    hint: args.hint.clone().unwrap_or_else(|| "Search...".into()),
                    width,
                    run_on,
                    buttons: pairs("--button", &args.buttons)?,
                }
            }
            "label-pairs" => Recipe::LabelPairs {
                container: widget("the responsive container")?,
                data: required(&args.data, "--data <existing data call>", name)?,
                fields: args.fields.clone(),
                title: args.title.clone(),
                columns: args.label_columns,
                replace: args.replace.clone(),
                row: args.row.clone(),
                layout: args.layout.clone().unwrap_or_else(|| "inline".into()),
                status: args.status.clone(),
                status_state: args.status_state.clone(),
            },
            "collapsible" => {
                if args.toggle.is_some() && args.add_toggle {
                    return Err(AuthoringError::Invalid(
                        "collapsible: choose --toggle or --add-toggle".into(),
                    ));
                }
                if args.in_container.is_some() && !args.add_toggle {
                    return Err(AuthoringError::Invalid(
                        "collapsible: --in requires --add-toggle".into(),
                    ));
                }
                let tab = args.tab.clone().unwrap_or_else(|| "outside".into());
                if !["inside", "outside", "none"].contains(&tab.as_str()) {
                    return Err(AuthoringError::Invalid(
                        "collapsible: --tab must be inside, outside or none".into(),
                    ));
                }
                if args
                    .start
                    .as_deref()
                    .is_some_and(|value| value != "collapsed")
                {
                    return Err(AuthoringError::Invalid(
                        "collapsible: --start must be collapsed".into(),
                    ));
                }
                Recipe::Collapsible {
                    container: widget("the container")?,
                    toggle: args.toggle.clone(),
                    add_toggle_in: if args.add_toggle {
                        Some(required(&args.in_container, "--in <container>", name)?)
                    } else {
                        None
                    },
                    tab,
                    collapsed: args.start.is_some(),
                }
            }
            "enable-when" | "show-when" => {
                let widget = widget("the widget")?;
                let condition = required(&args.condition, "--if <condition>", name)?;
                if name == "enable-when" {
                    Recipe::EnableWhen {
                        widget,
                        condition,
                        inputs,
                    }
                } else {
                    Recipe::ShowWhen {
                        widget,
                        condition,
                        inputs,
                    }
                }
            }
            "text" => Recipe::Text {
                widget: widget("the label")?,
                expression: required(&args.expression, "--expr <expression>", name)?,
                inputs,
            },
            "set-on-click" => Recipe::SetOnClick {
                trigger: widget("the button")?,
                parameter: required(&args.param, "--param <name>", name)?,
                expression: required(&args.expression, "--expr <expression>", name)?,
                inputs,
            },
            "tracker" => Recipe::Tracker {
                widget: widget("the progress tracker, or new")?,
                data: required(&args.data, "--data <service result>", name)?,
                field_maps: pairs("--field-map", &args.field_maps)?,
                initial_step: args.initial_step.unwrap_or(1),
                step_size: args.step_size.clone().unwrap_or_else(|| "large".into()),
                host: args.host.clone(),
            },
            "tree" => Recipe::Tree {
                widget: widget("a tree selector")?,
                data: required(&args.data, "--data <service result>", name)?,
                field_maps: pairs("--field-map", &args.field_maps)?,
            },
            "message" => Recipe::Message {
                after: required(&args.after, "--after <data call>", name)?,
                text: args.say.clone(),
                from: args.from.clone(),
                kind: args.kind.clone().unwrap_or_else(|| "info".into()),
                on_failure: args.on_failure.clone(),
            },
            "confirm" => Recipe::Confirm {
                trigger: widget("the button")?,
                then: required(&args.then, "--then <handler>", name)?,
                title: args.title.clone(),
                message: args.say.clone(),
                action_label: args.action_label.clone(),
                cancel_label: args.cancel_label.clone(),
            },
            "popup" => {
                let size = match &args.size {
                    None => None,
                    Some(text) => Some(
                        text.split_once(['x', 'X'])
                            .and_then(|(w, h)| {
                                Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
                            })
                            .ok_or_else(|| {
                                AuthoringError::Invalid(format!(
                                    "--size {text:?} must be WIDTHxHEIGHT"
                                ))
                            })?,
                    ),
                };
                Recipe::Popup {
                    trigger: args
                        .on
                        .clone()
                        .or_else(|| args.widget.clone())
                        .ok_or_else(|| {
                            AuthoringError::Invalid(
                                "popup needs the widget that opens it, or --on <event>".into(),
                            )
                        })?,
                    target: required(&args.target, "--mashup <mashup it opens>", name)?,
                    window: args.window.clone().unwrap_or_else(|| "modal-popup".into()),
                    size,
                    title: args.title.clone(),
                    pass: pairs("--pass", &args.pass)?,
                    refresh: args.reload.clone(),
                }
            }
            "close-after" => Recipe::CloseAfter {
                after: required(&args.after, "--after <data call>", name)?,
            },
            "auto-refresh" => Recipe::AutoRefresh {
                call: required(&args.then, "--then <data call>", name)?,
                every: args.every,
                interval: args.interval.clone(),
            },
            "validate" => Recipe::Validate {
                trigger: args
                    .on
                    .clone()
                    .or_else(|| args.widget.clone())
                    .ok_or_else(|| {
                        AuthoringError::Invalid(
                            "validate needs the widget whose click it checks, or --on <event>"
                                .into(),
                        )
                    })?,
                condition: required(&args.condition, "--if <condition>", name)?,
                inputs,
                then: required(&args.then, "--then <handler>", name)?,
                otherwise: args.else_say.clone(),
            },
            "sparkline" => Recipe::Sparkline {
                container: widget("a responsive container")?,
                data: required(&args.data, "--data <service result>", name)?,
                x: required(&args.x, "--x <field>", name)?,
                value: required(&args.value, "--value <field>", name)?,
            },
            "chart" => Recipe::Chart {
                series: args.series.clone(),
                widget: widget("the chart")?,
                data: required(&args.data, "--data <service result>", name)?,
                x: required(&args.x, "--x <field>", name)?,
                value: args.value.clone(),
                kind: args.kind.clone(),
                title: args.title.clone(),
                stacked: args.stacked,
                x_title: args.x_title.clone(),
                y_title: args.y_title.clone(),
            },
            "columns" => Recipe::Columns {
                auto_select_first: args.auto_select_first,
                widget: widget("the grid")?,
                data: args.data.clone(),
                columns: args.columns.clone(),
                date_format: args.date_format.clone(),
                editable: args.editable.clone(),
                column_editors: pairs("--column-editor", &args.column_editors)?,
                preserve_whitespace: args.preserve_whitespace.clone(),
                status: args
                    .status
                    .as_deref()
                    .map(|text| {
                        let (field, state) = text
                            .split_once('=')
                            .filter(|(field, state)| {
                                !field.trim().is_empty() && !state.trim().is_empty()
                            })
                            .ok_or_else(|| {
                                AuthoringError::Invalid(
                                    "columns: --status must be field=StateDefinition".into(),
                                )
                            })?;
                        Ok::<_, AuthoringError>((
                            field.trim().to_string(),
                            state.trim().to_string(),
                        ))
                    })
                    .transpose()?,
            },
            "pagination" => Recipe::Pagination {
                widget: widget("the grid")?,
                data: required(&args.data, "--data <service result>", name)?,
            },
            "picker" => Recipe::Picker {
                widget: widget("the dropdown or list")?,
                data: required(&args.data, "--data <service result>", name)?,
                display: required(&args.x, "--x <display field>", name)?,
                value: args.value.clone(),
                selected: args.selected.clone(),
                no_none_row: args.no_none_row,
                auto_select_first: args.auto_select_first,
                multiple: args.multiple,
            },
            "side-panel" => Recipe::SidePanel {
                container: widget("an empty responsive column")?,
                title: required(&args.title, "--title <text>", name)?,
                info: args.info.clone(),
                tab: args.tab.clone().unwrap_or_else(|| "Filters".into()),
            },
            "empty-state" => Recipe::EmptyState {
                widget: widget("the collection, grid or list")?,
                text: required(&args.say, "--say <text>", name)?,
                data: args.data.clone(),
            },
            "section" => Recipe::Section {
                container: widget("an empty responsive column")?,
                title: required(&args.title, "--title <text>", name)?,
                center: match args.align.as_deref() {
                    None | Some("start") => false,
                    Some("center") => true,
                    Some(_) => {
                        return Err(AuthoringError::Invalid(
                            "section: --align is start or center".into(),
                        ))
                    }
                },
            },
            "card-grid" => Recipe::CardGrid {
                container: widget("an empty responsive column")?,
                per_row: required_number(args.per_row, "--per-row <n>", name)?,
                cells: required_number(args.cells, "--cells <n>", name)?,
            },
            "input-variant" => Recipe::InputVariant {
                widget: widget("the text field")?,
                variant: required(&args.variant, "--variant <name>", name)?,
            },
            "nested-menu" => Recipe::NestedMenu {
                menubar: widget("the menu bar")?,
                menu: required(&args.menu, "--menu <root Menu>", name)?,
                auto_collapse: !args.no_auto_collapse,
                width: match args.width.unwrap_or(300) {
                    0 => {
                        return Err(AuthoringError::Invalid(
                            "--width is a number of pixels above 0".into(),
                        ))
                    }
                    width => width,
                },
            },
            "navigate-on-click" => Recipe::NavigateOnClick {
                widget: widget("the collection or grid")?,
                field: required(&args.field, "--field <field naming the mashup>", name)?,
                window: args.window.clone().unwrap_or_else(|| "replace".into()),
            },
            other => {
                return Err(AuthoringError::Invalid(format!(
                    "unknown recipe {other}; the recipes are {}",
                    RECIPES.join(", ")
                )))
            }
        })
    }
}

/// The plan as data, for `--json` and MCP.
pub fn recipe_plan_json(solution: &Solution, plan: &RecipePlan, applied: bool) -> Value {
    let relative = |path: &std::path::Path| solution.relative(path);
    let mut files = vec![relative(&plan.edit.entity_path)];
    if let Some(sidecars) = &plan.edit.sidecars {
        files.push(relative(&sidecars.content_path));
        if sidecars.css_before != sidecars.css_after {
            files.push(relative(&sidecars.css_path));
        }
    }
    for generated in &plan.generated {
        files.extend(
            generated.report["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_str().map(str::to_string)),
        );
    }
    json!({
        "applied": applied,
        "unchanged": plan.unchanged(),
        "mashup": plan.edit.mashup,
        "recipe": plan.name,
        "added": plan.added,
        "removed": plan.removed,
        "notes": plan.notes,
        "wired": plan.wired,
        "files": files,
    })
}

fn column_editors_for(base: &str) -> &'static [&'static str] {
    match base {
        "BOOLEAN" => &["checkbox", "textfield", "textarea"],
        "DATETIME" => &["datepicker", "textfield"],
        "HTML" | "HYPERLINK" | "IMAGE" | "JSON" | "TEXT" => &["textarea", "textfield"],
        "STRING" => &["textfield", "textarea", "dropdown"],
        "XML" => &["textarea"],
        _ => &["textfield", "textarea"],
    }
}

fn refresh_column_type(
    column: &mut Value,
    previous: Option<&[(String, String)]>,
    name: &str,
    base: &str,
) {
    let Some((_, old_base)) = previous
        .into_iter()
        .flatten()
        .find(|(field, _)| field == name)
    else {
        return;
    };
    if old_base != base {
        let defaults = column_format(name, name, json!("auto"), base);
        column["cellEditor"] = defaults["cellEditor"].clone();
        for key in ["renderer", "FormatString"] {
            column["FormatOptions"][key] = defaults["FormatOptions"][key].clone();
        }
    }
}

/// One grid column as Composer writes it: the renderer, its format and the cell editor follow the
/// field's base type.
fn column_format(name: &str, title: &str, width: Value, base: &str) -> Value {
    let (renderer, format, editor) = match base {
        "STRING" | "THINGNAME" | "USERNAME" | "GUID" => ("STRING", "full", "textfield"),
        "TEXT" => ("STRING", "full", "textarea"),
        "NUMBER" => ("NUMBER", "0.00", "textfield"),
        "INTEGER" | "LONG" => ("NUMBER", "0", "textfield"),
        "BOOLEAN" => ("BOOLEAN", "checkbox", "checkbox"),
        "DATETIME" => ("DATETIME", "yyyy-MM-dd HH:mm:ss", "datepicker"),
        "IMAGELINK" => ("IMAGELINK", "", ""),
        _ => ("DEFAULT", "", ""),
    };
    json!({
        "AllowEdit": false,
        "Description": "",
        "FieldName": name,
        "FormatOptions": {
            "FormatString": format,
            "RoundingEnabled": true,
            "formatInfo": {"StateFormats": []},
            "formatText": "[[HyperlinkName]]",
            "renderer": renderer,
            "roundingEnabled": true,
            "showDependentField": false
        },
        "Title": title,
        "Width": width,
        "cellEditor": editor,
        "cellHorizontalAlignment": "left",
        "columnMinimumWidth": 75,
        "editable": false,
        "excludeFromRowEdit": false,
        "expressionValidator": "validator2",
        "headerHorizontalAlignment": "left",
        "hidden": false,
        "inLayout": true,
        "initialSort": "none",
        "preserveWhiteSpaces": false,
        "required": false,
        "routeLinks": false,
        "sortable": true,
        "stripHtml": false
    })
}

/// Maps a tracker needs; the others are left unset when no field matches.
const TRACKER_REQUIRED: &[&str] = &["StepNumberField", "StepLabelField"];

/// Catalogue field maps and deliberately conservative, case/separator-insensitive aliases.
const TRACKER_MAPS: &[(&str, &[&str], &[&str])] = &[
    (
        "StepNumberField",
        &["stepnumber", "number", "step", "index", "ordinal"],
        &["NUMBER", "INTEGER", "LONG"],
    ),
    (
        "StepLabelField",
        &[
            "steplabel",
            "stepname",
            "label",
            "title",
            "name",
            "description",
        ],
        &["STRING", "TEXT"],
    ),
    (
        "StepStateField",
        &["stepstate", "state", "status"],
        &["STRING", "TEXT"],
    ),
    (
        "StepInteractiveField",
        &[
            "stepinteractive",
            "isinteractive",
            "interactive",
            "enabled",
            "canselect",
        ],
        &["BOOLEAN"],
    ),
    (
        "StepMashupNameField",
        &["stepmashupname", "stepmashup", "mashupname", "mashup"],
        &["MASHUPNAME", "STRING"],
    ),
];

const TREE_REQUIRED: &[&str] = &["IDField", "NameField", "ParentIDField"];
const TREE_MAPS: &[FieldMapSpec] = &[
    (
        "IDField",
        &["id", "nodeid", "itemid", "identifier"],
        &["STRING", "TEXT", "INTEGER", "LONG", "NUMBER", "GUID"],
    ),
    (
        "NameField",
        &[
            "name",
            "nodename",
            "itemname",
            "label",
            "title",
            "displayname",
        ],
        &["STRING", "TEXT"],
    ),
    (
        "ParentIDField",
        &["parentid", "parent", "parentnodeid", "parentitemid"],
        &["STRING", "TEXT", "INTEGER", "LONG", "NUMBER", "GUID"],
    ),
    (
        "ColorField",
        &["color", "colour", "nodecolor", "nodecolour"],
        &["STRING", "TEXT"],
    ),
];

fn tracker_maps(
    fields: &[(String, String)],
    overrides: &[(String, String)],
) -> Result<Vec<(&'static str, String)>, AuthoringError> {
    field_maps_for("tracker", TRACKER_MAPS, TRACKER_REQUIRED, fields, overrides)
}

type FieldMapSpec = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

fn field_maps_for(
    recipe: &str,
    specs: &[FieldMapSpec],
    required: &[&str],
    fields: &[(String, String)],
    overrides: &[(String, String)],
) -> Result<Vec<(&'static str, String)>, AuthoringError> {
    let mut explicit = std::collections::BTreeMap::new();
    for (property, field) in overrides {
        if !specs.iter().any(|(name, _, _)| property == name) {
            return Err(AuthoringError::Invalid(format!(
                "unknown {recipe} map {property}; use {}",
                specs
                    .iter()
                    .map(|(name, _, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        if explicit.insert(property.as_str(), field.as_str()).is_some() {
            return Err(AuthoringError::Invalid(format!(
                "{recipe} map {property} is given twice"
            )));
        }
    }
    let normalize = |name: &str| {
        name.chars()
            .filter(char::is_ascii_alphanumeric)
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let mut maps = Vec::new();
    let mut unmatched = Vec::new();
    for &(property, aliases, bases) in specs {
        let field = if let Some(field) = explicit.get(property) {
            fields
                .iter()
                .find(|(name, _)| name == field)
                .ok_or_else(|| {
                    AuthoringError::Invalid(format!(
                        "{recipe} map {property} names missing field {field}; available fields: {}",
                        fields
                            .iter()
                            .map(|(name, _)| name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })?
        } else {
            let candidates: Vec<_> = fields
                .iter()
                .filter(|(name, base)| {
                    aliases.contains(&normalize(name).as_str()) && bases.contains(&base.as_str())
                })
                .collect();
            match candidates.as_slice() {
                [field] => *field,
                [] if !required.contains(&property) => continue,
                _ => {
                    unmatched.push(if candidates.is_empty() {
                        property.to_string()
                    } else {
                        format!(
                            "{property} (ambiguous: {})",
                            candidates
                                .iter()
                                .map(|(name, _)| name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    });
                    continue;
                }
            }
        };
        if !bases.contains(&field.1.as_str()) {
            return Err(AuthoringError::Invalid(format!(
                "{recipe} map {property} field {} is {}; use a {} field",
                field.0,
                field.1,
                bases.join("/")
            )));
        }
        maps.push((property, field.0.clone()));
    }
    if !unmatched.is_empty() {
        return Err(AuthoringError::Invalid(format!("{recipe} has unmatched field maps: {}; give --field-map Property=field for each (MCP field_maps); available fields: {}", unmatched.join("; "), fields.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", "))));
    }
    Ok(maps)
}

/// A decided property list as the `(name, value)` pairs the writers take.
fn as_pairs(properties: &[(String, Value)]) -> Vec<(&str, Value)> {
    properties
        .iter()
        .map(|(key, value)| (key.as_str(), value.clone()))
        .collect()
}

type ComboSeries = (String, String, Option<String>, Option<String>);

pub(crate) fn combo_props(series: &[ComboSeries], current: &Value) -> Vec<(String, Value)> {
    let mut props = vec![
        ("NumberOfSeries".into(), json!(series.len())),
        ("DisableLegendGrouping".into(), json!(true)),
        ("LegendPosition".into(), json!("bottom")),
        ("LegendAlignment".into(), json!("start")),
        ("NumberOfYaxes".into(), json!(1)),
        ("HideYAxis2".into(), json!(true)),
    ];
    for (i, (field, kind, title, axis)) in series.iter().enumerate() {
        let n = i + 1;
        props.extend([
            (format!("DataField{n}"), json!(field)),
            (format!("Series{n}Type"), json!(kind)),
            (
                format!("DisplaySeries{n}On"),
                json!(if axis.as_deref() == Some("secondary") {
                    "yaxis2"
                } else {
                    "yaxis1"
                }),
            ),
        ]);
        if let Some(title) = title {
            props.push((format!("DataLabel{n}"), json!(title)));
        }
        if axis.as_deref() == Some("secondary") {
            props.extend([
                ("HideYAxis2".into(), json!(false)),
                ("NumberOfYaxes".into(), json!(2)),
                ("YAxis2Label".into(), json!(title.as_ref().unwrap_or(field))),
                ("YAxis2Position".into(), json!("right")),
            ]);
        }
    }
    for n in series.len() + 1..=MAX_COMBO_SERIES {
        let key = format!("DataField{n}");
        if current[&key]
            .as_str()
            .is_some_and(|field| !field.is_empty())
        {
            props.push((key, json!("")));
        }
    }
    props
}

pub(crate) fn sparkline_props(x: &str, value: &str) -> serde_json::Map<String, Value> {
    json!({"XAxisField":x,"DataField1":value,"NumberOfSeries":1,
        "Label":"","LabelType":"label","XAxisLabel":"","YAxisLabel":"","Notes":"","HideNotes":true,
        "SparkView":true,"HideXAxis":true,"HideYAxis":true,"HideLegend":true,"ShowHideLegend":false,
        "HorizontalAxisMaxHeight":0,"VerticalAxisMaxWidth":0,"LegendMaxWidth":0,
        "ShowXAxisRuler":false,"ShowYAxisRuler":false,"ShowZeroValueRuler":false,
        "HorizontalZoom":false,"HorizontalRangeZoom":false,"HorizontalSliderZoom":false,
        "VerticalZoom":false,"VerticalSliderZoom":false,"ShowZoomButtons":false,
        "ShowHorizontalAnchorPointControl":false,"HorizontalIntervalControl":"none",
        "DirectSelectionZoom":"none","DragSelectionZoom":"none",
        "HideValues":true,"HideMarkers":true,"MarkerSize":1,"ShowDataLoading":false,"Height":40,"Margin":"0"})
    .as_object()
    .unwrap()
    .clone()
}

pub(crate) fn sparkline_styles() -> Vec<String> {
    [
        "min-height=0px",
        "min-width=0px",
        "height=40px",
        "max-height=40px",
        "margin=0px",
        "padding=0px",
        "chart-layout:min-height=0px",
        "chart-layout:min-width=0px",
        "chart-layout:margin=0px",
        "chart-layout:box-sizing=border-box",
        "chart-layout:padding-top=0px",
        "chart-layout:padding-bottom=0px",
        "chart-layout:padding-left=0px",
        "chart-layout:padding-right=0px",
        // The plot row is 1fr of the chart grid and keeps the core chart's own 155px floor;
        // live, only fixed part heights let the line fill the 40px box.
        "chart:min-height=0px",
        "chart-slot:min-height=0px",
        "chart-slot:height=40px",
        "core-chart:height=40px",
    ]
    .into_iter()
    .map(String::from)
    .chain(
        [
            "title-area",
            "notes-area",
            "legend-area",
            "action-bar-area",
            "zoom-xaxis",
            "zoom-yaxis",
            "xaxis",
            "yaxis",
            "yaxis2",
        ]
        .into_iter()
        .map(|part| format!("{part}:display=none")),
    )
    .collect()
}

fn sparkline_style() -> Value {
    let mut entry =
        json!({"widget":"ptcs-chart-line","parts":{"$":{"$":{}},"chart-layout":{"$":{}}}});
    for text in sparkline_styles() {
        let (key, value) = text.split_once('=').unwrap();
        let (part, key) = key.split_once(':').unwrap_or(("$", key));
        if entry["parts"].get(part).is_none() {
            entry["parts"][part] = json!({"$": {}});
        }
        entry["parts"][part]["$"][key] = json!(value);
    }
    entry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_overrides_use_native_chart_properties_and_automatic_bounds() {
        let chart = widgets::catalog().by_type("ptcschartline").unwrap();
        let props = sparkline_props("year", "Value");
        for key in props.keys() {
            assert!(
                chart.property(key).is_some()
                    || chart.defaults.contains_key(key)
                    || chart.numbered_default(key).is_some(),
                "{key}"
            );
        }
        let styles = sparkline_styles();
        for part in [
            "chart-slot:height=40px",
            "core-chart:height=40px",
            "chart:min-height=0px",
        ] {
            assert!(styles.iter().any(|s| s == part), "{part}");
        }
        for bound in ["YAxisMinimumValues", "YAxisMaximumValues"] {
            assert!(chart.property(bound).is_some());
            assert!(!props.contains_key(bound));
            assert!(!chart.defaults.contains_key(bound));
        }
    }

    #[test]
    fn tree_maps_cover_catalogue_and_require_parent_and_unambiguous_optional_color() {
        let catalog = widgets::catalog().by_type("ptcstreeselector").unwrap();
        let mut catalogue_fields: Vec<_> = catalog
            .properties
            .as_ref()
            .unwrap()
            .iter()
            .filter(|(_, p)| p.base_type.as_deref() == Some("FIELDNAME"))
            .map(|(name, _)| name.as_str())
            .collect();
        catalogue_fields.sort();
        let mut mapped: Vec<_> = TREE_MAPS.iter().map(|(name, _, _)| *name).collect();
        mapped.sort();
        assert_eq!(catalogue_fields, mapped);
        let fields = vec![
            ("id".into(), "STRING".into()),
            ("name".into(), "STRING".into()),
        ];
        let error = field_maps_for("tree", TREE_MAPS, TREE_REQUIRED, &fields, &[])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("ParentIDField") && error.contains("available fields: id, name"),
            "{error}"
        );
        let fields = vec![
            ("id".into(), "STRING".into()),
            ("name".into(), "STRING".into()),
            ("parentId".into(), "STRING".into()),
            ("color".into(), "STRING".into()),
            ("colour".into(), "STRING".into()),
        ];
        assert!(
            field_maps_for("tree", TREE_MAPS, TREE_REQUIRED, &fields, &[])
                .unwrap_err()
                .to_string()
                .contains("ColorField (ambiguous: color, colour)")
        );
        let maps = field_maps_for("tree", TREE_MAPS, TREE_REQUIRED, &fields[..3], &[]).unwrap();
        assert_eq!(maps.len(), 3);
        for (name, args) in [
            (
                "tree",
                RecipeArgs {
                    initial_step: Some(1),
                    ..Default::default()
                },
            ),
            (
                "tree",
                RecipeArgs {
                    step_size: Some("small".into()),
                    ..Default::default()
                },
            ),
            (
                "tree",
                RecipeArgs {
                    host: Some("host".into()),
                    ..Default::default()
                },
            ),
            (
                "tracker",
                RecipeArgs {
                    style: Some("card".into()),
                    ..Default::default()
                },
            ),
        ] {
            assert!(Recipe::from_args(name, &args).is_err());
        }
    }

    #[test]
    fn tracker_maps_require_complete_unambiguous_typed_fields_and_valid_overrides() {
        let fields: Vec<_> = [
            ("number", "NUMBER"),
            ("name", "STRING"),
            ("description", "STRING"),
            ("state", "STRING"),
        ]
        .into_iter()
        .map(|(name, base)| (name.into(), base.into()))
        .collect();
        let error = tracker_maps(&fields, &[]).unwrap_err().to_string();
        for missing in [
            "StepLabelField (ambiguous: name, description)",
            "--field-map",
        ] {
            assert!(error.contains(missing), "{error}");
        }
        // Optional maps with no matching field stay unset; the required ones must match.
        let maps = tracker_maps(&fields, &[("StepLabelField".into(), "name".into())]).unwrap();
        assert_eq!(
            maps,
            vec![
                ("StepNumberField", "number".to_string()),
                ("StepLabelField", "name".to_string()),
                ("StepStateField", "state".to_string()),
            ]
        );
        let error = tracker_maps(&fields[1..2], &[]).unwrap_err().to_string();
        assert!(error.contains("StepNumberField"), "{error}");
        assert!(!error.contains("StepMashupNameField"), "{error}");
        for (overrides, expected) in [
            (
                vec![("LabelField", "name")],
                "unknown tracker map LabelField",
            ),
            (
                vec![
                    ("StepLabelField", "name"),
                    ("StepLabelField", "description"),
                ],
                "given twice",
            ),
            (vec![("StepLabelField", "missing")], "missing field missing"),
            (
                vec![("StepNumberField", "name")],
                "use a NUMBER/INTEGER/LONG field",
            ),
        ] {
            let overrides: Vec<_> = overrides
                .into_iter()
                .map(|(p, f)| (p.into(), f.into()))
                .collect();
            assert!(tracker_maps(&fields, &overrides)
                .unwrap_err()
                .to_string()
                .contains(expected));
        }
        for (recipe, args, flag) in [
            (
                "text",
                RecipeArgs {
                    action_label: Some("Delete".into()),
                    ..Default::default()
                },
                "--action-label",
            ),
            (
                "text",
                RecipeArgs {
                    cancel_label: Some("Keep".into()),
                    ..Default::default()
                },
                "--cancel-label",
            ),
            (
                "text",
                RecipeArgs {
                    param: Some("Step".into()),
                    ..Default::default()
                },
                "--param",
            ),
            (
                "text",
                RecipeArgs {
                    field_maps: vec!["StepLabelField=name".into()],
                    ..Default::default()
                },
                "--field-map",
            ),
        ] {
            assert!(Recipe::from_args(recipe, &args)
                .unwrap_err()
                .to_string()
                .contains(flag));
        }
    }

    #[test]
    fn column_editor_choices_follow_composer_base_types() {
        for (base, expected) in [
            ("BOOLEAN", vec!["checkbox", "textfield", "textarea"]),
            ("DATETIME", vec!["datepicker", "textfield"]),
            ("HTML", vec!["textarea", "textfield"]),
            ("HYPERLINK", vec!["textarea", "textfield"]),
            ("IMAGE", vec!["textarea", "textfield"]),
            ("JSON", vec!["textarea", "textfield"]),
            ("TEXT", vec!["textarea", "textfield"]),
            ("STRING", vec!["textfield", "textarea", "dropdown"]),
            ("XML", vec!["textarea"]),
        ] {
            assert_eq!(column_editors_for(base), expected, "{base}");
        }
        for base in [
            "IMAGELINK",
            "INTEGER",
            "LOCATION",
            "LONG",
            "NUMBER",
            "PASSWORD",
            "THINGCODE",
            "THINGNAME",
            "VEC2",
            "VEC3",
            "VEC4",
            "UNKNOWN",
            "",
        ] {
            assert_eq!(
                column_editors_for(base),
                ["textfield", "textarea"],
                "{base}"
            );
        }
        for (base, editor) in [("TEXT", "textarea"), ("STRING", "textfield")] {
            let column = column_format("Notes", "Notes", json!("auto"), base);
            assert_eq!(column["cellEditor"], editor);
            assert_eq!(column["preserveWhiteSpaces"], false);
        }
        for args in [
            RecipeArgs {
                column_editors: vec!["Notes=textarea".into()],
                ..Default::default()
            },
            RecipeArgs {
                preserve_whitespace: vec!["Notes".into()],
                ..Default::default()
            },
        ] {
            assert!(Recipe::from_args("text", &args)
                .unwrap_err()
                .to_string()
                .contains("is for recipe columns"));
        }
    }

    #[test]
    fn untriggered_call_commands_quote_aliases_and_check_the_exact_instance() {
        let mut content = json!({
            "Data": {"Acme.Manager": {"EntityName":"Acme.Manager", "EntityType":"Thing", "Services":[{"Name":"rows", "Target":"GetRows"}]}},
            "DataBindings":[{"SourceArea":"Data", "SourceSection":"Acme.Manager", "SourceId":"rows"}],
            "Events":[{"EventHandlerArea":"Data", "EventHandlerId":"Acme.Manager", "EventHandlerService":"GetRows"}]
        });
        let notes = untriggered_calls(&content, "Acme.Page_MU");
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("--invoke \"data:Acme.Manager.GetRows[as rows]\" --apply"));
        content["Events"][0]["EventHandlerService"] = json!("rows");
        assert!(untriggered_calls(&content, "Acme.Page_MU").is_empty());
    }

    #[test]
    fn numeric_column_formats_preserve_integer_precision() {
        for (base, expected) in [("INTEGER", "0"), ("LONG", "0"), ("NUMBER", "0.00")] {
            let column = column_format("Acme.Count", "Count", json!(120), base);
            assert_eq!(column["FormatOptions"]["FormatString"], expected);
            assert_eq!(column["FormatOptions"]["renderer"], "NUMBER");
        }
    }
}
