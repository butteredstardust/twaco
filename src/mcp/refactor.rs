use super::requests::refactor::{
    AdoptApplyRequest, AdoptKind, AdoptReportRequest, AdoptTakeSide, EntityEditAction,
    EntityEditRequest, EntityNewRequest, HandoffAction, HandoffRequest, MashupAddContainedRequest,
    MashupAddRequest, MashupBindRequest, MashupCollectionAddRequest, MashupDescribeRequest,
    MashupDiffRequest, MashupEventRequest, MashupGapRequest, MashupNewRequest, MashupOffRequest,
    MashupOnRequest, MashupParameterAction, MashupParameterRequest, MashupRecipeRequest,
    MashupRemoveRequest, MashupSetRequest, MashupSourceAddRequest, MashupSourceListRequest,
    MashupSourceRemoveRequest, MashupSourceRetargetRequest, MashupTemplatesRequest,
    MashupUnbindRequest, MashupValidateRequest, MoveMemberAction, MoveMemberRequest,
    NewBuildingBlockRequest, RenameRequest, ReprojectRequest, RetemplateRequest, WidgetsRequest,
};
use super::source::add_types_refresh;
use super::{
    add_notices, adopt, commands, json, newblock, nonempty, outcome_key, relative, relocate,
    rename, required_text, retemplate, Mode, PathBuf, Solution, ToolError, Value,
};

pub(crate) fn move_member_tool(
    solution: &Solution,
    arguments: MoveMemberRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = relocate::Request {
        member: arguments.kind.into(),
        copy: arguments.action == MoveMemberAction::Copy,
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
        "ok": problems.is_empty(), "action": arguments.action, "member": arguments.kind, "from": plan.request.from, "to": plan.request.to,
        "name": plan.request.name, "as": plan.final_name, "files": plan.files(solution),
        "callers": plan.callers, "notes": plan.notes, "out_of_step": problems,
    });
    result[outcome_key(dry_run)] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn new_building_block_tool(
    solution: &Solution,
    arguments: NewBuildingBlockRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = newblock::Request {
        name: nonempty(&arguments.name, "name")?.to_string(),
        kind: arguments.r#type.into(),
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
        .map(|file| solution.relative(&file.path))
        .collect();
    let mut result = json!({
        "ok": true, "name": result_name, "type": result_kind.word(), "root": plan.root,
        "files": files, "twaco_toml": plan.config_addition, "notes": plan.notes,
    });
    result[outcome_key(dry_run)] = json!(true);
    add_notices(&mut result, &notices);
    Ok(result)
}

/// `entity_new` from the root: a first project in a folder with no twaco.toml creates the file.
pub(crate) fn entity_new_root_tool(
    root: &std::path::Path,
    arguments: EntityNewRequest,
) -> Result<Value, ToolError> {
    let project = arguments.kind == super::requests::refactor::EntityNewKind::Project;
    match Solution::discover(root) {
        Ok(solution) => entity_new_tool(&solution, arguments),
        Err(crate::core::config::ConfigError::NotFound { .. }) if project => {
            entity_new_tool(&Solution::bare(root), arguments)
        }
        Err(error) => Err(ToolError::coded(error)),
    }
}

pub(crate) fn entity_new_tool(
    solution: &Solution,
    arguments: EntityNewRequest,
) -> Result<Value, ToolError> {
    let request = crate::core::entity::new::NewRequest {
        kind: arguments.kind.into(),
        name: nonempty(&arguments.name, "name")?.to_string(),
        project: arguments.project.as_ref().cloned(),
        description: arguments.description.as_ref().cloned().unwrap_or_default(),
        depends_on: arguments.depends_on.items().to_vec(),
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
        tools: arguments.tool.items().to_vec(),
        service_resources: arguments.service_resource.items().to_vec(),
        file_resources: arguments.file_resource.items().to_vec(),
        prompts: arguments.prompt.items().to_vec(),
        describe: arguments.describe.items().to_vec(),
        namespaces: arguments.namespace.items().to_vec(),
        skills: arguments.skill.items().to_vec(),
        resolver: arguments.resolver.as_ref().cloned(),
        taxonomy: arguments.taxonomy.items().to_vec(),
        instructions: arguments.instructions.as_ref().cloned(),
        instructions_file: arguments.instructions_file.as_ref().map(Into::into),
        agent_description: arguments.agent_description.as_ref().cloned(),
        display_name: arguments.display_name.as_ref().cloned(),
        theme_content: None,
    };
    let dry_run = arguments.dry_run;
    let request = commands::entity_new::Request {
        request,
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp entity_new",
        fetch_profile: Some("default".to_string()),
    };
    let mut notices = commands::Notices::default();
    let outcome = match commands::entity_new::execute(solution, &request, &mut notices) {
        Ok(outcome) => outcome,
        Err(error) if notices.lines().is_empty() => return Err(ToolError::coded(error)),
        // What fetching into the platform cache said is often why a template is unknown.
        Err(error) => {
            use crate::core::codes::Coded;
            return Err(ToolError::with(
                error.code(),
                format!("{error}; {}", notices.lines().join("; ")),
            ));
        }
    };
    let mut result = crate::core::entity::new::plan_json(solution, &outcome.plan, outcome.applied);
    result["dry_run"] = json!(dry_run);
    if !outcome.fetched.is_empty() {
        result["platform_cache"] = json!(outcome.fetched);
    }
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn entity_edit_tool(
    solution: &Solution,
    arguments: EntityEditRequest,
) -> Result<Value, ToolError> {
    use crate::core::entity::edit::{
        Action, AgentChange, OverrideSource, Property, Request, Service, StyleChange,
    };
    use crate::mcp::requests::common::Absent;
    let text = |value: &Absent<String>| value.as_ref().cloned();
    let need = |value: &Absent<String>, what: &str| -> Result<String, ToolError> {
        value
            .as_ref()
            .filter(|value| !value.is_empty())
            .cloned()
            .ok_or_else(|| ToolError::invalid(format!("this action needs {what}")))
    };
    let flag = |value: &Absent<bool>| value.as_ref().copied();
    let property = || -> Result<Property, ToolError> {
        Ok(Property {
            name: need(&arguments.name, "name")?,
            base_type: text(&arguments.base_type).map(|value| value.to_uppercase()),
            default: text(&arguments.default),
            no_default: arguments.no_default,
            persistent: flag(&arguments.persistent),
            logged: flag(&arguments.logged),
            read_only: flag(&arguments.read_only),
            category: text(&arguments.category),
            units: text(&arguments.units),
            data_shape: text(&arguments.data_shape),
            description: text(&arguments.description),
        })
    };
    if arguments.from_parent && !matches!(arguments.action, EntityEditAction::ServiceOverride) {
        return Err(ToolError::invalid(
            "from_parent belongs to service_override",
        ));
    }
    let action = match arguments.action {
        EntityEditAction::PropertyAdd => {
            need(&arguments.base_type, "type")?;
            Action::PropertyAdd(property()?)
        }
        EntityEditAction::PropertySet => Action::PropertySet {
            property: property()?,
            force: arguments.force,
        },
        EntityEditAction::PropertyRemove => Action::PropertyRemove {
            name: need(&arguments.name, "name")?,
            force: arguments.force,
        },
        EntityEditAction::SubscriptionAdd => Action::SubscriptionAdd {
            name: need(&arguments.name, "name")?,
            event: need(&arguments.event, "event")?,
            source: text(&arguments.source),
            source_property: text(&arguments.source_property),
            code: need(&arguments.then, "then (the script)")?,
        },
        EntityEditAction::SubscriptionRemove => Action::SubscriptionRemove {
            name: need(&arguments.name, "name")?,
        },
        EntityEditAction::ServiceAdd => Action::ServiceAdd(Service {
            name: need(&arguments.name, "name")?,
            parameters: arguments.params.items().to_vec(),
            returns: text(&arguments.returns),
            description: text(&arguments.description),
            category: text(&arguments.category),
            private: arguments.private,
            code: text(&arguments.then).unwrap_or_default(),
        }),
        EntityEditAction::ServiceRemove => Action::ServiceRemove {
            name: need(&arguments.name, "name")?,
            force: arguments.force,
        },
        EntityEditAction::ServiceOverride => {
            // The signature is the ancestor's: as on the command line, these are refused.
            if !arguments.params.is_absent()
                || !arguments.returns.is_absent()
                || !arguments.description.is_absent()
                || arguments.private
                || arguments.force
            {
                return Err(ToolError::invalid(
                    "service_override takes name, then and from_parent only: the signature is the inherited one",
                ));
            }
            let source = match (text(&arguments.then), arguments.from_parent) {
                (Some(_), true) => {
                    return Err(ToolError::invalid(
                        "service_override takes then or from_parent, not both",
                    ))
                }
                (Some(code), false) => OverrideSource::Code(code),
                (None, true) => OverrideSource::Parent,
                (None, false) => OverrideSource::Code("// TODO: implement override.\n".to_string()),
            };
            Action::ServiceOverride {
                name: need(&arguments.name, "name")?,
                source,
            }
        }
        EntityEditAction::ServiceRevert => Action::ServiceRevert {
            name: need(&arguments.name, "name")?,
        },
        EntityEditAction::ShapeAdd => Action::ShapeAdd {
            shape: need(&arguments.shape, "shape")?,
        },
        EntityEditAction::ShapeRemove => Action::ShapeRemove {
            shape: need(&arguments.shape, "shape")?,
            force: arguments.force,
        },
        EntityEditAction::TimerSet => Action::TimerSet {
            every: text(&arguments.every),
            run_as: text(&arguments.run_as),
            enabled: flag(&arguments.enabled),
        },
        EntityEditAction::SchedulerSet => Action::SchedulerSet {
            cron: text(&arguments.cron),
            run_as: text(&arguments.run_as),
            enabled: flag(&arguments.enabled),
        },
        EntityEditAction::StyleSet => Action::StyleSet(StyleChange {
            background: text(&arguments.background),
            foreground: text(&arguments.foreground),
            secondary_background: text(&arguments.secondary_background),
            line_color: text(&arguments.line_color),
            line_style: text(&arguments.line_style),
            line_thickness: text(&arguments.line_thickness),
            bold: flag(&arguments.bold),
            italic: flag(&arguments.italic),
            underline: flag(&arguments.underline),
            text_size: text(&arguments.text_size),
            image: text(&arguments.image),
            display_string: text(&arguments.display_string),
        }),
        EntityEditAction::MediaSet => Action::MediaSet {
            file: need(&arguments.file, "file")?.into(),
        },
        EntityEditAction::MenuItemAdd => Action::MenuItemAdd {
            title: need(&arguments.title, "title")?,
            mashup: need(&arguments.mashup, "mashup")?,
            default: arguments.is_default,
            before: text(&arguments.before),
        },
        EntityEditAction::MenuItemRemove => Action::MenuItemRemove {
            title: need(&arguments.title, "title")?,
        },
        EntityEditAction::StateSet => Action::StateSet {
            state_type: need(&arguments.state_type, "state_type")?,
            states: arguments.state.items().to_vec(),
        },
        EntityEditAction::McpAdd => Action::McpAdd {
            kind: need(&arguments.item_kind, "item_kind")?,
            spec: need(&arguments.spec, "spec")?,
            describe: text(&arguments.description),
        },
        EntityEditAction::McpRemove => Action::McpRemove {
            name: need(&arguments.name, "name")?,
        },
        EntityEditAction::AgentAllow => Action::AgentAllow {
            spec: need(&arguments.spec, "spec")?,
        },
        EntityEditAction::AgentDisallow => Action::AgentDisallow {
            spec: need(&arguments.spec, "spec")?,
        },
        EntityEditAction::AgentTaxonomyAdd => Action::AgentTaxonomyAdd {
            spec: need(&arguments.spec, "spec")?,
        },
        EntityEditAction::AgentTaxonomyRemove => Action::AgentTaxonomyRemove {
            name: need(&arguments.name, "name")?,
        },
        EntityEditAction::AgentSkillAdd => Action::AgentSkillAdd {
            spec: need(&arguments.spec, "spec")?,
        },
        EntityEditAction::AgentSkillRemove => Action::AgentSkillRemove {
            spec: need(&arguments.spec, "spec")?,
        },
        EntityEditAction::AgentSet => Action::AgentSet(AgentChange {
            resolver: arguments.resolver.as_ref().cloned(),
            instructions: arguments.instructions.as_ref().cloned(),
            instructions_file: arguments.instructions_file.as_ref().map(Into::into),
            agent_description: arguments.agent_description.as_ref().cloned(),
            display_name: arguments.display_name.as_ref().cloned(),
        }),
    };
    let dry_run = arguments.dry_run;
    let request = commands::entity_edit::EditRequest {
        request: Request {
            entity: nonempty(&arguments.entity, "entity")?.to_string(),
            action,
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp entity_edit",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::entity_edit::execute(solution, &request, &mut notices)
        .map_err(ToolError::coded)?;
    let mut result = crate::core::entity::edit::plan_json(solution, &outcome.plan, outcome.applied);
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
    result[outcome_key(dry_run)] = json!(true);
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
        crate::core::mashup::describe::describe(solution, nonempty(&arguments.mashup, "mashup")?)
            .map_err(ToolError::coded)?;
    Ok(serde_json::to_value(description).expect("a description serialises"))
}

pub(crate) fn mashup_diff_tool(
    solution: &Solution,
    arguments: MashupDiffRequest,
) -> Result<Value, ToolError> {
    let tier = arguments.tier.as_ref().map(String::as_str).unwrap_or("all");
    let tier = crate::core::mashup::diff::Tier::parse(tier)
        .ok_or_else(|| ToolError::invalid("tier must be executable, layout, visual or all"))?;
    let result = crate::core::mashup::diff::diff(
        solution,
        nonempty(&arguments.mashup_a, "mashup_a")?,
        nonempty(&arguments.mashup_b, "mashup_b")?,
        tier,
    )
    .map_err(ToolError::coded)?;
    Ok(serde_json::to_value(result).expect("a diff serialises"))
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
        crate::core::mashup::validate::validate(solution, name).map_err(ToolError::coded)?;
    let counts = crate::core::mashup::validate::problem_counts(&reports);
    Ok(json!({
        "problems": counts.problems,
        "errors": counts.errors,
        "advisories": counts.advisories,
        "mashups": reports,
    }))
}

pub(crate) fn mashup_new_tool(
    solution: &Solution,
    arguments: MashupNewRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    if !arguments.template.is_absent() && !arguments.layout.is_absent() {
        return Err(ToolError::invalid("give template or layout, not both"));
    }
    if !arguments.from.is_absent()
        && (!arguments.template.is_absent()
            || !arguments.layout.is_absent()
            || !arguments.primary_menu.is_absent()
            || !arguments.secondary_menu.is_absent()
            || !arguments.theme.is_absent()
            || arguments.master
            || !arguments.params.is_absent()
            || !arguments.tabs.is_absent())
    {
        return Err(ToolError::invalid(
            "from cannot be used with template, layout, menus, theme, master, params or tabs",
        ));
    }
    let template = arguments
        .template
        .as_ref()
        .or_else(|| arguments.layout.as_ref())
        .cloned()
        .unwrap_or_else(|| "blank".into());
    let request = commands::mashup::MashupNewRequest {
        request: crate::core::mashup::authoring::NewRequest {
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
            from: arguments.from.as_ref().cloned(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_new",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::new(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result =
        crate::core::mashup::authoring::new_plan_json(solution, &outcome.plan, outcome.applied);
    result["dry_run"] = json!(dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_templates_tool(arguments: MashupTemplatesRequest) -> Result<Value, ToolError> {
    match arguments.name.as_ref() {
        Some(name) => crate::core::mashup::templates_view::detail(name)
            .map_err(ToolError::invalid)?
            .map(|template| serde_json::to_value(template).expect("template serialises"))
            .ok_or_else(|| ToolError::invalid(format!("unknown template `{name}`"))),
        None => crate::core::mashup::templates_view::list()
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
            .map(crate::core::mashup::edit::Placement::In),
        arguments
            .right_of
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Right)),
        arguments
            .left_of
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Left)),
        arguments
            .above
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Above)),
        arguments
            .below
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Below)),
    ];
    let mut supplied: Vec<_> = placements.into_iter().flatten().collect();
    if supplied.len() > 1 {
        return Err(ToolError::invalid(
            "give one of in, right_of, left_of, above, below",
        ));
    }
    if arguments.end {
        use crate::core::mashup::edit::{Placement, Side};
        match supplied.first_mut() {
            Some(Placement::Beside { side, .. }) if *side == Side::Right => *side = Side::End,
            _ => return Err(ToolError::invalid("end goes with right_of")),
        }
    }
    if arguments.bottom {
        use crate::core::mashup::edit::Placement;
        match supplied.first_mut() {
            Some(placement @ Placement::In(_)) => {
                if let Placement::In(column) = placement.clone() {
                    *placement = Placement::Bottom(column);
                }
            }
            _ => return Err(ToolError::invalid("bottom goes with in")),
        }
    }
    let request = commands::mashup::MashupAddRequest {
        request: crate::core::mashup::authoring::AddRequest {
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
        crate::core::mashup::authoring::add_plan_json(solution, &outcome.plan, outcome.applied);
    result["dry_run"] = json!(dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_collection_add_tool(
    solution: &Solution,
    arguments: MashupCollectionAddRequest,
) -> Result<Value, ToolError> {
    let placements = [
        arguments
            .container
            .as_ref()
            .cloned()
            .map(crate::core::mashup::edit::Placement::In),
        arguments
            .right_of
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Right)),
        arguments
            .left_of
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Left)),
        arguments
            .above
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Above)),
        arguments
            .below
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Below)),
    ];
    let supplied: Vec<_> = placements.into_iter().flatten().collect();
    if supplied.len() > 1 {
        return Err(ToolError::invalid(
            "give one of in, right_of, left_of, above, below",
        ));
    }
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupCollectionAddRequest {
        request: crate::core::mashup::authoring::CollectionAddRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            cell: nonempty(&arguments.cell_mashup, "cell_mashup")?.into(),
            data: nonempty(&arguments.data, "data")?.into(),
            mappings: arguments.map.into_iter().collect(),
            uid: arguments.uid.as_ref().cloned(),
            width: arguments.width.as_ref().copied(),
            height: arguments.height.as_ref().copied(),
            placement: supplied.into_iter().next(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_collection_add",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::mashup::collection_add(solution, &request, &mut notices)
        .map_err(ToolError::coded)?;
    let mut result = crate::core::mashup::authoring::collection_add_plan_json(
        solution,
        &outcome.plan,
        outcome.applied,
    );
    result["dry_run"] = json!(dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_state_format_set_tool(
    solution: &Solution,
    arguments: super::requests::refactor::MashupStateFormatSetRequest,
) -> Result<Value, ToolError> {
    let request = commands::mashup::MashupStateFormatSetRequest {
        request: crate::core::mashup::authoring::StateFormatSetRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            widget: nonempty(&arguments.widget, "widget")?.into(),
            state: nonempty(&arguments.state, "state")?.into(),
            property: arguments.property.as_ref().cloned(),
            column: arguments.column.as_ref().cloned(),
            field: arguments.field.as_ref().cloned(),
            icon_only: arguments.icon_only,
        },
        mode: if arguments.dry_run {
            Mode::Plan
        } else {
            Mode::Apply
        },
        lock_label: "mcp mashup_state_format_set",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::mashup::state_format_set(solution, &request, &mut notices)
        .map_err(ToolError::coded)?;
    let mut result =
        crate::core::mashup::authoring::set_plan_json(solution, &outcome.plan, outcome.applied);
    result["dry_run"] = json!(arguments.dry_run);
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_set_tool(
    solution: &Solution,
    arguments: MashupSetRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    if arguments.properties.is_empty()
        && arguments.style_from.is_absent()
        && arguments.styles.is_empty()
    {
        return Err(ToolError::invalid(
            "properties, styles and style_from must not all be empty",
        ));
    }
    let request = commands::mashup::MashupSetRequest {
        request: crate::core::mashup::authoring::SetRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.to_string(),
            widget: nonempty(&arguments.widget, "widget")?.to_string(),
            properties: arguments.properties.into_iter().collect(),
            style_from: arguments.style_from.as_ref().cloned(),
            styles: arguments.styles,
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_set",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::set(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result =
        crate::core::mashup::authoring::set_plan_json(solution, &outcome.plan, outcome.applied);
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
            .map(crate::core::mashup::edit::Placement::In),
        arguments
            .right_of
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Right)),
        arguments
            .left_of
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Left)),
        arguments
            .above
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Above)),
        arguments
            .below
            .as_ref()
            .cloned()
            .map(|anchor| beside(anchor, crate::core::mashup::edit::Side::Below)),
    ];
    let supplied: Vec<_> = placements.into_iter().flatten().collect();
    if supplied.len() > 1 {
        return Err(ToolError::invalid(
            "give one of in, right_of, left_of, above, below",
        ));
    }
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupAddContainedRequest {
        request: crate::core::mashup::authoring::AddContainedRequest {
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
    let mut result = crate::core::mashup::authoring::add_contained_plan_json(
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
        MashupParameterAction::Add => crate::core::mashup::authoring::ParameterAction::Add,
        MashupParameterAction::Set => crate::core::mashup::authoring::ParameterAction::Set,
        MashupParameterAction::Remove => crate::core::mashup::authoring::ParameterAction::Remove,
    };
    let default = if arguments.no_default {
        crate::core::mashup::authoring::ParameterDefault::Clear
    } else if let Some(value) = arguments.default.as_ref() {
        crate::core::mashup::authoring::ParameterDefault::Set(value.clone())
    } else {
        crate::core::mashup::authoring::ParameterDefault::Unchanged
    };
    let request = commands::mashup::MashupParameterRequest {
        request: crate::core::mashup::authoring::ParameterRequest {
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
    let mut result = crate::core::mashup::authoring::parameter_plan_json(
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
    use crate::core::mashup::authoring::{Recipe, RecipeArgs, RecipeRequest};
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
        crate::core::mashup::authoring::recipe_plan_json(solution, &outcome.plan, outcome.applied);
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
        request: crate::core::mashup::authoring::EventRequest {
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
    let mut result = crate::core::mashup::authoring::parameter_plan_json(
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
        request: crate::core::mashup::authoring::BindRequest {
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
    let relative = |path: &std::path::Path| solution.relative(path);
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

pub(crate) fn mashup_source_add_tool(
    solution: &Solution,
    arguments: MashupSourceAddRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupSourceAddRequest {
        request: crate::core::mashup::authoring::SourceAddRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            entity: nonempty(&arguments.entity, "entity")?.into(),
            service: nonempty(&arguments.service, "service")?.into(),
            dynamic: arguments.dynamic,
            on_load: arguments.on_load,
            alias: arguments.alias.as_ref().cloned(),
            entity_from: arguments.entity_from.as_ref().cloned(),
            refresh: arguments.every.as_ref().copied(),
            cache: arguments.cache,
            cache_keys: arguments.cache_keys.items().to_vec(),
            cache_strategy: arguments.cache_strategy.as_ref().cloned(),
            max_result_sets: arguments.max_result_sets.as_ref().copied(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_source_add",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::source_add(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result = json!({"applied":outcome.applied,"dry_run":dry_run,"mashup":outcome.plan.edit.mashup,"section":outcome.plan.section,"call":outcome.plan.call,"event":outcome.plan.event,"files":files(solution,&outcome.plan.edit)});
    add_notices(&mut result, &notices);
    Ok(result)
}
pub(crate) fn mashup_source_retarget_tool(
    solution: &Solution,
    arguments: MashupSourceRetargetRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupSourceRetargetRequest {
        request: crate::core::mashup::authoring::SourceRetargetRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            from: nonempty(&arguments.from, "from")?.into(),
            to: nonempty(&arguments.to, "to")?.into(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_source_retarget",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::mashup::source_retarget(solution, &request, &mut notices)
        .map_err(ToolError::coded)?;
    let mut result = json!({"mashup":arguments.mashup,"from":arguments.from,"to":arguments.to,"sections":outcome.plan.sections.iter().map(|(from_key,to_key)| json!({"from_key":from_key,"to_key":to_key})).collect::<Vec<_>>(),"bindings_rewritten":outcome.plan.bindings_rewritten,"events_rewritten":outcome.plan.events_rewritten,"notes":outcome.plan.notes,"files":files(solution,&outcome.plan.edit),"applied":outcome.applied,"dry_run":dry_run});
    add_notices(&mut result, &notices);
    Ok(result)
}
pub(crate) fn mashup_source_list_tool(
    solution: &Solution,
    arguments: MashupSourceListRequest,
) -> Result<Value, ToolError> {
    let sources = commands::mashup::source_list(solution, nonempty(&arguments.mashup, "mashup")?)
        .map_err(ToolError::coded)?;
    Ok(json!({"mashup": arguments.mashup, "sources": sources}))
}
pub(crate) fn mashup_source_remove_tool(
    solution: &Solution,
    arguments: MashupSourceRemoveRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupSourceRemoveRequest {
        request: crate::core::mashup::authoring::SourceRemoveRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            address: nonempty(&arguments.address, "address")?.into(),
            cascade: arguments.cascade,
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_source_remove",
    };
    let mut notices = commands::Notices::default();
    let outcome = commands::mashup::source_remove(solution, &request, &mut notices)
        .map_err(ToolError::coded)?;
    let mut result = json!({"applied":outcome.applied,"dry_run":dry_run,"mashup":arguments.mashup,"section":outcome.plan.section,"call":outcome.plan.call,"bindings_removed":outcome.plan.bindings_removed,"events_removed":outcome.plan.events_removed,"files":files(solution,&outcome.plan.edit)});
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_on_tool(
    solution: &Solution,
    arguments: MashupOnRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupOnRequest {
        request: crate::core::mashup::authoring::OnRequest {
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
    let relative = |path: &std::path::Path| solution.relative(path);
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

pub(crate) fn mashup_remove_tool(
    solution: &Solution,
    arguments: MashupRemoveRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupRemoveRequest {
        request: crate::core::mashup::authoring::RemoveRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            widget: nonempty(&arguments.widget, "widget")?.into(),
            cascade: arguments.cascade,
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_remove",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::remove(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let files = outcome
        .plan
        .edit
        .as_ref()
        .map_or_else(Vec::new, |edit| files(solution, edit));
    let mut result = json!({ "applied": outcome.applied, "dry_run": dry_run, "mashup": arguments.mashup, "widget": outcome.plan.widget, "removed": outcome.plan.removed, "bindings_removed": outcome.plan.bindings_removed, "events_removed": outcome.plan.events_removed, "orphaned_data_calls": outcome.plan.orphaned_data_calls, "files": files });
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_unbind_tool(
    solution: &Solution,
    arguments: MashupUnbindRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupUnbindRequest {
        request: crate::core::mashup::authoring::UnbindRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            from: nonempty(&arguments.from, "from")?.into(),
            to: nonempty(&arguments.to, "to")?.into(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_unbind",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::unbind(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result = json!({ "applied": outcome.applied, "dry_run": dry_run, "mashup": outcome.plan.edit.mashup, "from": outcome.plan.from, "to": outcome.plan.to, "files": files(solution, &outcome.plan.edit) });
    add_notices(&mut result, &notices);
    Ok(result)
}

pub(crate) fn mashup_off_tool(
    solution: &Solution,
    arguments: MashupOffRequest,
) -> Result<Value, ToolError> {
    let dry_run = arguments.dry_run;
    let request = commands::mashup::MashupOffRequest {
        request: crate::core::mashup::authoring::OffRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            event: nonempty(&arguments.event, "event")?.into(),
            invoke: nonempty(&arguments.invoke, "invoke")?.into(),
        },
        mode: if dry_run { Mode::Plan } else { Mode::Apply },
        lock_label: "mcp mashup_off",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::off(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result = json!({ "applied": outcome.applied, "dry_run": dry_run, "mashup": outcome.plan.edit.mashup, "event": outcome.plan.event, "invoke": outcome.plan.invoke, "files": files(solution, &outcome.plan.edit) });
    add_notices(&mut result, &notices);
    Ok(result)
}

fn files(solution: &Solution, edit: &crate::core::mashup::authoring::EditPlan) -> Vec<String> {
    let mut files = vec![solution.relative(&edit.entity_path)];
    if let Some(sidecars) = &edit.sidecars {
        files.push(solution.relative(&sidecars.content_path));
    }
    files
}

fn beside(
    anchor: String,
    side: crate::core::mashup::edit::Side,
) -> crate::core::mashup::edit::Placement {
    crate::core::mashup::edit::Placement::Beside { anchor, side }
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
        "unmapped": outcome
            .unmapped_arguments(&report)
            .map(|a| json!({ "caller": a.caller, "file": solution.relative(&a.file), "line": a.line, "parameter": a.parameter, "literal": a.literal, "to": a.to, "why": a.unmappable_why() }))
            .chain(outcome.unmapped_defaults(&report).map(|d| json!({ "entity": d.entity, "service": d.service, "parameter": d.parameter, "default": d.default, "to": d.to, "why": d.why })))
            .collect::<Vec<_>>(),
        "base": report.base,
        "notes": report.notes,
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
    if arguments.suggest_base {
        let export_side = adopt::Side::from_export(&export).map_err(ToolError::coded)?;
        let suggestions =
            adopt::base_suggestions(solution, &export_side).map_err(ToolError::coded)?;
        if suggestions.is_empty() {
            return Ok(json!({ "notes": [adopt::NO_BASE_NOTE] }));
        }
        return Ok(adopt::suggestions_json(&suggestions));
    }
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
        "type_changes": report.type_changes().iter().map(|(entity, d)| json!({ "entity": entity.path(), "path": d.path, "export": d.export, "repo": d.repo })).collect::<Vec<_>>(),
        "retyped_arguments": report.retyped_arguments.iter().map(|a| json!({ "service_entity": a.service_entity, "service": a.service, "parameter": a.parameter, "from": a.from, "to": a.to, "caller": a.caller, "file": solution.relative(&a.file), "line": a.line, "literal": a.literal, "mapped": a.mapped_source(), "why": a.unmappable_why() })).collect::<Vec<_>>(),
        "retyped_defaults": report.retyped_defaults.iter().map(|d| json!({ "entity": d.entity, "service": d.service, "parameter": d.parameter, "to": d.to, "default": d.default, "why": d.why })).collect::<Vec<_>>(),
        "identical": report.with_status(adopt::Status::Identical).count(),
        "base": report.base,
        "notes": report.notes,
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
    let dry_run = arguments.dry_run;
    let request = rename::Request {
        kind: arguments.kind.into(),
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

pub(crate) fn mashup_gap_tool(
    solution: &Solution,
    arguments: MashupGapRequest,
) -> Result<Value, ToolError> {
    let request = commands::mashup::MashupGapRequest {
        request: crate::core::mashup::authoring::GapRequest {
            mashup: nonempty(&arguments.mashup, "mashup")?.into(),
            container: nonempty(&arguments.container, "container")?.into(),
            px: arguments.px,
        },
        mode: if arguments.dry_run {
            Mode::Plan
        } else {
            Mode::Apply
        },
        lock_label: "mcp mashup_gap",
    };
    let mut notices = commands::Notices::default();
    let outcome =
        commands::mashup::gap(solution, &request, &mut notices).map_err(ToolError::coded)?;
    let mut result =
        crate::core::mashup::authoring::gap_plan_json(solution, &outcome.plan, outcome.applied);
    add_notices(&mut result, &notices);
    Ok(result)
}
