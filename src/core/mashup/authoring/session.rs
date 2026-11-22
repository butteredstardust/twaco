//! The same planners as commands, with an in-memory entity and sidecars.
use super::{AuthoringContext, AuthoringError, ExistingMashup};
use crate::core::{config::Solution, mashup, types};
use mashup::edit::MashupDocument;

/// Where a collection's cell mashup comes from: the solution, or a page compiled in memory that
/// the build writes beside the page.
pub(crate) enum CellSource {
    Existing(String),
    Generated {
        name: String,
        entity: Vec<u8>,
        content: String,
        css: String,
    },
}

pub(crate) struct Session {
    pub(crate) document: MashupDocument,
    pub(crate) entity: Vec<u8>,
    pub(crate) css: String,
    name: String,
}

impl Session {
    pub(crate) fn source_type(
        &mut self,
        solution: &Solution,
        model: &types::Model,
        platform: Option<&types::Platform>,
        address: &str,
    ) -> Result<String, AuthoringError> {
        let address = mashup::address::parse_address(address)
            .map_err(|e| AuthoringError::Invalid(e.to_string()))?;
        let context = AuthoringContext {
            parameters: &[],
            model,
            platform,
        };
        let end = super::resolve_end(
            &mut self.document,
            Some(solution),
            &context,
            address,
            true,
            &mut vec![],
        )?;
        Ok(super::base(&end).to_string())
    }
    pub(crate) fn new(
        name: &str,
        project: &str,
        template: &str,
        theme: &str,
    ) -> Result<Self, AuthoringError> {
        crate::core::entity::key::EntityKey::new("Mashups", name)
            .map_err(|e| AuthoringError::Invalid(e.to_string()))?;
        let rules = mashup::layout::RuleSet::embedded(mashup::layout::Version::V1);
        let template = mashup::templates::layouts_with(rules)
            .map_err(AuthoringError::Invalid)?
            .into_iter()
            .find(|t| t.name == template || t.aliases.iter().any(|a| a == template))
            .ok_or_else(|| AuthoringError::Unknown(format!("unknown template {template}")))?;
        super::new::validate_template(&template)?;
        if matches!(
            template.name.as_str(),
            "sidebar" | "header-sidebar" | "sidebar-header"
        ) {
            return Err(AuthoringError::Refused(
                "menu templates need explicit menu configuration".into(),
            ));
        }
        let mut document = super::new::new_document(&template, false, theme)?;
        if template.name == "blank" {
            let context = mashup::layout::Context {
                template: "blank",
                element: mashup::layout::Element::Page,
                slot: None,
                parent: None,
                widget: None,
                op: None,
                label: None,
                facts: &[],
            };
            if let Some(gutter) = rules.evaluate(&context).gutter {
                let center = document.find("center").map_err(AuthoringError::Edit)?;
                document.set_property(&center.path, "Margin", serde_json::json!(gutter.value));
            }
        }
        document.root_mut().remove("CustomMashupCss");
        document.set_top("twacoPageRules", serde_json::json!("v1"));
        document.set_uuid_generator(crate::core::mashup::page::ids::uuid);
        let entity = mashup::templates::entity_xml(
            name,
            project,
            mashup::templates::MashupKind::Mashup,
            &[],
        )
        .map_err(|e| AuthoringError::Invalid(e.to_string()))?
        .into_bytes();
        let mut session = Self {
            document,
            entity,
            css: String::new(),
            name: name.into(),
        };
        session.sync()?;
        Ok(session)
    }

    /// An input parameter, written as `mashup new --param` writes it.
    pub(crate) fn add_parameter(&mut self, text: &str) -> Result<(), AuthoringError> {
        let definition = super::parse_new_parameter(text)?;
        super::set_content_parameter(&mut self.document, &definition, false);
        self.sync()?;
        self.entity = super::splice_parameter_definition(&self.entity, &definition, None, false)?;
        Ok(())
    }

    /// Configure a collection widget already in the page: its cell mashup, field mapping,
    /// preset and data, through the code `mashup collection add` runs.
    pub(crate) fn collection(
        &mut self,
        solution: &Solution,
        request: &super::CollectionAddRequest,
        cell: &CellSource,
        widget: &str,
    ) -> Result<(), AuthoringError> {
        self.sync()?;
        let cell = match cell {
            CellSource::Existing(name) => super::load_existing(solution, name)?,
            CellSource::Generated {
                name,
                entity,
                content,
                css,
            } => ExistingMashup {
                mashup: name.clone(),
                entity_path: Default::default(),
                entity_before: entity.clone(),
                assets: mashup::Assets {
                    content: content.clone(),
                    css: css.clone(),
                },
                sidecars: None,
                deterministic: true,
            },
        };
        let plan = super::collection::plan_collection_configure_loaded(
            solution,
            request,
            self.loaded(),
            &cell,
            widget,
        )?;
        self.accept(plan.edit)
    }

    pub(crate) fn sync(&mut self) -> Result<(), AuthoringError> {
        let assets = mashup::Assets {
            content: self.document.to_content(),
            css: self.css.clone(),
        };
        self.entity = mashup::sync(&self.entity, &assets)
            .map_err(AuthoringError::Mashup)?
            .0;
        Ok(())
    }

    fn loaded(&self) -> ExistingMashup {
        ExistingMashup {
            mashup: self.name.clone(),
            entity_path: Default::default(),
            entity_before: self.entity.clone(),
            assets: mashup::Assets {
                content: self.document.to_content(),
                css: self.css.clone(),
            },
            sidecars: None,
            deterministic: true,
        }
    }

    fn accept(&mut self, plan: super::EditPlan) -> Result<(), AuthoringError> {
        self.entity = plan.entity_after;
        let assets = mashup::extract(&self.entity).map_err(AuthoringError::Mashup)?;
        self.document = mashup::edit::parse(&assets.content).map_err(AuthoringError::Edit)?;
        self.document
            .set_uuid_generator(crate::core::mashup::page::ids::uuid);
        self.css = assets.css;
        Ok(())
    }

    pub(crate) fn add(
        &mut self,
        solution: &Solution,
        request: &super::AddRequest,
    ) -> Result<String, AuthoringError> {
        self.sync()?;
        let plan = super::add::plan_add_loaded(solution, request, self.loaded())?;
        let id = plan.placed.id;
        self.accept(plan.edit)?;
        Ok(id)
    }

    pub(crate) fn template_children(
        &mut self,
        parent: &str,
        template: &str,
    ) -> Result<(), AuthoringError> {
        let layout = mashup::templates::layouts_with(self.document.rules())
            .map_err(AuthoringError::Invalid)?
            .into_iter()
            .find(|t| t.name == template)
            .ok_or_else(|| AuthoringError::Unknown(format!("unknown template {template}")))?;
        let path = self
            .document
            .find_exact(parent)
            .map_err(AuthoringError::Edit)?
            .path;
        self.document.at_mut(&path)["twacoTemplateName"] = serde_json::json!(template);
        for (key, value) in &layout.page {
            self.document.set_property(&path, key, value.clone());
        }
        for node in &layout.children {
            super::new::add_template_node(&mut self.document, &path, node, template)?;
        }
        Ok(())
    }

    pub(crate) fn contained(
        &mut self,
        solution: &Solution,
        parent: &str,
        mashup: &str,
    ) -> Result<String, AuthoringError> {
        self.sync()?;
        let request = super::AddContainedRequest {
            mashup: self.name.clone(),
            child: mashup.into(),
            placement: Some(mashup::edit::Placement::In(parent.into())),
            settings: vec![],
        };
        let plan = super::add::plan_add_contained_loaded(solution, &request, self.loaded())?;
        let id = plan.placed.id;
        self.accept(plan.edit)?;
        Ok(id)
    }

    pub(crate) fn set(
        &mut self,
        solution: &Solution,
        request: &super::SetRequest,
    ) -> Result<(), AuthoringError> {
        self.sync()?;
        let plan = super::set::plan_set_loaded(solution, request, self.loaded())?;
        if let Some(edit) = plan.edit {
            self.accept(edit)?;
        }
        Ok(())
    }

    pub(crate) fn source(
        &mut self,
        solution: &Solution,
        request: &super::SourceAddRequest,
    ) -> Result<(), AuthoringError> {
        self.sync()?;
        let plan = super::source::plan_source_add_loaded(solution, request, self.loaded())?;
        self.accept(plan.edit)
    }

    pub(crate) fn recipe(
        &mut self,
        solution: &Solution,
        recipe: super::Recipe,
    ) -> Result<(), AuthoringError> {
        self.sync()?;
        let request = super::RecipeRequest {
            mashup: self.name.clone(),
            recipe,
        };
        let plan = super::recipes::plan_recipe_loaded(solution, &request, self.loaded())?;
        self.accept(plan.edit)
    }

    pub(crate) fn bind(
        &mut self,
        solution: &Solution,
        model: &types::Model,
        platform: Option<&types::Platform>,
        from: &str,
        to: &str,
    ) -> Result<(), AuthoringError> {
        let parameters = mashup::describe::mashup_parameters(&self.entity)
            .map_err(AuthoringError::Unreadable)?;
        let context = AuthoringContext {
            parameters: &parameters,
            model,
            platform,
        };
        super::bind_in(&mut self.document, Some(solution), &context, from, to)?;
        Ok(())
    }

    pub(crate) fn on(
        &mut self,
        model: &types::Model,
        platform: Option<&types::Platform>,
        from: &str,
        to: &str,
    ) -> Result<(), AuthoringError> {
        let context = AuthoringContext {
            parameters: &[],
            model,
            platform,
        };
        super::on_in(&mut self.document, &context, from, to)?;
        Ok(())
    }
}

pub(crate) use super::new::validate_theme;

pub(crate) fn source_target(
    model: &types::Model,
    platform: Option<&types::Platform>,
    raw: &str,
) -> Result<(String, String), AuthoringError> {
    let (entity, service) = super::resolve_service(model, platform, raw)?;
    if raw != format!("{}.{service}", entity.name) {
        return Err(AuthoringError::Invalid(format!(
            "{raw} must name an entity and service, without a result field"
        )));
    }
    Ok((entity.name.clone(), service))
}
