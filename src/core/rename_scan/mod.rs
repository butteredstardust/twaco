//! Byte-preserving entity-name discovery and edits for one XML document.
//!
//! The pass consumes bytes and returns spans; it performs no file I/O and never serialises XML.
//! Comments and processing instructions are deliberately ignored, while raw attribute values,
//! non-whitespace text nodes and CDATA payloads are inspected without decoding XML entities.

mod config;
mod field;
mod findings;
mod mashup;
mod param;
mod service;
mod table;
mod xml;

pub use config::{scan_param_config, scan_service_config, ParamConfigPass};
pub(crate) use field::child_elements;
pub use field::{
    property_shapes, scan_configuration_field, scan_data_shape_field, scan_infotable_field,
    FieldPass,
};
pub(crate) use findings::{
    add_field_edit, add_review, add_service_edit, lexical_mentions, span_text,
};
pub use findings::{replace_file, review_mentions, Finding, Place, XmlPass};
pub use mashup::{scan_param_mashup, scan_service_mashup};
pub use param::{
    scan_param_definition, scan_param_entity, scan_param_script, script_uses_identifier, ParamPass,
};
pub use service::{
    has_local_service, scan_service_definition, scan_service_entity, scan_service_script,
};
pub use table::{scan_configuration_table, scan_table_script, TablePass};
pub use xml::{scan_text, scan_xml};

#[cfg(test)]
mod tests;
