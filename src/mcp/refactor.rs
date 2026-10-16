use super::requests::refactor::{
    AdoptApplyRequest, AdoptKind, AdoptReportRequest, AdoptTakeSide, EntityNewRequest,
    HandoffAction, HandoffRequest, MashupAddContainedRequest, MashupAddRequest, MashupBindRequest,
    MashupDescribeRequest, MashupEventRequest, MashupNewRequest, MashupOnRequest,
    MashupParameterAction, MashupParameterRequest, MashupRecipeRequest, MashupTemplatesRequest,
    MashupValidateRequest, MoveMemberRequest, NewBuildingBlockRequest, RenameRequest,
    ReprojectRequest, RetemplateRequest, WidgetsRequest,
};
use super::source::add_types_refresh;
use super::*;

pub(crate) fn move_member_tool(
    solution: &Solution,
    arguments: MoveMemberRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let action = arguments.action.as_str();
    if !matches!(action, "move" | "copy") {
        return Err(ToolError::invalid(format!(
            "unknown action `{action}`; use `move` or `copy`"
        )));
    }
    let kind = arguments.kind.as_str();
    let member = relocate::Member::from_word(kind).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown kind `{kind}`; use `service` or `property`"
        ))
    })?;
    let request = relocate::Request {
        member,
        copy: action == "copy",
        from: nonempty(&arguments.from, "from")?.to_string(),
        to: nonempty(&arguments.to, "to")?.to_string(),
        name: nonempty(&arguments.name, "name")?.to_string(),
        new_name: arguments.new_name.as_ref().cloned(),
        leave_delegate: arguments.leave_delegate,
    };
    let request = commands::relocate::RelocateRequest {
        request,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp move_member".to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::relocate::execute(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let plan = outcome.plan();
    let problems = outcome.problems();
    let mut result = json!({
        "ok": problems.is_empty(), "action": action, "member": kind, "from": plan.request.from, "to": plan.request.to,
        "name": plan.request.name, "as": plan.final_name, "files": plan.files(solution),
        "callers": plan.callers, "notes": plan.notes, "out_of_step": problems,
    });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn new_building_block_tool(
    solution: &Solution,
    arguments: NewBuildingBlockRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let kind_word = arguments.r#type.as_str();
    let kind = newblock::BlockType::from_word(kind_word).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown type `{kind_word}`; use standard, abstract or implementation"
        ))
    })?;
    let request = newblock::Request {
        name: nonempty(&arguments.name, "name")?.to_string(),
        kind,
        display_name: arguments.display_name.as_ref().cloned(),
        description: arguments.description.as_ref().cloned().unwrap_or_default(),
        parent: arguments.parent.as_ref().cloned(),
        model_logic: arguments.model_logic,
        management_shape: arguments.management_shape,
        root: arguments.root.as_ref().cloned(),
        base_extension: arguments.base_extension.as_ref().cloned(),
    };
    let result_name = request.name.clone();
    let result_kind = request.kind;
    let request = commands::newblock::NewBlockRequest {
        request,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp new_building_block",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::newblock::execute(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let plan = outcome.plan();
    let files: Vec<String> = plan
        .files
        .iter()
        .map(|file| {
            file.path
                .strip_prefix(&solution.root)
                .unwrap_or(&file.path)
                .display()
                .to_string()
                .replace('\\', "/")
        })
        .collect();
    let mut result = json!({
        "ok": true, "name": result_name, "type": result_kind.word(), "root": plan.root,
        "files": files, "twaco_toml": plan.config_addition, "notes": plan.notes,
    });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn entity_new_tool(
    solution: &Solution,
    arguments: EntityNewRequest,
) -> Result<Value, ToolError> {
    let kind = crate::core::entity_new::Kind::parse(&arguments.kind)
        .ok_or_else(|| ToolError::invalid("kind must be a supported entity kind"))?;
    let request = crate::core::entity_new::NewRequest {
        kind,
        name: nonempty(&arguments.name, "name")?.to_string(),
        project: arguments.project.as_ref().cloned(),
        description: arguments.description.as_ref().cloned().unwrap_or_default(),
        visibility: arguments.visibility.items().to_vec(),
        template: arguments.template.as_ref().cloned(),
        shapes: arguments.shape.items().to_vec(),
        value_stream: arguments.value_stream.as_ref().cloned(),
        base: arguments.base.as_ref().cloned(),
        fields: arguments.field.items().to_vec(),
        every: arguments.every.as_ref().cloned(),
        cron: arguments.cron.as_ref().cloned(),
        run_as: arguments.run_as.as_ref().cloned(),
        disabled: arguments.disabled,
        then: arguments.then.as_ref().cloned(),
        script: arguments.script.as_ref().map(Into::into),
        data_shape: arguments.data_shape.as_ref().cloned(),
        indexes: arguments.index.items().to_vec(),
        file: arguments.file.as_ref().map(Into::into),
        background: arguments.background.as_ref().cloned(),
        foreground: arguments.foreground.as_ref().cloned(),
        secondary_background: arguments.secondary_background.as_ref().cloned(),
        line_color: arguments.line_color.as_ref().cloned(),
        line_style: arguments.line_style.as_ref().cloned(),
        line_thickness: arguments.line_thickness.as_ref().cloned(),
        bold: arguments.bold,
        italic: arguments.italic,
        underline: arguments.underline,
        text_size: arguments.text_size.as_ref().cloned(),
        image: arguments.image.as_ref().cloned(),
        display_string: arguments.display_string.as_ref().cloned(),
        state_type: arguments.state_type.as_ref().cloned(),
        states: arguments.state.items().to_vec(),
        items: arguments.item.items().to_vec(),
        default: arguments.default,
        label: arguments.label.as_ref().cloned(),
        from: arguments.from.as_ref().cloned(),
        props: arguments.prop.items().to_vec(),
        theme_content: None,
    };
    let dry_run = arguments.dry_run;
    let request = commands::entity_new::Request {
        request,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp entity_new",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::entity_new::execute(solution, &request, &mut notices)
        .map_err(ToolError::coded)?;
    let mut result = crate::core::entity_new::plan_json(solution, &outcome.plan, outcome.applied);
    result["dry_run"] = json!(dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn retemplate_tool(
    solution: &Solution,
    arguments: RetemplateRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = retemplate::Request {
        entity: nonempty(&arguments.entity, "entity")?.to_string(),
        template: arguments.template.as_ref().cloned(),
        add_shapes: arguments.add_shapes.items().to_vec(),
        remove_shapes: arguments.remove_shapes.items().to_vec(),
        accept_loss: arguments.accept_loss,
    };
    let request = commands::retemplate::RetemplateRequest {
        request,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp retemplate",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::retemplate::execute(solution, &request, &mut notices)
        .map_err(ToolError::coded)?;
    let plan = outcome.plan();
    let mut result = json!({
        "ok": true, "entity": plan.request.entity, "collection": plan.collection, "file": plan.file_relative(solution),
        "affected": plan.affected, "gained": plan.gained, "lost": plan.lost,
        "needs_accept_loss": plan.blocked, "notes": plan.notes,
    });
    result[if dry_run { "plan" } else { "applied" }] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn reproject_tool(
    solution: &Solution,
    arguments: ReprojectRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::reproject::ReprojectRequest {
        request: crate::core::reproject::Request {
            entities: arguments.entities,
            to: nonempty(&arguments.to, "to")?.to_string(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp reproject",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::reproject::execute(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result = crate::core::reproject::plan_json(solution, outcome.plan(), !dry_run);
    result["dry_run"] = json!(dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_describe_tool(
    solution: &Solution,
    arguments: MashupDescribeRequest,
) -> Result<Value, ToolError> {
    let description =
        crate::core::mashup_describe::describe(solution, nonempty(&arguments.mashup, "mashup")?)
            .map_err(ToolError::coded)?;
    Ok(serde_json::to_value(description).expect("a description serialises"))
}

pub(crate) fn mashup_validate_tool(
    solution: &Solution,
    arguments: MashupValidateRequest,
) -> Result<Value, ToolError> {
    let name = match (arguments.mashup.as_ref(), arguments.all) {
        (Some(name), false) => Some(nonempty(name, "mashup")?),
        (None, true) => None,
        _ => {
            return Err(ToolError::invalid(
                "give mashup, or all, not both or neither",
            ))
        }
    };
    let reports =
        crate::core::mashup_validate::validate(solution, name).map_err(ToolError::coded)?;
    let problems: usize = reports.iter().map(|report| report.problems.len()).sum();
    Ok(json!({ "problems": problems, "mashups": reports }))
}

pub(crate) fn mashup_new_tool(
    solution: &Solution,
    arguments: MashupNewRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    if !arguments.template.is_absent() && !arguments.layout.is_absent() {
        return Err(ToolError::invalid("give template or layout, not both"));
    }
    let template = arguments
        .template
        .as_ref()
        .or_else(|| arguments.layout.as_ref())
        .cloned()
        .unwrap_or_else(|| "blank".into());
    let request = commands::mashup::MashupNewRequest {
        request: crate::core::mashup_authoring::NewRequest {
            name: nonempty(&arguments.name, "name")?.to_string(),
            project: arguments.project.as_ref().cloned(),
            template,
            primary_menu: arguments.primary_menu.as_ref().cloned(),
            secondary_menu: arguments.secondary_menu.as_ref().cloned(),
            theme: arguments.theme.as_ref().cloned(),
            master: arguments.master,
            visibility: arguments.visibility.items().to_vec(),
            params: arguments.params.items().to_vec(),
            tabs: arguments.tabs.items().to_vec(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_new",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::new(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result =
        crate::core::mashup_authoring::new_plan_json(solution, &outcome.plan, outcome.applied);
    result["dry_run"] = json!(dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_templates_tool(arguments: MashupTemplatesRequest) -> Result<Value, ToolError> {
    match arguments.name.as_ref() {
        Some(name) => crate::core::mashup_templates_view::detail(name)
            .map_err(ToolError::invalid)?
            .map(|template| serde_json::to_value(template).expect("template serialises"))
            .ok_or_else(|| ToolError::invalid(format!("unknown template `{name}`"))),
        None => crate::core::mashup_templates_view::list()
            .map(|templates| json!({ "templates": templates }))
            .map_err(ToolError::invalid),
    }
}

pub(crate) fn mashup_add_tool(
    solution: &Solution,
    arguments: MashupAddRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let placements = [
        arguments
            .container
            .as_ref()
            .cloned()
            .map(crate::core::mashup_edit::Placement::In),
        arguments
            .right_of
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup_edit::Side::Right)),
        arguments
            .left_of
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup_edit::Side::Left)),
        arguments
            .above
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup_edit::Side::Above)),
        arguments
            .below
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup_edit::Side::Below)),
    ];
    let supplied: Vec<_> = placements.into_iter().flatten().collect();
    if supplied.len() > 1 {
        return Err(ToolError::invalid(
            "give one of in, right_of, left_of, above, below",
        ));
    }
    let request = commands::mashup::MashupAddRequest {
        request: crate::core::mashup_authoring::AddRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.to_string(),
            widget: nonempty(&arguments.widget, "widget")?.to_string(),
            inputs: arguments.inputs.items().to_vec(),
            placement: supplied.into_iter().next(),
            label: arguments.label.as_ref().cloned(),
            properties: arguments
                .properties
                .as_ref()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_add",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::add(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result =
        crate::core::mashup_authoring::add_plan_json(solution, &outcome.plan, outcome.applied);
    result["dry_run"] = json!(dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_add_contained_tool(
    solution: &Solution,
    arguments: MashupAddContainedRequest,
) -> Result<Value, ToolError> {
    let placements = [
        arguments
            .container
            .as_ref()
            .cloned()
            .map(crate::core::mashup_edit::Placement::In),
        arguments
            .right_of
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup_edit::Side::Right)),
        arguments
            .left_of
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup_edit::Side::Left)),
        arguments
            .above
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup_edit::Side::Above)),
        arguments
            .below
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup_edit::Side::Below)),
    ];
    let supplied: Vec<_> = placements.into_iter().flatten().collect();
    if supplied.len() > 1 {
        return Err(ToolError::invalid(
            "give one of in, right_of, left_of, above, below",
        ));
    }
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupAddContainedRequest {
        request: crate::core::mashup_authoring::AddContainedRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            child: nonempty(&arguments.child, "child")?.into(),
            placement: supplied.into_iter().next(),
            settings: arguments
                .set
                .as_ref()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_add_contained",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::mashup::add_contained(solution, &request, &mut notices)
        .map_err(ToolError::coded)?;
    let mut result = crate::core::mashup_authoring::add_contained_plan_json(
        solution,
        &outcome.plan,
        outcome.applied,
    );
    result["dry_run"] = json!(dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_parameter_tool(
    solution: &Solution,
    arguments: MashupParameterRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    if arguments.no_default && !arguments.default.is_absent() {
        return Err(ToolError::invalid("give default or no_default, not both"));
    }
    let action = match arguments.action {
        MashupParameterAction::Add => crate::core::mashup_authoring::ParameterAction::Add,
        MashupParameterAction::Set => crate::core::mashup_authoring::ParameterAction::Set,
        MashupParameterAction::Remove => crate::core::mashup_authoring::ParameterAction::Remove,
    };
    let default = if arguments.no_default {
        crate::core::mashup_authoring::ParameterDefault::Clear
    } else if let Some(value) = arguments.default.as_ref() {
        crate::core::mashup_authoring::ParameterDefault::Set(value.clone())
    } else {
        crate::core::mashup_authoring::ParameterDefault::Unchanged
    };
    let request = commands::mashup::MashupParameterRequest {
        request: crate::core::mashup_authoring::ParameterRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            action,
            name: nonempty(&arguments.name, "name")?.into(),
            base_type: arguments.r#type.as_ref().cloned(),
            direction: arguments.direction.as_ref().cloned(),
            description: arguments.description.as_ref().cloned(),
            default,
            data_shape: arguments.data_shape.as_ref().cloned(),
            force: arguments.force,
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_param",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::parameter(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result = crate::core::mashup_authoring::parameter_plan_json(
        solution,
        &outcome.plan,
        outcome.applied,
    );
    result["dry_run"] = json!(dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_recipe_tool(
    solution: &Solution,
    arguments: MashupRecipeRequest,
) -> Result<Value, ToolError> {
    use crate::core::mashup_authoring::{Recipe, RecipeArgs, RecipeRequest};
    let dry_run = arguments.dry_run;
    let recipe_args = RecipeArgs {
        widget: arguments.widget.as_ref().cloned(),
        condition: arguments.condition.as_ref().cloned(),
        expression: arguments.expression.as_ref().cloned(),
        inputs: arguments.inputs.items().to_vec(),
        after: arguments.after.as_ref().cloned(),
        say: arguments.say.as_ref().cloned(),
        from: arguments.from.as_ref().cloned(),
        kind: arguments.kind.as_ref().cloned(),
        then: arguments.then.as_ref().cloned(),
        title: arguments.title.as_ref().cloned(),
        on: arguments.on.as_ref().cloned(),
        target: arguments.target.as_ref().cloned(),
        window: arguments.window.as_ref().cloned(),
        size: arguments.size.as_ref().cloned(),
        pass: arguments.pass.items().to_vec(),
        reload: arguments.reload.as_ref().cloned(),
        every: arguments.every.as_ref().copied(),
        interval: arguments.interval.as_ref().cloned(),
        else_say: arguments.else_say.as_ref().cloned(),
        data: arguments.data.as_ref().cloned(),
        x: arguments.x.as_ref().cloned(),
        value: arguments.value.as_ref().cloned(),
        columns: arguments.columns.items().to_vec(),
        selected: arguments.selected.as_ref().cloned(),
    };
    let recipe = Recipe::from_args(nonempty(&arguments.recipe, "recipe")?, &recipe_args)
        .map_err(ToolError::coded)?;
    let request = commands::mashup::MashupRecipeRequest {
        request: RecipeRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            recipe,
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_recipe",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::recipe(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result =
        crate::core::mashup_authoring::recipe_plan_json(solution, &outcome.plan, outcome.applied);
    result["dry_run"] = json!(dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_event_tool(
    solution: &Solution,
    arguments: MashupEventRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupEventRequest {
        request: crate::core::mashup_authoring::EventRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            name: nonempty(&arguments.name, "name")?.into(),
            when: nonempty(&arguments.when, "when")?.into(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_event",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::event(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result = crate::core::mashup_authoring::parameter_plan_json(
        solution,
        &outcome.plan,
        outcome.applied,
    );
    result["dry_run"] = json!(dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_bind_tool(
    solution: &Solution,
    arguments: MashupBindRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupBindRequest {
        request: crate::core::mashup_authoring::BindRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.to_string(),
            from: nonempty(&arguments.from, "from")?.to_string(),
            to: nonempty(&arguments.to, "to")?.to_string(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_bind",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::bind(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let relative = |path: &std::path::Path| {
        path.strip_prefix(&solution.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut files = vec![relative(&outcome.plan.edit.entity_path)];
    if let Some(sidecars) = &outcome.plan.edit.sidecars {
        files.push(relative(&sidecars.content_path));
    }
    let mut result = json!({
        "applied": outcome.applied,
        "dry_run": dry_run,
        "mashup": outcome.plan.edit.mashup,
        "from": outcome.plan.from,
        "to": outcome.plan.to,
        "new_source": outcome.plan.new_source,
        "warnings": outcome.plan.warnings,
        "refreshed": outcome.plan.refreshed,
        "files": files,
    });
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_on_tool(
    solution: &Solution,
    arguments: MashupOnRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupOnRequest {
        request: crate::core::mashup_authoring::OnRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.to_string(),
            event: nonempty(&arguments.event, "event")?.to_string(),
            invoke: nonempty(&arguments.invoke, "invoke")?.to_string(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_on",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::on(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let relative = |path: &std::path::Path| {
        path.strip_prefix(&solution.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut files = vec![relative(&outcome.plan.edit.entity_path)];
    if let Some(sidecars) = &outcome.plan.edit.sidecars {
        files.push(relative(&sidecars.content_path));
    }
    let mut result = json!({
        "applied": outcome.applied, "dry_run": dry_run, "mashup": outcome.plan.edit.mashup,
        "event": outcome.plan.event, "invoke": outcome.plan.invoke,
        "new_source": outcome.plan.new_source, "files": files,
    });
    add_notices(&mut result, &notices);
    Ok(result)
}

fn beside(
    anchor: String,
    side: crate::core::mashup_edit::Side,
) -> crate::core::mashup_edit::Placement {
    crate::core::mashup_edit::Placement::Beside { anchor, side }
}

pub(crate) fn widgets_tool(arguments: WidgetsRequest) -> Result<Value, ToolError> {
    match (arguments.widget.as_ref(), arguments.search.as_ref()) {
        (Some(_), Some(_)) => Err(ToolError::invalid("give a widget or search, not both")),
        (Some(widget), None) => crate::core::widgets_view::detail(widget)
            .map(|detail| serde_json::to_value(detail).expect("a widget serialises"))
            .map_err(ToolError::coded),
        (None, search) => {
            Ok(json!({ "widgets": crate::core::widgets_view::list(search.map(String::as_str)) }))
        }
    }
}

pub(crate) fn adopt_apply_tool(
    solution: &Solution,
    arguments: AdoptApplyRequest,
) -> Result<Value, ToolError> {
    let export = nonempty(&arguments.export, "export")?;
    let export = {
        let path = PathBuf::from(export);
        if path.is_absolute() {
            path
        } else {
            solution.root.join(path)
        }
    };
    let only = arguments.entity.items().to_vec();
    let request = commands::adopt::AdoptRequest {
        export,
        only,
        base: arguments.base.as_ref().cloned(),
        only_kind: arguments.only.as_ref().map(adopt_kind),
        takes: arguments
            .take
            .items()
            .iter()
            .map(|take| adopt::Take {
                side: match take.side {
                    AdoptTakeSide::Theirs => adopt::TakeSide::Theirs,
                    AdoptTakeSide::Ours => adopt::TakeSide::Ours,
                },
                target: take.target.clone(),
            })
            .collect(),
        mode: Mode::Apply,
        lock_label: "mcp adopt_apply",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::adopt::execute(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let commands::adopt::AdoptOutcome::Applied {
        report, outcome, ..
    } = outcome
    else {
        unreachable!("an adopt apply request has an applied outcome")
    };
    let mashup_written = report.entities.iter().any(|entry| {
        entry.entity.collection == "Mashups"
            && outcome
                .lines
                .iter()
                .any(|line| line == &format!("sidecar  {}", entry.entity.name))
    });
    let next = if mashup_written {
        "backend changes are already folded; sync mashup sidecars, then check"
    } else {
        "backend changes are already folded; then check"
    };
    let mut result = json!({
        "applied": outcome.lines,
        "reverts_not_applied": report.reverts().count(),
        "base": report.base,
        "entities": report.entities.iter().map(adopt_entity_json).collect::<Vec<_>>(),
        "services": report.services.iter().map(adopt_service_json).collect::<Vec<_>>(),
        "next": next,
    });
    add_types_refresh(&mut result, &outcome.types);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn adopt_tool(
    solution: &Solution,
    arguments: AdoptReportRequest,
) -> Result<Value, ToolError> {
    let export = nonempty(&arguments.export, "export")?;
    let export = {
        let path = PathBuf::from(export);
        if path.is_absolute() {
            path
        } else {
            solution.root.join(path)
        }
    };
    let only = arguments.entity.items().to_vec();
    let request = commands::adopt::AdoptRequest {
        export,
        only,
        base: arguments.base.as_ref().cloned(),
        only_kind: arguments.only.as_ref().map(adopt_kind),
        takes: Vec::new(),
        mode: Mode::Plan,
        lock_label: "mcp adopt_report",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::adopt::execute(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let commands::adopt::AdoptOutcome::Plan { report, .. } = outcome else {
        unreachable!("an adopt report request has a plan outcome")
    };
    let detail = arguments.detail;
    let services: Vec<Value> = report
        .services
        .iter()
        .map(|s| {
            let mut item = adopt_service_json(s);
            item["compared_with"] = json!(relative(solution, &s.source));
            item
        })
        .collect();
    let changed: Vec<Value> = report
        .with_status(adopt::Status::Changed)
        .map(|e| {
            let mut entry = json!({
                "entity": e.entity.path(),
                "nodes": e.differences.len(),
                "regenerated_ids": e.volatile_ids,
                "ignored": e.ignored,
                "change": change_word(e.change),
                "kind": kind_word(e.kind),
            });
            if detail {
                entry["differences"] = json!(e
                    .differences
                    .iter()
                    .map(|d| json!({ "path": d.path, "export": d.export, "repo": d.repo }))
                    .collect::<Vec<_>>());
            }
            entry
        })
        .collect();
    let mut result = json!({
        "reverts": report.reverts().count(),
        "services": services,
        "unmatched_services": report.unmatched_services,
        "new": report.with_status(adopt::Status::New).map(|e| json!({ "entity": e.entity.path(), "project": e.project })).collect::<Vec<_>>(),
        "absent": report.absent.iter().map(adopt::EntityRef::path).collect::<Vec<_>>(),
        "changed": changed,
        "identical": report.with_status(adopt::Status::Identical).count(),
        "base": report.base,
        "entities": report.entities.iter().map(adopt_entity_json).collect::<Vec<_>>(),
    });
    add_notices(&mut result, &notices);
    Ok(result)
}

fn adopt_kind(kind: &AdoptKind) -> adopt::Kind {
    match kind {
        AdoptKind::Ui => adopt::Kind::Ui,
        AdoptKind::Backend => adopt::Kind::Backend,
    }
}
fn change_word(change: adopt::Change) -> &'static str {
    match change {
        adopt::Change::Same => "same",
        adopt::Change::Stale => "stale",
        adopt::Change::Theirs => "theirs",
        adopt::Change::Conflict => "conflict",
        adopt::Change::Added => "added",
        adopt::Change::WeRemoved => "we_removed",
        adopt::Change::Unknown => "unknown",
    }
}
fn kind_word(kind: adopt::Kind) -> &'static str {
    match kind {
        adopt::Kind::Ui => "ui",
        adopt::Kind::Backend => "backend",
    }
}
fn adopt_entity_json(entry: &adopt::EntityReport) -> Value {
    json!({ "entity": entry.entity.path(), "change": change_word(entry.change), "kind": kind_word(entry.kind) })
}
fn adopt_service_json(service: &adopt::ServiceReport) -> Value {
    json!({ "entity": service.entity, "service": service.service, "generated": service.generated, "change": change_word(service.change), "kind": "backend" })
}

pub(crate) fn handoff_tool(
    solution: &Solution,
    arguments: HandoffRequest,
) -> Result<Value, ToolError> {
    let request = match arguments.action {
        HandoffAction::List => commands::handoff::HandoffRequest::List,
        HandoffAction::Record => {
            let name = required_text(&arguments.name, "name")?.to_string();
            let files = arguments
                .files
                .items()
                .iter()
                .map(|file| {
                    let path = PathBuf::from(file);
                    if path.is_absolute() {
                        path
                    } else {
                        solution.root.join(path)
                    }
                })
                .collect();
            commands::handoff::HandoffRequest::Record { name, files }
        }
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::handoff::execute(
        solution,
        &request,
        if arguments.dry_run {
            Mode::Plan
        } else {
            Mode::Apply
        },
        "mcp handoff",
        &mut notices,
    )
    .map_err(ToolError::coded)?;
    let mut result = match outcome {
        commands::handoff::HandoffOutcome::Listed { handoffs, .. } => {
            json!({ "ok": true, "handoffs": handoffs })
        }
        commands::handoff::HandoffOutcome::Planned { name, files, .. } => {
            json!({ "ok": true, "plan": true, "name": name, "files": files.iter().map(|file| relative(solution, file)).collect::<Vec<_>>() })
        }
        commands::handoff::HandoffOutcome::Recorded { handoff, .. } => {
            json!({ "ok": true, "recorded": handoff })
        }
    };
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn rename_tool(
    solution: &Solution,
    arguments: RenameRequest,
) -> Result<Value, ToolError> {
    let word = arguments.kind.as_str();
    let kind = rename::Kind::from_word(word).ok_or_else(|| {
        ToolError::invalid(format!(
            "unknown rename kind `{word}`; use {}",
            rename::Kind::list_words()
        ))
    })?;
    let dry_run = arguments.dry_run;
    let request = rename::Request {
        kind,
        scope: arguments.scope.as_ref().cloned(),
        service: arguments.service.as_ref().cloned(),
        old: nonempty(&arguments.old, "old")?.to_string(),
        new: nonempty(&arguments.new, "new")?.to_string(),
        apply: !dry_run,
        include_outside: arguments.include_outside,
        skip_checks: arguments.skip_checks,
        expect_digest: arguments.plan_digest.as_ref().cloned(),
        database: rename::DatabaseFlags {
            sql: arguments.sql,
            no_sql: arguments.no_sql,
            dir: arguments.sql_dir.as_ref().cloned(),
        },
    };
    let request = commands::rename::RenameRequest {
        request,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        date: jiff::Zoned::now().strftime("%Y-%m-%d").to_string(),
        lock_label: "mcp rename".to_string(),
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::rename::execute(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result =
        rename::summary_json(solution, outcome.outcome(), arguments.include_outside, 10);
    add_notices(&mut result, &notices);
    Ok(result)
}
