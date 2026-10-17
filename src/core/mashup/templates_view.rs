//! Read-only summaries of the embedded mashup layout templates.

use crate::core::mashup;
use crate::core::mashup::authoring::template_slots;
use crate::core::mashup::templates::LayoutNode;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct TemplateSummary {
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: String,
    pub slots: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TemplateDetail {
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: String,
    pub slots: Vec<String>,
    pub page: serde_json::Map<String, serde_json::Value>,
    pub master_slot: Option<String>,
    pub children: Vec<LayoutNode>,
}

pub fn list() -> Result<Vec<TemplateSummary>, String> {
    mashup::templates::layouts().map(|templates| {
        templates
            .into_iter()
            .map(|template| TemplateSummary {
                slots: template_slots(&template),
                name: template.name,
                aliases: template.aliases,
                summary: template.summary,
            })
            .collect()
    })
}

pub fn detail(name: &str) -> Result<Option<TemplateDetail>, String> {
    mashup::templates::layout(name).map(|template| {
        template.map(|template| TemplateDetail {
            slots: template_slots(&template),
            name: template.name,
            aliases: template.aliases,
            summary: template.summary,
            page: template.page,
            master_slot: template.master_slot,
            children: template.children,
        })
    })
}

/// One line per container, indented by depth: its slot, direction and sizing.
pub fn tree_lines(detail: &TemplateDetail) -> Vec<String> {
    fn describe(properties: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(direction) = properties.get("flex-direction").and_then(|v| v.as_str()) {
            out.push(direction.to_string());
        }
        if properties.get("flex-grow").and_then(|v| v.as_i64()) == Some(1) {
            out.push("fills".into());
        }
        if let Some(justify) = properties.get("justify-content").and_then(|v| v.as_str()) {
            out.push(justify.to_string());
        }
        out
    }
    fn walk(nodes: &[LayoutNode], depth: usize, lines: &mut Vec<String>) {
        for node in nodes {
            let name = if node.divider {
                "divider".to_string()
            } else {
                node.slot.clone().unwrap_or_default()
            };
            let mut bits = describe(&node.properties);
            if let Some(size) = &node.fixed {
                bits.push(format!("fixed {size}"));
            }
            if node.divider {
                bits.push("fixed 1px".into());
            }
            if let Some(widget) = &node.widget {
                bits.push(format!("holds {widget}"));
            }
            lines.push(format!(
                "{}{name:<10} {}",
                "  ".repeat(depth + 1),
                bits.join(", ")
            ));
            walk(&node.children, depth + 1, lines);
        }
    }
    let mut lines = vec![format!("page       {}", describe(&detail.page).join(", "))];
    walk(&detail.children, 0, &mut lines);
    lines
}
