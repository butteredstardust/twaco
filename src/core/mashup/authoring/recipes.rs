//! Recipes: the wiring real mashups repeat, each written in one edit from the same pieces
//! `mashup add`, `mashup bind` and `mashup on` use. Counts are from 1,084 reference mashups.

use super::{
    bind_in, cache_entry, contained_parameters, finish_edit, load_existing, navigation_definition,
    on_in, parse_loaded, AuthoringContext, AuthoringError, EditPlan, ParameterDefinition,
};
use crate::core::config::Solution;
use crate::core::mashup;
use crate::core::mashup::address::parse_address;
use crate::core::mashup::describe::mashup_parameters;
use crate::core::mashup::edit::MashupDocument;
use crate::core::{bindings, types, widgets};
use serde_json::{json, Value};

/// The most series a combo chart holds (DataField1..DataField24 in the 10.2 widget).
const MAX_COMBO_SERIES: usize = 24;

/// One recipe and what it needs. Addresses are those `mashup bind` and `mashup on` take.
#[derive(Clone, Debug)]
pub enum Recipe {
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
    CloseAfter { after: String },
    /// A service run again every so often (48 seen).
    AutoRefresh {
        call: String,
        every: Option<u32>,
        interval: Option<String>,
    },
    /// A chart drawing a service result: `Data` bound, the category field named (pie: label and
    /// value fields). Every other numeric column of a single source becomes a series.
    Chart {
        widget: String,
        data: String,
        x: String,
        value: Option<String>,
        kind: Option<String>,
    },
    /// A grid's columns, in order, with titles and widths, rendered by each field's base type.
    Columns {
        widget: String,
        data: Option<String>,
        columns: Vec<String>,
    },
    /// A dropdown or list filled from a service result, showing one field.
    Picker {
        widget: String,
        data: String,
        display: String,
        value: Option<String>,
        selected: Option<String>,
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
    pub name: &'static str,
    /// The widgets the recipe added.
    pub added: Vec<String>,
    /// What it wired, one line each, in `mashup describe` address form.
    pub wired: Vec<String>,
    /// What the wiring changed besides, such as a grid's selection turned on.
    pub notes: Vec<String>,
}

impl Recipe {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::EnableWhen { .. } => "enable-when",
            Self::ShowWhen { .. } => "show-when",
            Self::Text { .. } => "text",
            Self::SetOnClick { .. } => "set-on-click",
            Self::Tracker { .. } => "tracker",
            Self::Message { .. } => "message",
            Self::Confirm { .. } => "confirm",
            Self::Popup { .. } => "popup",
            Self::CloseAfter { .. } => "close-after",
            Self::AutoRefresh { .. } => "auto-refresh",
            Self::Validate { .. } => "validate",
            Self::Chart { .. } => "chart",
            Self::Columns { .. } => "columns",
            Self::Picker { .. } => "picker",
        }
    }
}

struct Work<'a> {
    document: MashupDocument,
    solution: &'a Solution,
    context: AuthoringContext<'a>,
    added: Vec<String>,
    wired: Vec<String>,
    notes: Vec<String>,
}

impl Work<'_> {
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
            ("AutoEvaluate", json!(true)),
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
    let rest = trigger.strip_prefix("ui:").unwrap_or(trigger);
    if rest.contains('.') {
        format!("ui:{rest}")
    } else {
        format!("ui:{rest}.Clicked")
    }
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
    let document = parse_loaded(&loaded)?;
    let parameters =
        mashup_parameters(&loaded.entity_before).map_err(AuthoringError::Unreadable)?;
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let mut work = Work {
        document,
        solution,
        context: AuthoringContext {
            parameters: &parameters,
            model: &model,
            platform: platform.as_ref(),
        },
        added: Vec::new(),
        wired: Vec::new(),
        notes: Vec::new(),
    };
    match &request.recipe {
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
            let expression = if enable {
                format!("Output = !({condition});")
            } else {
                format!("Output = ({condition});")
            };
            let id = work.computed("expression2", &expression, Some("BOOLEAN"), inputs)?;
            work.bind(
                &format!("ui:{id}.Output"),
                &format!(
                    "ui:{}.{}",
                    target.id,
                    if enable { "Disabled" } else { "Visible" }
                ),
            )?;
            work.on("mashup:Loaded", &format!("ui:{id}.Evaluate"))?;
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
                let id =
                    work.computed("expression2", &assigning(expression), Some(&base), inputs)?;
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
            let mut properties = vec![("IsInteractive", json!(true)), ("StepSize", json!("large"))];
            properties.extend(maps.iter().map(|(name, field)| (*name, json!(field))));
            work.set(&target, &properties);
            work.bind(data, &format!("ui:{}.Data", target.id))?;
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
        Recipe::Chart {
            widget,
            data,
            x,
            value,
            kind,
        } => {
            let target = widget_id(&work.document, widget)?;
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
            work.set(&target, &properties);
            work.bind(data, &format!("ui:{}.Data", target.id))?;
        }
        Recipe::Columns {
            widget,
            data,
            columns,
        } => {
            let target = widget_id(&work.document, widget)?;
            if target.type_id != "ptcsgrid" {
                return Err(AuthoringError::Invalid(format!(
                    "{} is a {}, not a grid",
                    target.id, target.type_id
                )));
            }
            if columns.is_empty() {
                return Err(AuthoringError::Invalid(
                    "columns needs at least one --column".into(),
                ));
            }
            if let Some(data) = data {
                work.bind(data, &format!("ui:{}.Data", target.id))?;
            }
            // Without a known DataShape the listed columns are written as given and the grid
            // keeps showing the fields it is not told about.
            let fields = work.bound_fields(&target.id);
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
            let known = fields.clone();
            let mut format = Vec::new();
            for text in columns {
                let mut parts = text.splitn(3, ':');
                let name = parts.next().unwrap_or_default().trim().to_string();
                if name.is_empty() {
                    return Err(AuthoringError::Invalid(format!(
                        "--column {text:?} must be field[:Title[:width]]"
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
                format.push(column_format(&name, &title, width, &base));
            }
            // Composer keeps all fields in the configuration, even those outside the layout.
            for (name, base) in fields.iter().flatten() {
                if !format.iter().any(|column| column["FieldName"] == *name) {
                    let mut column = column_format(name, name, json!("auto"), base);
                    column["inLayout"] = json!(false);
                    column["hidden"] = json!(true);
                    format.push(column);
                }
            }
            work.set(
                &target,
                &[("ColumnFormat", json!({ "formatInfo": format }))],
            );
        }
        Recipe::Picker {
            widget,
            data,
            display,
            value,
            selected,
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
            work.set(&target, &properties);
            work.bind(data, &format!("ui:{}.Data", target.id))?;
            if let Some(selected) = selected {
                work.bind(selected, &format!("ui:{}.SelectedText", target.id))?;
            }
        }
        Recipe::Validate {
            trigger,
            condition,
            inputs,
            then,
            otherwise,
        } => {
            let id = work.computed("validator2", &assigning(condition), None, inputs)?;
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
    let Work {
        document,
        added,
        wired,
        mut notes,
        ..
    } = work;
    // Two inputs from one source raise the same note; keep the first, in order.
    let mut seen = std::collections::BTreeSet::new();
    notes.retain(|note| seen.insert(note.clone()));
    Ok(RecipePlan {
        edit: finish_edit(loaded, document, "mashup recipe")?,
        name: request.recipe.name(),
        added,
        wired,
        notes,
    })
}

/// What a recipe can be given, as the command line and MCP both pass it.
#[derive(Clone, Debug, Default)]
pub struct RecipeArgs {
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
    /// `field[:Title[:width]]`, repeatable.
    pub columns: Vec<String>,
    /// What a picker's selection is bound from.
    pub selected: Option<String>,
}

/// The recipe names, for help and errors.
pub const RECIPES: &[&str] = &[
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
    "columns",
    "picker",
    "tracker",
    "set-on-click",
];

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

fn required(value: &Option<String>, what: &str, recipe: &str) -> Result<String, AuthoringError> {
    value
        .clone()
        .ok_or_else(|| AuthoringError::Invalid(format!("{recipe} needs {what}")))
}

impl Recipe {
    /// Build a recipe from its name and arguments, refusing what it does not take.
    pub fn from_args(name: &str, args: &RecipeArgs) -> Result<Recipe, AuthoringError> {
        if args.on_failure.is_some() && name != "message" {
            return Err(AuthoringError::Invalid(
                "--on-failure is for recipe message; close-after closes only on success".into(),
            ));
        }
        for (given, flag, recipe) in [
            (args.action_label.is_some(), "--action-label", "confirm"),
            (args.cancel_label.is_some(), "--cancel-label", "confirm"),
            (args.param.is_some(), "--param", "set-on-click"),
            (!args.field_maps.is_empty(), "--field-map", "tracker"),
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
            "chart" => Recipe::Chart {
                widget: widget("the chart")?,
                data: required(&args.data, "--data <service result>", name)?,
                x: required(&args.x, "--x <field>", name)?,
                value: args.value.clone(),
                kind: args.kind.clone(),
            },
            "columns" => Recipe::Columns {
                widget: widget("the grid")?,
                data: args.data.clone(),
                columns: args.columns.clone(),
            },
            "picker" => Recipe::Picker {
                widget: widget("the dropdown or list")?,
                data: required(&args.data, "--data <service result>", name)?,
                display: required(&args.x, "--x <display field>", name)?,
                value: args.value.clone(),
                selected: args.selected.clone(),
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
    }
    json!({
        "applied": applied,
        "mashup": plan.edit.mashup,
        "recipe": plan.name,
        "added": plan.added,
        "wired": plan.wired,
        "files": files,
    })
}

/// One grid column as Composer writes it: the renderer, its format and the cell editor follow the
/// field's base type.
fn column_format(name: &str, title: &str, width: Value, base: &str) -> Value {
    let (renderer, format, editor) = match base {
        "STRING" | "TEXT" | "THINGNAME" | "USERNAME" | "GUID" => ("STRING", "full", "textfield"),
        "NUMBER" | "INTEGER" | "LONG" => ("NUMBER", "0.00", "textfield"),
        "BOOLEAN" => ("BOOLEAN", "checkbox", "checkbox"),
        "DATETIME" => ("DATETIME", "[[dateTimeFormat_Default]]", "datepicker"),
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

fn tracker_maps(
    fields: &[(String, String)],
    overrides: &[(String, String)],
) -> Result<Vec<(&'static str, String)>, AuthoringError> {
    let mut explicit = std::collections::BTreeMap::new();
    for (property, field) in overrides {
        if !TRACKER_MAPS.iter().any(|(name, _, _)| property == name) {
            return Err(AuthoringError::Invalid(format!(
                "unknown tracker map {property}; use {}",
                TRACKER_MAPS
                    .iter()
                    .map(|(name, _, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        if explicit.insert(property.as_str(), field.as_str()).is_some() {
            return Err(AuthoringError::Invalid(format!(
                "tracker map {property} is given twice"
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
    for &(property, aliases, bases) in TRACKER_MAPS {
        let field = if let Some(field) = explicit.get(property) {
            fields
                .iter()
                .find(|(name, _)| name == field)
                .ok_or_else(|| {
                    AuthoringError::Invalid(format!(
                        "tracker map {property} names missing field {field}; available fields: {}",
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
                [] if !TRACKER_REQUIRED.contains(&property) => continue,
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
                "tracker map {property} field {} is {}; use a {} field",
                field.0,
                field.1,
                bases.join("/")
            )));
        }
        maps.push((property, field.0.clone()));
    }
    if !unmatched.is_empty() {
        return Err(AuthoringError::Invalid(format!("tracker has unmatched field maps: {}; give --field-map Property=field for each (MCP field_maps); available fields: {}", unmatched.join("; "), fields.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", "))));
    }
    Ok(maps)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
