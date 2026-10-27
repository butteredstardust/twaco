//! How each caller uses one service, where `impact --member` says only who calls it.
//!
//! A view over what twaco already reads: the index says which mashups bind the service and which
//! scripts name it through `Things[...]`; `mashup describe` says, for each mashup, what runs the
//! call, what sets each parameter and where each result field goes; the script parser finds each
//! call site, the arguments it passes and the result fields it reads, including `me.<Service>()`
//! calls from the entity and from what inherits it. Read-only and offline.

use super::config::Solution;
use super::entity::key::EntityKey;
use super::index::{EdgeKind, Index};
use super::mashup;
use super::scan;
use super::workspace::WorkspaceError;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use swc_common::{BytePos, Spanned};
use swc_ecma_ast::{
    AssignTarget, CallExpr, Callee, Expr, Lit, MemberExpr, MemberProp, Pat, SimpleAssignTarget,
    VarDeclarator,
};
use swc_ecma_parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_ecma_visit::{Visit, VisitWith};

#[derive(Debug, Serialize)]
pub struct Trace {
    /// `Collection/Name.Service`.
    pub(crate) service: String,
    /// Where the definition was read, when it is inherited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) defined_in: Option<String>,
    pub(crate) parameters: Vec<Parameter>,
    pub(crate) returns: String,
    pub(crate) mashups: Vec<MashupCaller>,
    pub(crate) scripts: Vec<ScriptCall>,
    /// What the service itself calls: `Collection/Name.Member`.
    pub(crate) calls: Vec<String>,
    pub(crate) notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Parameter {
    pub(crate) name: String,
    pub(crate) base_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) default: Option<String>,
    pub(crate) required: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct MashupCaller {
    pub(crate) mashup: String,
    /// The data source as the mashup names it, `Entity.Service` or with an alias.
    pub(crate) call: String,
    pub(crate) runs_on: Vec<String>,
    /// Each parameter and what sets it; `UNBOUND` when nothing does.
    pub(crate) parameters: Vec<(String, Vec<String>)>,
    /// Each result path and where it goes: `[SelectedRows].UID -> ui:...`.
    pub(crate) results: Vec<(String, String)>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ScriptCall {
    /// `Collection/Name.Service` of the calling script.
    pub(crate) caller: String,
    pub(crate) line: usize,
    /// The call as written, on one line.
    pub(crate) call: String,
    /// The variable the result is put in, when it is put in one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) into: Option<String>,
    /// What the script reads of the result: fields, `rows`, `length`...
    pub(crate) reads: Vec<String>,
    /// False when the variable is bound more than once in the script (a parameter, another
    /// declaration, a reassignment): then what is read of it may not be this result.
    pub(crate) reads_followed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    #[error("{0}")]
    Entity(WorkspaceError),
    #[error("give the service as <Entity>.<Service>, such as Acme.Orders.Manager.GetOrder")]
    Shape,
    #[error("{entity} has no service {service}, of its own or inherited from the repository")]
    NoService { entity: String, service: String },
}

impl super::codes::Coded for TraceError {
    fn code(&self) -> super::codes::ErrorCode {
        match self {
            Self::Entity(error) => error.code(),
            Self::Shape => super::codes::ErrorCode::InvalidArguments,
            Self::NoService { .. } => super::codes::ErrorCode::UnknownEntity,
        }
    }
}

/// Trace `<Entity>.<Service>`; `param` narrows to one parameter's sources.
pub fn run(solution: &Solution, given: &str, param: Option<&str>) -> Result<Trace, TraceError> {
    let (entity, service) = given.rsplit_once('.').ok_or(TraceError::Shape)?;
    if entity.is_empty() || service.is_empty() {
        return Err(TraceError::Shape);
    }
    let index = Index::build(solution);
    let key = index.resolve(entity).map_err(TraceError::Entity)?;
    let found = index.files();
    let mut platform = None;
    let file_of = |key: &EntityKey| {
        found
            .iter()
            .find(|file| file.info.collection == key.collection() && file.info.name == key.name())
    };
    // The definition: the entity's own, else the nearest template or shape that has it.
    let mut owners = vec![key.clone()];
    for name in index.inheritance_names(&key) {
        for collection in ["ThingTemplates", "ThingShapes"] {
            if let Ok(ancestor) = EntityKey::new(collection, &name) {
                if index.contains(&ancestor) {
                    owners.push(ancestor);
                }
            }
        }
    }
    let mut definition = None;
    for owner in &owners {
        if let Some(file) = file_of(owner) {
            let src = std::fs::read(&file.path).unwrap_or_default();
            if let Some(found) = read_definition(&src, service) {
                definition = Some((owner.clone(), found));
                break;
            }
        }
    }
    let (owner, (parameters, returns)) = definition.ok_or_else(|| TraceError::NoService {
        entity: key.to_string(),
        service: service.to_string(),
    })?;

    let defaults: BTreeMap<String, String> = parameters
        .iter()
        .filter_map(|p| Some((p.name.clone(), p.default.clone()?)))
        .collect();

    // Who has the service: the owner and everything that inherits it.
    let mut holders: BTreeSet<EntityKey> = BTreeSet::from([owner.clone(), key.clone()]);
    for (other, _) in index.entities() {
        if index
            .inheritance_names(other)
            .iter()
            .any(|name| name == owner.name())
        {
            holders.insert(other.clone());
        }
    }
    let labels: BTreeSet<String> = holders.iter().map(EntityKey::to_string).collect();
    let edges = index.edges();

    // Mashup callers, through describe.
    let mut mashups = Vec::new();
    let mut seen = BTreeSet::new();
    for (from, to, edge) in &edges {
        if edge.kind != EdgeKind::MashupBinding
            || edge.to_member.as_deref() != Some(service)
            || !labels.contains(to)
            || !seen.insert(from.clone())
        {
            continue;
        }
        let Some(name) = from.strip_prefix("Mashups/") else {
            continue;
        };
        // Read once, and only when a mashup calls the service.
        let platform = platform.get_or_insert_with(|| super::types::cached_platform(solution));
        let Ok(description) =
            mashup::describe::describe_in(solution, name, found, index.model(), platform.as_ref())
        else {
            continue;
        };
        let names: BTreeSet<String> = holders
            .iter()
            .map(|holder| format!("{}.{service}", holder.name()))
            .collect();
        for used in &description.services {
            let called = used.service.split("[as ").next().unwrap_or_default();
            if !names.contains(called) {
                continue;
            }
            // A binding names the call as describe does: `data:E.S` or `data:E.S[as Alias]`.
            let source = format!("data:{}", used.service);
            let results = description
                .bindings
                .iter()
                .filter_map(|binding| {
                    let rest = binding.from.strip_prefix(&source)?;
                    let rest = rest.strip_prefix('.').unwrap_or(rest);
                    // `data:X.S` alone is the whole result; anything else must follow a separator.
                    if !(rest.is_empty()
                        || binding.from.as_bytes().get(source.len()) == Some(&b'.')
                        || binding.from.as_bytes().get(source.len()) == Some(&b'['))
                    {
                        return None;
                    }
                    let path = if rest.is_empty() { "(all data)" } else { rest };
                    Some((path.to_string(), binding.to.clone()))
                })
                .collect();
            let parameters = used
                .parameters
                .iter()
                .filter(|used| param.is_none_or(|wanted| used.name == wanted))
                .map(|used| {
                    let mut sources = used.set_by.clone();
                    if sources.is_empty() {
                        sources.push("unbound".to_string());
                    }
                    // Unbound, the service sees its default, if it has one.
                    if sources == ["unbound"] {
                        if let Some(default) = defaults.get(&used.name) {
                            sources = vec![format!("unbound (default {default})")];
                        }
                    }
                    (used.name.clone(), sources)
                })
                .collect();
            mashups.push(MashupCaller {
                mashup: from.clone(),
                call: used.service.clone(),
                runs_on: used.invoked_by.clone(),
                parameters,
                results,
            });
        }
    }

    // Script callers: `Things|ThingTemplates|ThingShapes["X"].Service()` from anywhere, and
    // `me.Service()` from the holders. Read from the scripts themselves: the index records only
    // `Things[...]` receivers.
    // Collection and name both: a template and a Thing may share a name.
    let targets: BTreeSet<(String, String)> = holders
        .iter()
        .map(|h| (h.collection().to_string(), h.name().to_string()))
        .collect();
    let needle = format!(".{service}");
    let mut scripts = Vec::new();
    let mut parse_failures = Vec::new();
    for (entity, file) in found.iter().filter_map(|file| {
        let key = EntityKey::new(&file.info.collection, &file.info.name).ok()?;
        Some((key, file))
    }) {
        let src = std::fs::read(&file.path).unwrap_or_default();
        if !String::from_utf8_lossy(&src).contains(&needle) {
            continue;
        }
        let label = entity.to_string();
        let own = holders.contains(&entity);
        let Ok(services) = super::sidecar::script_services(&src) else {
            continue;
        };
        for script in services
            .iter()
            .filter(|script| script.script.contains(&needle))
        {
            match call_sites(&script.script, service, &targets, own) {
                Some(sites) => {
                    for site in sites {
                        scripts.push(ScriptCall {
                            caller: format!("{label}.{}", script.name),
                            line: site.line,
                            call: site.call,
                            into: site.into,
                            reads: site.reads,
                            reads_followed: site.reads_followed,
                        });
                    }
                }
                None => parse_failures.push(format!("{label}.{}", script.name)),
            }
        }
    }

    // Forward: what the service's own script calls.
    let owner_label = owner.to_string();
    let mut calls: BTreeSet<String> = BTreeSet::new();
    for (from, to, edge) in &edges {
        if from == &owner_label
            && edge.kind == EdgeKind::ScriptReference
            && edge.from_member.as_deref() == Some(service)
        {
            match &edge.to_member {
                Some(member) => calls.insert(format!("{to}.{member}")),
                None => calls.insert(to.clone()),
            };
        }
    }
    if let Some(file) = file_of(&owner) {
        let src = std::fs::read(&file.path).unwrap_or_default();
        if let Ok(services) = super::sidecar::script_services(&src) {
            if let Some(own) = services.iter().find(|script| script.name == service) {
                for (receiver, member) in calls_in(&own.script) {
                    let receiver = if receiver == "me" {
                        owner_label.clone()
                    } else {
                        receiver
                    };
                    calls.insert(format!("{receiver}.{member}"));
                }
            }
        }
    }

    let mut notes = Vec::new();
    if !parse_failures.is_empty() {
        notes.push(format!(
            "could not parse {}: calls in them are not listed",
            parse_failures.join(", ")
        ));
    }
    if !index.is_complete() {
        notes.push("the index could not read every file; callers in those are missing".into());
    }
    notes.push(super::impact::LIMITS.to_string());
    Ok(Trace {
        service: format!("{key}.{service}"),
        defined_in: (owner != key).then(|| owner.to_string()),
        parameters: parameters
            .into_iter()
            .filter(|p| param.is_none_or(|wanted| p.name == wanted))
            .collect(),
        returns,
        mashups,
        scripts,
        calls: calls.into_iter().collect(),
        notes,
    })
}

/// The parameters and result of the service named `service` in an entity file.
fn read_definition(src: &[u8], service: &str) -> Option<(Vec<Parameter>, String)> {
    let tokens = scan::tokenize(src).ok()?;
    let attr = |tag: usize, name: &str| {
        scan::attribute(src, &tokens[tag], name)
            .ok()
            .flatten()
            .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
    };
    let definition = (0..tokens.len()).find(|&at| {
        matches!(tokens[at].kind, scan::Kind::Start | scan::Kind::Empty)
            && tokens[at].name.of(src) == b"ServiceDefinition"
            && attr(at, "name").as_deref() == Some(service)
    })?;
    let mut parameters = Vec::new();
    for list in scan::child_tags(&tokens, src, "ParameterDefinitions", definition) {
        for field in scan::child_tags(&tokens, src, "FieldDefinition", list) {
            parameters.push(Parameter {
                name: attr(field, "name").unwrap_or_default(),
                base_type: attr(field, "baseType").unwrap_or_default(),
                default: attr(field, "aspect.defaultValue"),
                required: attr(field, "aspect.isRequired").as_deref() == Some("true"),
            });
        }
    }
    let returns = scan::child_tags(&tokens, src, "ResultType", definition)
        .first()
        .map(|&result| {
            let base = attr(result, "baseType").unwrap_or_default();
            match attr(result, "aspect.dataShape").filter(|shape| !shape.is_empty()) {
                Some(shape) => format!("{base} {shape}"),
                None => base,
            }
        })
        .unwrap_or_default();
    Some((parameters, returns))
}

struct Site {
    line: usize,
    call: String,
    into: Option<String>,
    reads: Vec<String>,
    reads_followed: bool,
}

/// The calls of `service` in one script: on `me` (when `own`), or on `Things["T"]` / `Things.T`
/// for a `T` in `targets`. None when the script does not parse.
fn call_sites(
    script: &str,
    service: &str,
    targets: &BTreeSet<(String, String)>,
    own: bool,
) -> Option<Vec<Site>> {
    let ast = parse(script)?;
    let mut finder = Finder {
        src: script,
        service,
        targets,
        own,
        sites: Vec::new(),
        reads: BTreeMap::new(),
        bindings: BTreeMap::new(),
    };
    ast.visit_with(&mut finder);
    let reads = finder.reads;
    let bindings = finder.bindings;
    Some(
        finder
            .sites
            .into_iter()
            .map(|mut site| {
                if let Some(variable) = &site.into {
                    if bindings.get(variable).copied().unwrap_or(0) > 1 {
                        site.reads_followed = false;
                        return site;
                    }
                    let all = reads.get(variable).cloned().unwrap_or_default();
                    // `getRow()` says nothing `getRow().Count` does not.
                    site.reads = all
                        .iter()
                        .filter(|read| {
                            let deeper = format!("{read}.");
                            !all.iter().any(|other| other.starts_with(&deeper))
                        })
                        .cloned()
                        .collect();
                }
                site
            })
            .collect(),
    )
}

/// The calls in a script on `me`, or on `Things|ThingTemplates|ThingShapes["X"]` / `.X`:
/// `("me" or "Collection/X", member)`.
fn calls_in(script: &str) -> BTreeSet<(String, String)> {
    let Some(ast) = parse(script) else {
        return BTreeSet::new();
    };
    struct Calls(BTreeSet<(String, String)>);
    impl Visit for Calls {
        fn visit_call_expr(&mut self, call: &CallExpr) {
            if let Callee::Expr(callee) = &call.callee {
                if let Expr::Member(member) = &**callee {
                    if let MemberProp::Ident(name) = &member.prop {
                        let receiver = match &*member.obj {
                            Expr::Ident(object) if object.sym.as_ref() == "me" => {
                                Some("me".to_string())
                            }
                            Expr::Member(things) => collection_target(things)
                                .map(|(collection, entity)| format!("{collection}/{entity}")),
                            _ => None,
                        };
                        if let Some(receiver) = receiver {
                            self.0.insert((receiver, name.sym.to_string()));
                        }
                    }
                }
            }
            call.visit_children_with(self);
        }
    }
    let mut calls = Calls(BTreeSet::new());
    ast.visit_with(&mut calls);
    calls.0
}

/// `Things["X"]`, `ThingTemplates.X`...: the collection and the entity name.
fn collection_target(member: &MemberExpr) -> Option<(String, String)> {
    let Expr::Ident(root) = &*member.obj else {
        return None;
    };
    let collection = root.sym.to_string();
    if !matches!(
        collection.as_str(),
        "Things" | "ThingTemplates" | "ThingShapes"
    ) {
        return None;
    }
    let name = match &member.prop {
        MemberProp::Ident(ident) => ident.sym.to_string(),
        MemberProp::Computed(computed) => match &*computed.expr {
            Expr::Lit(Lit::Str(string)) => string.value.to_atom_lossy().to_string(),
            _ => return None,
        },
        MemberProp::PrivateName(_) => return None,
    };
    Some((collection, name))
}

fn parse(script: &str) -> Option<swc_ecma_ast::Script> {
    let end = u32::try_from(script.len()).ok()?;
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax {
            allow_return_outside_function: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::new(script, BytePos(0), BytePos(end)),
        None,
    );
    Parser::new_from(lexer).parse_script().ok()
}

struct Finder<'a> {
    src: &'a str,
    service: &'a str,
    targets: &'a BTreeSet<(String, String)>,
    own: bool,
    sites: Vec<Site>,
    /// What is read off each variable: `v.X`, `v.rows[i].X`, `v.getRow(i).X`.
    reads: BTreeMap<String, BTreeSet<String>>,
    /// How often each name is bound: declared with a value, assigned, or a parameter.
    bindings: BTreeMap<String, usize>,
}

impl Finder<'_> {
    fn bind(&mut self, name: &str) {
        *self.bindings.entry(name.to_string()).or_default() += 1;
    }
}

impl Finder<'_> {
    /// Whether `expr` is a call of the service on a target; its span when it is.
    fn is_call(&self, expr: &Expr) -> bool {
        let Expr::Call(CallExpr {
            callee: Callee::Expr(callee),
            ..
        }) = expr
        else {
            return false;
        };
        let Expr::Member(member) = &**callee else {
            return false;
        };
        let MemberProp::Ident(name) = &member.prop else {
            return false;
        };
        if name.sym.as_ref() != self.service {
            return false;
        }
        match &*member.obj {
            Expr::Ident(ident) => self.own && ident.sym.as_ref() == "me",
            Expr::Member(things) => self.things_target(things),
            _ => false,
        }
    }

    /// `Things["T"]`, `ThingTemplates.T`... with `T` a target.
    fn things_target(&self, member: &MemberExpr) -> bool {
        collection_target(member).is_some_and(|target| self.targets.contains(&target))
    }

    fn record(&mut self, expr: &Expr, into: Option<String>) {
        let span = expr.span();
        let text = self
            .src
            .get(span.lo.0 as usize..span.hi.0 as usize)
            .unwrap_or_default();
        let mut call: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if call.chars().count() > 120 {
            call = format!("{}...", call.chars().take(117).collect::<String>());
        }
        let line = self.src[..(span.lo.0 as usize).min(self.src.len())]
            .matches('\n')
            .count()
            + 1;
        self.sites.push(Site {
            line,
            call,
            into,
            reads: Vec::new(),
            reads_followed: true,
        });
    }
}

impl Visit for Finder<'_> {
    fn visit_param(&mut self, param: &swc_ecma_ast::Param) {
        if let Pat::Ident(name) = &param.pat {
            self.bind(&name.id.sym);
        }
        param.visit_children_with(self);
    }

    fn visit_arrow_expr(&mut self, arrow: &swc_ecma_ast::ArrowExpr) {
        for param in &arrow.params {
            if let Pat::Ident(name) = param {
                self.bind(&name.id.sym);
            }
        }
        arrow.visit_children_with(self);
    }

    fn visit_catch_clause(&mut self, clause: &swc_ecma_ast::CatchClause) {
        if let Some(Pat::Ident(name)) = &clause.param {
            self.bind(&name.id.sym);
        }
        clause.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(name), Some(_)) = (&declarator.name, &declarator.init) {
            self.bind(&name.id.sym);
        }
        if let (Pat::Ident(name), Some(init)) = (&declarator.name, &declarator.init) {
            if self.is_call(init) {
                self.record(init, Some(name.id.sym.to_string()));
                if let Expr::Call(call) = &**init {
                    for arg in &call.args {
                        arg.visit_with(self);
                    }
                }
                return;
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &swc_ecma_ast::AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left {
            self.bind(&left.id.sym);
            if self.is_call(&assign.right) {
                self.record(&assign.right, Some(left.id.sym.to_string()));
                if let Expr::Call(call) = &*assign.right {
                    for arg in &call.args {
                        arg.visit_with(self);
                    }
                }
                return;
            }
        }
        assign.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        let expr = Expr::Call(call.clone());
        if self.is_call(&expr) {
            self.record(&expr, None);
        }
        // `v.method(...)` reads the result through a method: say `method()`, not a field.
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(member) = &**callee {
                if let (Expr::Ident(variable), MemberProp::Ident(method)) =
                    (&*member.obj, &member.prop)
                {
                    self.reads
                        .entry(variable.sym.to_string())
                        .or_default()
                        .insert(format!("{}()", method.sym));
                    for arg in &call.args {
                        arg.visit_with(self);
                    }
                    return;
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        // `v.X`, `v.rows[i].X`, `v.getRow(i).X`: what is read off a variable.
        if let MemberProp::Ident(name) = &member.prop {
            let field = name.sym.to_string();
            match &*member.obj {
                Expr::Ident(variable) => {
                    self.reads
                        .entry(variable.sym.to_string())
                        .or_default()
                        .insert(field);
                }
                Expr::Member(inner) => {
                    if let (MemberProp::Computed(_), Expr::Member(rows)) =
                        (&inner.prop, &*inner.obj)
                    {
                        if let (Expr::Ident(variable), MemberProp::Ident(rows_name)) =
                            (&*rows.obj, &rows.prop)
                        {
                            if rows_name.sym.as_ref() == "rows" {
                                self.reads
                                    .entry(variable.sym.to_string())
                                    .or_default()
                                    .insert(format!("rows[].{field}"));
                            }
                        }
                    }
                }
                Expr::Call(call) => {
                    if let Callee::Expr(callee) = &call.callee {
                        if let Expr::Member(get_row) = &**callee {
                            if let (Expr::Ident(variable), MemberProp::Ident(method)) =
                                (&*get_row.obj, &get_row.prop)
                            {
                                if method.sym.as_ref() == "getRow" {
                                    self.reads
                                        .entry(variable.sym.to_string())
                                        .or_default()
                                        .insert(format!("getRow().{field}"));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        member.visit_children_with(self);
    }
}

/// The trace as people read it.
pub fn render(trace: &Trace) -> String {
    let mut out = format!("trace {}\n", trace.service);
    if let Some(owner) = &trace.defined_in {
        out.push_str(&format!("  defined in {owner}\n"));
    }
    let takes: Vec<String> = trace
        .parameters
        .iter()
        .map(|p| {
            let mut text = format!("{} {}", p.name, p.base_type);
            if let Some(default) = &p.default {
                text.push_str(&format!(" (default {default})"));
            }
            if p.required {
                text.push_str(" (required)");
            }
            text
        })
        .collect();
    out.push_str(&format!(
        "  takes   {}\n",
        if takes.is_empty() {
            "nothing".to_string()
        } else {
            takes.join(", ")
        }
    ));
    out.push_str(&format!("  returns {}\n", trace.returns));
    out.push_str(&format!("mashup callers ({})\n", trace.mashups.len()));
    for caller in &trace.mashups {
        out.push_str(&format!("  {}  as {}\n", caller.mashup, caller.call));
        out.push_str(&format!("    runs on  {}\n", caller.runs_on.join(", ")));
        for (name, sources) in &caller.parameters {
            out.push_str(&format!("    {name} <- {}\n", sources.join(", ")));
        }
        for (path, to) in &caller.results {
            out.push_str(&format!("    {path} -> {to}\n"));
        }
    }
    out.push_str(&format!("script callers ({})\n", trace.scripts.len()));
    for call in &trace.scripts {
        let into = call
            .into
            .as_deref()
            .map(|v| format!("{v} = "))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {}:{}  {into}{}\n",
            call.caller, call.line, call.call
        ));
        if !call.reads_followed {
            out.push_str("    reads not followed: the variable is bound more than once\n");
        } else if !call.reads.is_empty() {
            out.push_str(&format!("    reads {}\n", call.reads.join(", ")));
        }
    }
    out.push_str(&format!("calls ({})\n", trace.calls.len()));
    for call in &trace.calls {
        out.push_str(&format!("  {call}\n"));
    }
    for note in &trace.notes {
        out.push_str(&format!("note: {note}\n"));
    }
    out
}

#[cfg(test)]
mod tests;
