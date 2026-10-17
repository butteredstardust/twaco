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
use crate::core::{types, widgets};
use serde_json::{json, Value};

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
    /// A status message after a service completes (114 seen).
    Message {
        after: String,
        text: Option<String>,
        from: Option<String>,
        kind: String,
    },
    /// A confirmation before a click runs its handler (44 seen).
    Confirm {
        trigger: String,
        then: String,
        title: Option<String>,
        message: Option<String>,
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
    pub edit: EditPlan,
    pub name: &'static str,
    /// The widgets the recipe added.
    pub added: Vec<String>,
    /// What it wired, one line each, in `mashup describe` address form.
    pub wired: Vec<String>,
}

impl Recipe {
    pub fn name(&self) -> &'static str {
        match self {
            Self::EnableWhen { .. } => "enable-when",
            Self::ShowWhen { .. } => "show-when",
            Self::Text { .. } => "text",
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
}

impl Work<'_> {
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
        bind_in(
            &mut self.document,
            Some(self.solution),
            &self.context,
            from,
            to,
        )?;
        self.wired.push(format!("{from} -> {to}"));
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
pub fn plan_recipe(
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
        Recipe::Message {
            after,
            text,
            from,
            kind,
        } => {
            if !matches!(kind.as_str(), "info" | "warning" | "error") {
                return Err(AuthoringError::Invalid(
                    "the message type is info, warning or error".into(),
                ));
            }
            if text.is_some() == from.is_some() {
                return Err(AuthoringError::Invalid(
                    "give the message as --text, or bind it with --from".into(),
                ));
            }
            let mut properties = vec![("MessageType", json!(kind))];
            if let Some(text) = text {
                properties.push(("Message", json!(text)));
            }
            let id = work.add("statusmessage", &properties);
            if let Some(from) = from {
                work.bind(from, &format!("ui:{id}.Message"))?;
            }
            work.on(
                &call_event(after, "ServiceInvokeCompleted")?,
                &format!("ui:{id}.ShowMessage"),
            )?;
        }
        Recipe::Confirm {
            trigger,
            then,
            title,
            message,
        } => {
            let mut properties = Vec::new();
            if let Some(title) = title {
                properties.push(("TitleText", json!(title)));
            }
            if let Some(message) = message {
                properties.push(("MessageText", json!(message)));
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
            let mut properties = Vec::new();
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
            let fields = data.as_deref().and_then(|data| work.result_fields(data));
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
                let base = work.field(&fields, &name)?;
                format.push(column_format(&name, &title, width, &base));
            }
            work.set(
                &target,
                &[("ColumnFormat", json!({ "formatInfo": format }))],
            );
            if let Some(data) = data {
                work.bind(data, &format!("ui:{}.Data", target.id))?;
            }
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
        ..
    } = work;
    Ok(RecipePlan {
        edit: finish_edit(loaded, document, "mashup recipe")?,
        name: request.recipe.name(),
        added,
        wired,
    })
}

/// What a recipe can be given, as the command line and MCP both pass it.
#[derive(Clone, Debug, Default)]
pub struct RecipeArgs {
    /// The widget the recipe is about (enable-when, show-when, text) or its trigger (confirm, popup).
    pub widget: Option<String>,
    pub condition: Option<String>,
    pub expression: Option<String>,
    /// `name=address`, repeatable.
    pub inputs: Vec<String>,
    pub after: Option<String>,
    pub say: Option<String>,
    pub from: Option<String>,
    pub kind: Option<String>,
    pub then: Option<String>,
    pub title: Option<String>,
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
            "message" => Recipe::Message {
                after: required(&args.after, "--after <data call>", name)?,
                text: args.say.clone(),
                from: args.from.clone(),
                kind: args.kind.clone().unwrap_or_else(|| "info".into()),
            },
            "confirm" => Recipe::Confirm {
                trigger: widget("the button")?,
                then: required(&args.then, "--then <handler>", name)?,
                title: args.title.clone(),
                message: args.say.clone(),
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
