//! References from `twaco.toml`.

use super::build::Builder;
use super::EdgeKind;
use crate::core::entity_key::EntityKey;

impl Builder<'_> {
    /// An edge from each project to the entities its deploy configuration names: the entry point
    /// Thing and its deploy service, and every post-import call. These are structural: the
    /// configuration says so, and a deploy runs them.
    pub(super) fn add_deploy_edges(&mut self) {
        let solution = self.solution;
        for project in &solution.projects {
            let Some(&from) = self.projects.get(&project.name) else {
                continue;
            };
            let deploy = &project.deploy;
            if let Some(thing) = &deploy.entry_point_thing {
                self.link(
                    from,
                    "Things",
                    thing,
                    EdgeKind::Deploy,
                    None,
                    deploy.deploy_service.as_deref(),
                );
            }
            for call in &deploy.post_import {
                let (collection, name) = match call.target.as_deref().map(EntityKey::parse) {
                    Some(Ok(key)) => (key.collection().to_string(), key.name().to_string()),
                    _ => ("Things".to_string(), call.thing.clone()),
                };
                self.link(
                    from,
                    &collection,
                    &name,
                    EdgeKind::Deploy,
                    None,
                    Some(&call.service),
                );
            }
        }
    }
}
