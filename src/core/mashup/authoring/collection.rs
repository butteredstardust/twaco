//! `mashup collection add`: a cell mashup repeated for every row of a service result.

use super::{
    base, bind_in, existing_data_call, finish_edit, load_existing, parse_loaded, plan_add,
    AuthoringContext, AuthoringError, EditPlan,
};
use crate::core::bindings;
use crate::core::config::Solution;
use crate::core::mashup::address::{parse_address, Address};
use crate::core::mashup::describe::mashup_parameters;
use crate::core::mashup::edit::Placement;
use crate::core::{mashup, types, widgets};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct CollectionAddRequest {
    pub mashup: String,
    pub cell: String,
    pub data: String,
    pub mappings: Vec<(String, String)>,
    pub uid: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Size cells so this many fit on one row of a collection `across` pixels wide.
    pub columns: Option<u32>,
    /// The collection's width that `columns` is worked out for; [`DEFAULT_ACROSS`] when absent.
    pub across: Option<u32>,
    pub preset: Option<CollectionPreset>,
    /// A StateDefinition in the solution whose states are the cell menu's items.
    pub menu: Option<String>,
    pub placement: Option<Placement>,
}

/// A collection presentation as the design kits configure it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionPreset {
    /// Framed cards spread evenly across each row, the last row aligned under the first columns,
    /// the first card selected: the kit's KPI card collection.
    Cards,
}

impl CollectionPreset {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "cards" => Some(Self::Cards),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Cards => "cards",
        }
    }

    /// The properties the kit's card collection sets away from the catalogue defaults; its
    /// distribution (flex, space-evenly, 40px gaps) is already the catalogue's.
    fn properties(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Cards => &[
                ("LeftAlignLastRow", "true"),
                ("ContentAlignment", "top"),
                ("MashupBackground", "transparent"),
                ("LeftPadding", "15"),
                ("RightPadding", "24"),
                ("TopPadding", "24"),
                ("BottomPadding", "24"),
                ("SelectionMode", "single"),
                ("AutoSelectFirstRow", "true"),
            ],
        }
    }

    /// The cell's frame: the collection's `cell` part style, in each state the kit styles.
    fn style(self) -> Value {
        match self {
            Self::Cards => {
                let frame = json!({"border-color": CARD_BORDER, "border-radius": "6px", "border-style": "solid"});
                let mut normal = frame.clone();
                normal["border-width"] = json!("1px");
                let mut pressed = frame.clone();
                pressed["background"] = json!("rgba(255,255,255,1)");
                // Keys in the order Composer writes them.
                json!({"parts": {"cell": {
                    "$": sorted(normal),
                    "hover": frame,
                    "hover+selected": frame,
                    "pressed": sorted(pressed),
                    "selected": frame,
                }}, "widget": "ptcs-collection"})
            }
        }
    }

    /// The kit card's height. It does not shrink with a narrower width: the card's content
    /// (a title row over a filling dial) needs its height whatever the width, and a shorter cell
    /// scrolls (seen live at 285x192). The cell menu's row (50px in 10.2) comes on top of it.
    fn height(self) -> u32 {
        match self {
            Self::Cards => 270,
        }
    }
}

const CARD_BORDER: &str = "rgba(192,192,192,1)";

/// A full-width collection in a 1440px browser window, less the page's margins.
pub const DEFAULT_ACROSS: u32 = 1400;

/// What the collection keeps free for its own vertical scrollbar when its cells overflow.
const SCROLLBAR: u32 = 17;

fn sorted(value: Value) -> Value {
    let mut map = Map::new();
    if let Value::Object(object) = value {
        let mut entries: Vec<_> = object.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        map.extend(entries);
    }
    Value::Object(map)
}

/// The cell width that fits `columns` cells on one row of a flex collection `across` pixels
/// wide. The 10.2 collection starts a new row when `n*CellWidth + (n-1)*ColumnGap` reaches its
/// width less the paddings and the scrollbar; a gap is also kept at each end, so space-evenly
/// still shows space around the row as the kit does. `None` when no positive width fits.
pub(crate) fn column_width(
    columns: u32,
    across: u32,
    left: u32,
    right: u32,
    gap: u32,
) -> Option<u32> {
    // u64: an absurd --columns or --across is refused, never an overflow.
    let room =
        u64::from(across).checked_sub(u64::from(left) + u64::from(right) + u64::from(SCROLLBAR))?;
    let free = room.checked_sub((u64::from(columns) + 1) * u64::from(gap) + 1)?;
    u32::try_from(free / u64::from(columns))
        .ok()
        .filter(|width| *width > 0)
}

#[derive(Debug)]
pub struct CollectionAddPlan {
    pub edit: EditPlan,
    pub id: String,
    pub cell: String,
    pub mappings: Vec<(String, String)>,
    pub uid: Option<String>,
    pub binding: String,
    /// The cell size written, when one was given or worked out.
    pub cell_width: Option<u32>,
    pub cell_height: Option<u32>,
    pub preset: Option<CollectionPreset>,
    /// The cell menu's StateDefinition and the `CellMenu:<item>` events it fires.
    pub menu: Option<(String, Vec<String>)>,
    pub notes: Vec<String>,
}

/// Plan a collection insertion and its whole-result binding as one edit.
pub(crate) fn plan_collection_add(
    solution: &Solution,
    request: &CollectionAddRequest,
) -> Result<CollectionAddPlan, AuthoringError> {
    if request.mashup == request.cell {
        return Err(AuthoringError::Refused(
            "a mashup cannot use itself as a collection cell".into(),
        ));
    }
    if request.mappings.is_empty() {
        return Err(AuthoringError::Invalid(
            "give at least one --map <row-field>=<cell-parameter>".into(),
        ));
    }
    if request.width == Some(0) || request.height == Some(0) {
        return Err(AuthoringError::Invalid(
            "--width and --height must be positive integers".into(),
        ));
    }
    if request.columns == Some(0) || request.across == Some(0) {
        return Err(AuthoringError::Invalid(
            "--columns and --across must be positive integers".into(),
        ));
    }
    if request.width.is_some() && request.columns.is_some() {
        return Err(AuthoringError::Invalid(
            "give --width or --columns, not both: --columns works the width out".into(),
        ));
    }
    if request.across.is_some() && request.columns.is_none() {
        return Err(AuthoringError::Invalid(
            "--across is the width --columns is worked out for; give --columns too".into(),
        ));
    }
    let mut fields = BTreeSet::new();
    let mut parameters = BTreeSet::new();
    for (field, parameter) in &request.mappings {
        if field.is_empty() || parameter.is_empty() {
            return Err(AuthoringError::Invalid(
                "--map must be row-field=cell-parameter".into(),
            ));
        }
        if !fields.insert(field) {
            return Err(AuthoringError::Refused(format!(
                "row field {field} is mapped twice"
            )));
        }
        if !parameters.insert(parameter) {
            return Err(AuthoringError::Refused(format!(
                "cell parameter {parameter} is mapped twice"
            )));
        }
    }
    let menu = match &request.menu {
        None => None,
        Some(menu) => {
            let discovered = crate::core::workspace::discover(solution);
            let events = mashup::cell_menu::repository_events(&discovered, menu).ok_or_else(|| {
                AuthoringError::Unknown(format!(
                    "--menu {menu}: no readable StateDefinition {menu} in the solution; create it with `twaco new state {menu} --type string --state ...`"
                ))
            })?;
            Some((menu.clone(), events))
        }
    };
    let (cell_width, cell_height) = cell_size(request)?;
    let cell = load_existing(solution, &request.cell)?;
    let cell_parameters =
        mashup_parameters(&cell.entity_before).map_err(AuthoringError::Unreadable)?;
    for (_, parameter) in &request.mappings {
        let found = cell_parameters
            .iter()
            .find(|found| found.name == *parameter)
            .ok_or_else(|| {
                AuthoringError::Unknown(format!("{} has no parameter {parameter}", request.cell))
            })?;
        if found.direction == "OUT" {
            return Err(AuthoringError::Refused(format!(
                "{parameter} is an output of {}; a collection supplies an input",
                request.cell
            )));
        }
    }
    let parent = load_existing(solution, &request.mashup)?;
    let mut document = parse_loaded(&parent)?;
    let address =
        parse_address(&request.data).map_err(|error| AuthoringError::Invalid(error.to_string()))?;
    let Address::Data {
        selected_rows: false,
        entity_name: false,
        member: None,
        ..
    } = address
    else {
        return Err(AuthoringError::Invalid(
            "--data names a whole service result".into(),
        ));
    };
    let (model, _) = types::load_model(solution);
    let platform = types::cached_platform(solution);
    let parent_parameters =
        mashup_parameters(&parent.entity_before).map_err(AuthoringError::Unreadable)?;
    let context = AuthoringContext {
        parameters: &parent_parameters,
        model: &model,
        platform: platform.as_ref(),
    };
    let source = super::resolve_end(
        &mut document,
        Some(solution),
        &context,
        address,
        true,
        &mut Vec::new(),
    )?;
    if base(&source) != "INFOTABLE" {
        return Err(AuthoringError::Refused(
            "--data must return an INFOTABLE".into(),
        ));
    }
    let (section, call) = existing_data_call(&document, &source)?.ok_or_else(|| {
        AuthoringError::Refused(format!(
            "{} is not a call in {}; add it with `twaco mashup source add` first",
            request.data, request.mashup
        ))
    })?;
    let resolver = bindings::Resolver::new(&model, platform.as_ref());
    let sections = bindings::data_sections(document.value());
    let shape_name =
        bindings::service_at(&sections, &section, &call).and_then(|(section, found)| {
            // `call` is the call's name in the mashup; an aliased call names a different service.
            resolver
                .service(section, &found.target)
                .service
                .and_then(|service| {
                    service
                        .result
                        .base_type
                        .eq_ignore_ascii_case("INFOTABLE")
                        .then(|| service.result.data_shape.clone())
                        .flatten()
                })
        });
    if let Some(shape) = shape_name
        .as_deref()
        .and_then(|name| resolver.checkable_shape(name))
    {
        for (field, parameter) in &request.mappings {
            let found = shape
                .fields
                .iter()
                .find(|known| known.name == *field)
                .ok_or_else(|| {
                    AuthoringError::Refused(format!(
                        "{} returns {}, which has no field {field}",
                        request.data, shape.name
                    ))
                })?;
            let expected = cell_parameters
                .iter()
                .find(|known| known.name == *parameter)
                .expect("checked above");
            if !super::bind::compatible(&found.base_type, &expected.base_type) {
                return Err(AuthoringError::Refused(format!(
                    "field {field} is {}, not {} for {}",
                    found.base_type, expected.base_type, parameter
                )));
            }
        }
        if let Some(uid) = &request.uid {
            if !shape.fields.iter().any(|known| known.name == *uid) {
                return Err(AuthoringError::Refused(format!(
                    "{} returns {}, which has no field {uid}",
                    request.data, shape.name
                )));
            }
        }
    }
    let mut property_binding = Map::new();
    for (field, parameter) in &request.mappings {
        property_binding.insert(field.clone(), Value::String(parameter.clone()));
    }
    let mut properties = vec![
        ("Mashup".into(), request.cell.clone()),
        (
            "MashupPropertyBinding".into(),
            Value::Object(property_binding).to_string(),
        ),
    ];
    if let Some(uid) = &request.uid {
        properties.push(("UIDField".into(), uid.clone()));
    }
    if let Some(preset) = request.preset {
        for (name, value) in preset.properties() {
            properties.push(((*name).into(), (*value).into()));
        }
    }
    if let Some(width) = cell_width {
        properties.push(("CellWidth".into(), width.to_string()));
    }
    if let Some(height) = cell_height {
        properties.push(("CellHeight".into(), height.to_string()));
    }
    if let Some((menu, _)) = &menu {
        properties.push(("CellMenu".into(), menu.clone()));
    }
    // `plan_add` owns catalogue defaults, id allocation and responsive placement. Re-open its
    // pure result so the binding joins the same final sync and transaction.
    let added = plan_add(
        solution,
        &super::AddRequest {
            mashup: request.mashup.clone(),
            widget: "ptcscollection".into(),
            inputs: Vec::new(),
            placement: request.placement.clone(),
            label: None,
            properties,
        },
    )?;
    let assets = mashup::extract(&added.edit.entity_after).map_err(AuthoringError::Mashup)?;
    document = mashup::edit::parse(&assets.content).map_err(AuthoringError::Edit)?;
    if let Some(preset) = request.preset {
        super::set::style_properties(document.root_properties(), &request.mashup)?;
        super::set::copy_style_entry(
            &mut document,
            &added.placed.id,
            preset.style(),
            None,
            &mut Vec::new(),
        );
    }
    let target = format!("ui:{}.Data", added.placed.id);
    bind_in(
        &mut document,
        Some(solution),
        &context,
        &request.data,
        &target,
    )?;
    let has_event = document.value()["Events"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|event| {
            event["EventHandlerArea"] == "Data"
                && event["EventHandlerId"] == section
                && event["EventHandlerService"] == call
        });
    let mut notes: Vec<String> = (!has_event)
        .then(|| {
            format!(
                "nothing runs {call}; add `twaco mashup on {} --event mashup:Loaded --invoke {}`",
                request.mashup, request.data
            )
        })
        .into_iter()
        .collect();
    if let Some((menu, events)) = &menu {
        if events.is_empty() {
            notes.push(format!(
                "{menu} has no state with a value, so the cell menu has no items"
            ));
        }
    }
    Ok(CollectionAddPlan {
        edit: finish_edit(parent, document, "mashup collection add")?,
        id: added.placed.id,
        cell: request.cell.clone(),
        mappings: request.mappings.clone(),
        uid: request.uid.clone(),
        binding: format!("{} -> {target}", request.data),
        cell_width,
        cell_height,
        preset: request.preset,
        menu,
        notes,
    })
}

/// The cell size to write: given, worked out from `columns`, or (for a preset with no height)
/// the preset's height. Paddings and gaps are the preset's or the catalogue's,
/// whichever the collection will have.
fn cell_size(request: &CollectionAddRequest) -> Result<(Option<u32>, Option<u32>), AuthoringError> {
    let defaults = widgets::catalog()
        .by_type("ptcscollection")
        .map(|widget| widget.defaults.clone())
        .unwrap_or_default();
    let number = |name: &str| -> u32 {
        request
            .preset
            .and_then(|preset| {
                preset
                    .properties()
                    .iter()
                    .find(|(key, _)| *key == name)
                    .and_then(|(_, value)| value.parse().ok())
            })
            .or_else(|| {
                defaults
                    .get(name)
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            })
            .unwrap_or(0)
    };
    let width = match request.columns {
        None => request.width,
        Some(columns) => {
            let across = request.across.unwrap_or(DEFAULT_ACROSS);
            let (left, right, gap) = (
                number("LeftPadding"),
                number("RightPadding"),
                number("ColumnGap"),
            );
            Some(column_width(columns, across, left, right, gap).ok_or_else(|| {
                AuthoringError::Refused(format!(
                    "{columns} columns do not fit a collection {across}px wide (paddings {left}+{right}px, gaps {gap}px); give a larger --across or fewer --columns"
                ))
            })?)
        }
    };
    let height = request
        .height
        .or_else(|| request.preset.map(CollectionPreset::height));
    Ok((width, height))
}

pub fn collection_add_plan_json(
    solution: &Solution,
    plan: &CollectionAddPlan,
    applied: bool,
) -> Value {
    let mut files = vec![solution.relative(&plan.edit.entity_path)];
    if let Some(sidecars) = &plan.edit.sidecars {
        files.push(solution.relative(&sidecars.content_path));
    }
    json!({"applied": applied, "mashup": plan.edit.mashup, "id": plan.id, "cell": plan.cell,
        "mappings": plan.mappings.iter().map(|(field, parameter)| json!({"field":field,"parameter":parameter})).collect::<Vec<_>>(),
        "uid": plan.uid, "binding": plan.binding,
        "cell_width": plan.cell_width, "cell_height": plan.cell_height,
        "preset": plan.preset.map(CollectionPreset::name),
        "menu": plan.menu.as_ref().map(|(name, events)| json!({"state_definition": name, "events": events})),
        "notes": plan.notes, "files": files})
}
