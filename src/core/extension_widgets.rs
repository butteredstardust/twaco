//! The widgets a solution's own `[[extension]]` projects define, read from their mashup-builder
//! source, so `mashup add` can place them as Composer would.
//!
//! Read a JavaScript `TW.IDE.Widgets.<type> = function () { this.widgetProperties
//! = function () { return {...} } }`; a web-component widget's configuration, registered with
//! `TW.Widget.widgetWrapper.config(<type>, {flags, properties, events})`; and a TypeScript
//! `@TWWidgetDefinition` class, including its compiled `__decorate` calls. Only literals count: a
//! default worked out at run time is left out, and Composer fills it when the mashup is next
//! opened. A property without a default is not written, as Composer does not write it.
//!
//! Widgets of extensions installed on the server come from the platform cache, which
//! `types --platform` fills with the mashup builder's combined extension code.

use super::config::Solution;
use super::widgets::{PropertyContract, Widget};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use swc_common::BytePos;
use swc_ecma_ast::{
    ArrowExpr, AssignExpr, AssignTarget, BlockStmtOrExpr, CallExpr, Callee, ClassDecl, ClassMember,
    Decl, Decorator, Expr, Function, Lit, MemberExpr, MemberProp, ObjectLit, Pat, Prop, PropName,
    PropOrSpread, SimpleAssignTarget, Stmt, UnaryOp, VarDeclarator,
};
use swc_ecma_parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax, TsSyntax};
use swc_ecma_visit::{Visit, VisitWith};

/// What Composer writes on every widget it drops, before the widget's own defaults.
fn dropped() -> [(&'static str, Value); 8] {
    [
        ("Area", Value::from("UI")),
        ("DisableLegacyCustomClassStyles", Value::from(false)),
        ("Left", Value::from(0)),
        ("Margin", Value::from("5")),
        ("ShowDataLoading", Value::from(true)),
        ("Top", Value::from(0)),
        ("Visible", Value::from(true)),
        ("Z-index", Value::from(10)),
    ]
}

/// Folders that hold build output or dependencies, never a project's own widget source.
const SKIPPED: [&str; 8] = [
    "node_modules",
    "build",
    "dist",
    "zip",
    "target",
    "out",
    ".git",
    ".gradle",
];

/// Larger files are bundles, not source.
const MAX_SOURCE: u64 = 2 * 1024 * 1024;

/// More source files than a widget project has: the root holds something else as well.
const MAX_FILES: usize = 2000;

/// Every widget the solution's extension projects define, then those of the extensions the
/// platform cache saw installed on the server, and what could not be read.
pub(crate) fn load(solution: &Solution) -> (Vec<Widget>, Vec<String>) {
    let (mut found, mut skipped) = load_projects(solution);
    if let Some(platform) = super::types::platform::cached(solution) {
        if !platform.extension_widgets.is_empty() {
            match read_source(&platform.extension_widgets, false) {
                Ok(widgets) => found.extend(widgets),
                Err(why) => skipped.push(format!(".twaco/platform.json extension widgets: {why}")),
            }
        }
    }
    (found, skipped)
}

fn load_projects(solution: &Solution) -> (Vec<Widget>, Vec<String>) {
    let mut found = Vec::new();
    let mut skipped = Vec::new();
    for extension in &solution.extensions {
        let root = solution.root.join(&extension.root);
        if let Err(error) = std::fs::read_dir(&root) {
            skipped.push(format!("{}: {error}", solution.relative(&root)));
            continue;
        }
        let mut files = Vec::new();
        collect_sources(&root, &mut files);
        if files.len() > MAX_FILES {
            skipped.push(format!(
                "{}: more than {MAX_FILES} source files; point the [[extension]] root at the widget project itself",
                solution.relative(&root)
            ));
            continue;
        }
        files.sort();
        for file in files {
            match read_file(&file) {
                Ok(widgets) => found.extend(widgets),
                Err(why) => skipped.push(format!("{}: {why}", solution.relative(&file))),
            }
        }
    }
    (found, skipped)
}

/// The widget an extension project defines under `word`, its type ignoring case, and the
/// source files that could not be read.
pub(crate) fn find(solution: &Solution, word: &str) -> (Option<Widget>, Vec<String>) {
    let wanted = word.trim();
    let (widgets, skipped) = load(solution);
    let found = widgets
        .into_iter()
        .find(|widget| widget.type_id.eq_ignore_ascii_case(wanted));
    (found, skipped)
}

fn collect_sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() > MAX_FILES {
            return;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if kind.is_dir() {
            if !SKIPPED.contains(&name.as_ref()) {
                collect_sources(&path, out);
            }
        } else if kind.is_file()
            && (name.ends_with(".js") || name.ends_with(".ts"))
            && !name.ends_with(".d.ts")
            && !name.ends_with(".min.js")
            && entry.metadata().is_ok_and(|m| m.len() <= MAX_SOURCE)
        {
            out.push(path);
        }
    }
}

fn read_file(path: &Path) -> Result<Vec<Widget>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let typescript = path.extension().is_some_and(|e| e == "ts");
    if typescript {
        if !(text.contains("TWWidgetDefinition") && text.contains("widgetIDESupport")) {
            return Ok(Vec::new());
        }
    } else if !text.contains("TW.IDE.Widgets")
        && !text.contains("widgetWrapper")
        && !text.contains("TWWidgetDefinition")
    {
        return Ok(Vec::new());
    }
    read_source(&text, typescript)
}

/// The widgets one IDE source file defines.
pub(crate) fn read_source(text: &str, typescript: bool) -> Result<Vec<Widget>, String> {
    Ok(parse(text, typescript, false)?
        .found
        .into_iter()
        .map(|found| found.widget)
        .collect())
}

/// One widget read statically, with what the reading could not take.
pub(crate) struct Found {
    pub(crate) widget: Widget,
    /// A property whose default is not a literal, and the like: said, never guessed.
    pub(crate) notes: Vec<String>,
    /// Why the contract cannot be known statically; the widget's contract is then not trusted.
    pub(crate) unknown: Option<String>,
}

/// What [`read_report`] found in one file.
#[derive(Default)]
pub(crate) struct Report {
    pub(crate) found: Vec<Found>,
    /// Widgets registered under a name whose contract is built at run time (nothing to read).
    pub(crate) unnamed: Vec<(String, String)>,
}

/// The widgets one source file defines, with a verdict on each contract: the last `return` of
/// each definition function, identifiers bound to literals, webpack dev-mode `eval("...")`
/// modules and `translate("key")` values are read; a widget built any other way is `unknown`.
pub(crate) fn read_report(text: &str, typescript: bool) -> Result<Report, String> {
    parse(text, typescript, true)
}

fn parse(text: &str, typescript: bool, report: bool) -> Result<Report, String> {
    let mut collector = Collector {
        report,
        ..Collector::default()
    };
    collector.parse_text(text, typescript)?;
    Ok(Report {
        found: collector.found,
        unnamed: collector.unnamed,
    })
}

#[derive(Default)]
struct Collector {
    found: Vec<Found>,
    unnamed: Vec<(String, String)>,
    /// The latest value each `const`/`let`/`var` name was declared with, in source order: a
    /// wrapper registration names its widget and configuration through them.
    declared: HashMap<String, Expr>,
    /// Read for a verdict on each contract, not for authoring.
    report: bool,
    /// How many `eval` modules deep the text being read is.
    depth: u8,
    compiled: DecoratorScope,
}

#[derive(Default)]
struct DecoratorScope {
    classes: HashMap<String, CompiledClass>,
    names: HashMap<String, String>,
    aspects: HashMap<String, (String, bool)>,
    name_helpers: Vec<String>,
}

impl DecoratorScope {
    fn name(&self, expression: &Expr) -> Option<String> {
        let name = decorator_name(expression)?;
        Some(self.names.get(&name).cloned().unwrap_or(name))
    }
}

#[derive(Default)]
struct CompiledClass {
    members: HashMap<String, Parts>,
    found: Option<usize>,
}

impl Collector {
    fn resolve<'a>(&'a self, expr: &'a Expr) -> &'a Expr {
        match expr {
            Expr::Ident(ident) => self.declared.get(ident.sym.as_ref()).unwrap_or(expr),
            other => other,
        }
    }

    fn parse_text(&mut self, text: &str, typescript: bool) -> Result<(), String> {
        let end = u32::try_from(text.len()).map_err(|_| "too large to parse".to_string())?;
        let syntax = if typescript {
            Syntax::Typescript(TsSyntax {
                decorators: true,
                ..Default::default()
            })
        } else {
            Syntax::Es(EsSyntax::default())
        };
        let lexer = Lexer::new(
            syntax,
            Default::default(),
            StringInput::new(text, BytePos(0), BytePos(end)),
            None,
        );
        let program = super::script::guard_parse(|| Parser::new_from(lexer).parse_program())
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("cannot parse it: {}", error.kind().msg()))?;
        let outer = std::mem::take(&mut self.compiled);
        program.visit_with(self);
        self.compiled = outer;
        Ok(())
    }

    /// esbuild's keepNames helper is an arrow that sets a function or class's name.
    fn is_name_helper(&self, init: &Expr) -> bool {
        let Expr::Arrow(arrow) = init else {
            return false;
        };
        let ([Pat::Ident(target), Pat::Ident(value)], BlockStmtOrExpr::Expr(body)) =
            (arrow.params.as_slice(), &*arrow.body)
        else {
            return false;
        };
        let Expr::Call(call) = &**body else {
            return false;
        };
        let (Callee::Expr(callee), [owner, name, descriptor]) =
            (&call.callee, call.args.as_slice())
        else {
            return false;
        };
        member_path(self.resolve(callee)).is_some_and(|path| path == ["Object", "defineProperty"])
            && matches!(&*owner.expr, Expr::Ident(id) if id.sym == target.id.sym)
            && string(&name.expr).as_deref() == Some("name")
            && matches!(&*descriptor.expr, Expr::Object(object) if pairs(object).any(|(key, expr)| {
                key == "value" && matches!(expr, Expr::Ident(id) if id.sym == value.id.sym)
            }))
    }

    fn read_compiled_decorator(&mut self, call: &CallExpr, registration: Option<&str>) -> bool {
        let Callee::Expr(callee) = &call.callee else {
            return false;
        };
        let Some(helper) = decorator_name(callee) else {
            return false;
        };
        let named = matches!(helper.as_str(), "__decorate" | "__decorateClass");
        if !named && self.compiled.names.is_empty() {
            return false;
        }
        let [first, target, rest @ ..] = call.args.as_slice() else {
            return false;
        };
        let Some(path) = member_path(&target.expr) else {
            return false;
        };
        // In a minified esbuild bundle, keepNames still records the class and
        // decorator names. Associate the member calls with the class call by helper.
        if !named && !self.compiled.names.contains_key(&path[0]) {
            return false;
        }
        if first.spread.is_some() || target.spread.is_some() {
            return false;
        }
        let decorators = match &*first.expr {
            Expr::Array(array) => Some(&array.elems),
            _ => None,
        };
        let static_decorators: Vec<_> = decorators
            .into_iter()
            .flatten()
            .flatten()
            .filter(|element| element.spread.is_none())
            .filter_map(|element| decorator_expr(&element.expr, &self.compiled))
            .collect();
        let computed = decorators.is_none_or(|entries| entries.len() != static_decorators.len());
        match (path.as_slice(), rest) {
            ([class, prototype], [member, ..]) if prototype == "prototype" => {
                if !named
                    && !matches!(rest, [_, kind] if matches!(literal(&kind.expr), Some(value) if value == 1 || value == 2))
                {
                    return false;
                }
                let mut parts = self
                    .compiled
                    .classes
                    .entry(class.clone())
                    .or_default()
                    .members
                    .remove(&helper)
                    .unwrap_or_default();
                if computed {
                    parts
                        .unknown
                        .push("a member has computed decorators".into());
                }
                if let Some(name) = string(&member.expr) {
                    parts.read_decorated_member(name, &static_decorators, &self.compiled);
                } else {
                    parts
                        .unknown
                        .push("a decorated member has a computed name".into());
                }
                self.compiled
                    .classes
                    .entry(class.clone())
                    .or_default()
                    .members
                    .insert(helper, parts);
            }
            ([class], []) => {
                let Some((_, arguments)) = static_decorators
                    .iter()
                    .find(|(name, _)| name == "TWWidgetDefinition")
                else {
                    return false;
                };
                let compiled = self.compiled.classes.entry(class.clone()).or_default();
                let mut parts = compiled.members.remove(&helper).unwrap_or_default();
                compiled.found = Some(self.found.len());
                parts.read_definition(arguments, self.report, &self.compiled);
                if computed {
                    parts
                        .unknown
                        .push("the widget definition has a computed decorator".into());
                }
                let type_id = registration
                    .or_else(|| self.compiled.names.get(class).map(String::as_str))
                    .unwrap_or(class)
                    .to_owned();
                self.push(parts, type_id);
            }
            _ => return false,
        }
        true
    }

    fn push(&mut self, parts: Parts, type_id: String) {
        // A registration helper that names nothing is no widget.
        if type_id.is_empty() {
            return;
        }
        let notes = parts.notes.clone();
        let mut unknown = parts.unknown.first().cloned();
        if unknown.is_none() && !parts.saw_properties && !parts.wrapper {
            unknown = Some("no widgetProperties function".to_string());
        }
        self.found.push(Found {
            widget: parts.into_widget(type_id),
            notes,
            unknown,
        });
    }
}

impl Visit for Collector {
    fn visit_function(&mut self, function: &Function) {
        let outer = std::mem::take(&mut self.compiled);
        function.visit_children_with(self);
        self.compiled = outer;
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        let outer = std::mem::take(&mut self.compiled);
        arrow.visit_children_with(self);
        self.compiled = outer;
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(name), Some(init)) = (&declarator.name, &declarator.init) {
            if self.is_name_helper(init) {
                self.compiled.name_helpers.push(name.id.sym.to_string());
            }
            if let Expr::Call(call) = &**init {
                if let (Callee::Expr(callee), [key, value]) = (&call.callee, call.args.as_slice()) {
                    if decorator_name(callee).as_deref() == Some("aspectWithKeyAndValue") {
                        if let (Some(key), Some(Value::Bool(value))) =
                            (string(&key.expr), literal(&value.expr))
                        {
                            self.compiled
                                .aspects
                                .insert(name.id.sym.to_string(), (key, value));
                        }
                    }
                }
            }
            self.declared
                .insert(name.id.sym.to_string(), (**init).clone());
        }
        declarator.visit_children_with(self);
    }

    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let (Callee::Expr(callee), [target, name]) = (&call.callee, call.args.as_slice()) {
            if decorator_name(callee).is_some_and(|helper| {
                helper == "__name" || self.compiled.name_helpers.contains(&helper)
            }) {
                if let (Expr::Ident(target), Some(name)) = (&*target.expr, string(&name.expr)) {
                    self.compiled.names.insert(target.sym.to_string(), name);
                }
            }
        }
        self.read_compiled_decorator(call, None);
        if let Callee::Expr(callee) = &call.callee {
            let registers = member_path(callee).is_some_and(|path| {
                path.len() >= 2 && path[path.len() - 2..] == ["widgetWrapper", "config"]
            });
            if registers && call.args.len() == 2 {
                let name = string(self.resolve(&call.args[0].expr));
                if let (Some(name), Expr::Object(config)) = (name, self.resolve(&call.args[1].expr))
                {
                    let mut parts = Parts {
                        wrapper: true,
                        report: self.report,
                        ..Parts::default()
                    };
                    parts.read_wrapper(config);
                    self.push(parts, name);
                }
            }
            // Webpack's dev mode wraps each module as `eval("...")`: its text is the code.
            if self.report
                && self.depth < 2
                && matches!(&**callee, Expr::Ident(i) if i.sym.as_ref() == "eval")
            {
                if let Some(text) = call.args.first().and_then(|arg| string(&arg.expr)) {
                    if text.contains("TW.IDE.Widgets")
                        || text.contains("widgetWrapper")
                        || text.contains("TWWidgetDefinition")
                    {
                        self.depth += 1;
                        // A module that does not parse is no worse than a file that does not.
                        let _ = self.parse_text(&text, false);
                        self.depth -= 1;
                    }
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let Some(type_id) = ide_widget_name(&assign.left) {
            if let Expr::Call(call) = &*assign.right {
                if self.read_compiled_decorator(call, Some(&type_id)) {
                    return;
                }
            }
            if let Expr::Ident(class) = &*assign.right {
                if let Some(index) = self
                    .compiled
                    .classes
                    .get(class.sym.as_ref())
                    .and_then(|c| c.found)
                {
                    self.found[index].widget.type_id = type_id;
                    return;
                }
            }
            match &*assign.right {
                Expr::Fn(function) => {
                    let mut parts = Parts {
                        report: self.report,
                        ..Parts::default()
                    };
                    function.function.visit_with(&mut parts);
                    self.push(parts, type_id);
                }
                _ if self.report => self.unnamed.push((
                    type_id,
                    "the widget is built at run time, not by a function literal".to_string(),
                )),
                _ => {}
            }
        }
        assign.visit_children_with(self);
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        if let Some(parts) = typescript_widget(class, self.report, &self.compiled) {
            self.push(parts, class.ident.sym.to_string());
        }
        class.visit_children_with(self);
    }
}

/// `<type>` of `TW.IDE.Widgets.<type> = ...` or `TW.IDE.Widgets["<type>"] = ...`.
fn ide_widget_name(target: &AssignTarget) -> Option<String> {
    let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = target else {
        return None;
    };
    let path = member_path(&member.obj)?;
    if path != ["TW", "IDE", "Widgets"] {
        return None;
    }
    property_name(&member.prop)
}

fn member_path(expr: &Expr) -> Option<Vec<String>> {
    match expr {
        Expr::Ident(ident) => Some(vec![ident.sym.to_string()]),
        Expr::Member(MemberExpr { obj, prop, .. }) => {
            let mut path = member_path(obj)?;
            path.push(property_name(prop)?);
            Some(path)
        }
        _ => None,
    }
}

fn property_name(prop: &MemberProp) -> Option<String> {
    match prop {
        MemberProp::Ident(ident) => Some(ident.sym.to_string()),
        MemberProp::Computed(computed) => string(&computed.expr),
        MemberProp::PrivateName(_) => None,
    }
}

fn string(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(Lit::Str(text)) => Some(text.value.to_atom_lossy().to_string()),
        Expr::Tpl(template) if template.exprs.is_empty() && template.quasis.len() == 1 => template
            .quasis[0]
            .cooked
            .as_ref()
            .map(|cooked| cooked.to_atom_lossy().to_string()),
        Expr::Paren(inner) => string(&inner.expr),
        _ => None,
    }
}

/// A JSON value for a literal expression; `None` for anything worked out at run time.
fn literal(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Lit(Lit::Bool(value)) => Some(Value::from(value.value)),
        Expr::Lit(Lit::Num(number)) => number_value(number.value),
        Expr::Lit(Lit::Null(_)) => Some(Value::Null),
        Expr::Unary(unary) if unary.op == UnaryOp::Minus => match &*unary.arg {
            Expr::Lit(Lit::Num(number)) => number_value(-number.value),
            _ => None,
        },
        // Minified booleans.
        Expr::Unary(unary) if unary.op == UnaryOp::Bang => match &*unary.arg {
            Expr::Lit(Lit::Num(number)) if number.value == 0.0 => Some(Value::from(true)),
            Expr::Lit(Lit::Num(number)) if number.value == 1.0 => Some(Value::from(false)),
            _ => None,
        },
        // A label is its translation key: the server translates it, the contract is its text.
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            let translate = member_path(callee)
                .is_some_and(|path| path == ["TW", "IDE", "I18NController", "translate"]);
            match (translate, call.args.first()) {
                (true, Some(key)) => string(&key.expr).map(Value::from),
                _ => None,
            }
        }
        Expr::Paren(inner) => literal(&inner.expr),
        // An object or array counts only when every part of it is a literal.
        Expr::Object(object) => {
            let mut out = Map::new();
            for property in &object.props {
                let PropOrSpread::Prop(prop) = property else {
                    return None;
                };
                let Prop::KeyValue(pair) = &**prop else {
                    return None;
                };
                out.insert(key(&pair.key)?, literal(&pair.value)?);
            }
            Some(Value::Object(out))
        }
        Expr::Array(array) => array
            .elems
            .iter()
            .map(|element| match element {
                Some(element) if element.spread.is_none() => literal(&element.expr),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        other => string(other).map(Value::from),
    }
}

fn number_value(number: f64) -> Option<Value> {
    if number.fract() == 0.0 && number.abs() < 9.0e15 {
        Some(Value::from(super::number::saturating_i64(number)))
    } else {
        serde_json::Number::from_f64(number).map(Value::Number)
    }
}

fn key(name: &PropName) -> Option<String> {
    match name {
        PropName::Ident(ident) => Some(ident.sym.to_string()),
        PropName::Str(text) => Some(text.value.to_atom_lossy().to_string()),
        PropName::Num(number) => Some(match number_value(number.value)? {
            Value::Number(number) => number.to_string(),
            _ => return None,
        }),
        _ => None,
    }
}

fn pairs(object: &ObjectLit) -> impl Iterator<Item = (String, &Expr)> {
    object.props.iter().filter_map(|property| {
        let PropOrSpread::Prop(prop) = property else {
            return None;
        };
        match &**prop {
            Prop::KeyValue(pair) => Some((key(&pair.key)?, &*pair.value)),
            _ => None,
        }
    })
}

/// What the IDE function's `widgetProperties` and `widgetEvents` return.
#[derive(Default)]
struct Parts {
    label: Option<String>,
    auto_resize: bool,
    properties: Vec<(String, Declared)>,
    events: Vec<String>,
    /// A web-component widget: Composer themes it as its own PTCS widgets.
    wrapper: bool,
    /// Read for a verdict on each contract (services included), not for authoring.
    report: bool,
    services: Vec<String>,
    saw_properties: bool,
    /// Why the contract cannot be read statically; the first one is the verdict.
    unknown: Vec<String>,
    notes: Vec<String>,
    /// The `var x = {...}` declarations of the definition function being read.
    locals: HashMap<String, Expr>,
}

#[derive(Default)]
struct Declared {
    options: Vec<Value>,
    base_type: Option<String>,
    default: Option<Value>,
    source: bool,
    target: bool,
    editable: bool,
}

impl Visit for Parts {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        let AssignTarget::Simple(SimpleAssignTarget::Member(member)) = &assign.left else {
            return assign.visit_children_with(self);
        };
        let (Expr::This(_), Some(name)) = (&*member.obj, property_name(&member.prop)) else {
            return assign.visit_children_with(self);
        };
        let part = matches!(
            name.as_str(),
            "widgetProperties" | "widgetEvents" | "widgetServices"
        );
        if name == "widgetProperties" {
            self.saw_properties = true;
        }
        let Expr::Fn(function) = &*assign.right else {
            if part {
                self.unknown
                    .push(format!("{name} is not a function literal"));
            }
            return;
        };
        self.locals = locals(&function.function);
        let returned = match returned_object(&function.function, &self.locals) {
            Ok(returned) => returned.clone(),
            Err(why) => {
                if part {
                    self.unknown.push(format!("{name} {why}"));
                }
                return;
            }
        };
        match name.as_str() {
            "widgetProperties" => self.read_properties(&returned),
            "widgetEvents" => {
                let names = self.names(&returned, "widgetEvents");
                self.events.extend(names);
            }
            "widgetServices" => {
                let names = self.names(&returned, "widgetServices");
                self.services.extend(names);
            }
            _ => {}
        }
    }
}

impl Parts {
    fn read_properties(&mut self, object: &ObjectLit) {
        for (name, value) in pairs(object) {
            match name.as_str() {
                "name" => {
                    self.label = literal(value).and_then(|label| label.as_str().map(str::to_string))
                }
                "supportsAutoResize" => {
                    self.auto_resize = literal(value) == Some(Value::from(true))
                }
                "properties" => match resolve_local(value, &self.locals) {
                    Expr::Object(properties) => {
                        let properties = properties.clone();
                        self.read_property_map(&properties);
                    }
                    _ => self
                        .unknown
                        .push("properties is not an object literal".to_string()),
                },
                _ => {}
            }
        }
    }

    /// The keys of an object literal the widget lists (events, services); a spread or a computed
    /// key means the list is not all there.
    fn names(&mut self, object: &ObjectLit, what: &str) -> Vec<String> {
        if pairs(object).count() != object.props.len() {
            self.unknown
                .push(format!("{what} has an entry that is not name: value"));
        }
        pairs(object).map(|(name, _)| name).collect()
    }

    /// A wrapper configuration: `flags` holds the label and auto-resize, `properties` and
    /// `events` are as an IDE widget's.
    fn read_wrapper(&mut self, config: &ObjectLit) {
        for (name, value) in pairs(config) {
            let Expr::Object(object) = value else {
                continue;
            };
            match name.as_str() {
                "flags" => self.read_properties(object),
                "properties" => self.read_property_map(object),
                "events" => {
                    let names = self.names(object, "events");
                    self.events.extend(names);
                }
                "services" => {
                    let names = self.names(object, "services");
                    self.services.extend(names);
                }
                _ => {}
            }
        }
    }

    fn read_property_map(&mut self, properties: &ObjectLit) {
        if pairs(properties).count() != properties.props.len() {
            self.unknown
                .push("properties has an entry that is not name: value".to_string());
        }
        for (property, declaration) in pairs(properties) {
            let Expr::Object(declaration) = resolve_local(declaration, &self.locals).clone() else {
                self.notes.push(format!(
                    "{property}: its declaration is not an object literal"
                ));
                continue;
            };
            let mut declared = Declared {
                editable: true,
                ..Declared::default()
            };
            for (field, value) in pairs(&declaration) {
                let flag = literal(value) == Some(Value::from(true));
                match field.as_str() {
                    "baseType" => declared.base_type = string(value),
                    "defaultValue" => {
                        declared.default = literal(value);
                        if declared.default.is_none() {
                            self.notes
                                .push(format!("{property}: its default is not a literal"));
                        }
                    }
                    "isBindingSource" => declared.source = flag,
                    "isBindingTarget" => declared.target = flag,
                    "isEditable" => declared.editable = literal(value) != Some(Value::from(false)),
                    "selectOptions" => {
                        if let Some(Value::Array(options)) = literal(value) {
                            declared.options = options
                                .iter()
                                .filter_map(|option| option.get("value").cloned())
                                .collect();
                        }
                    }
                    _ => {}
                }
            }
            self.properties.push((property, declared));
        }
    }

    fn into_widget(self, type_id: String) -> Widget {
        let mut defaults = Map::new();
        for (name, value) in dropped() {
            defaults.insert(name.to_string(), value);
        }
        if self.wrapper {
            defaults.insert("UseTheme".to_string(), Value::from(true));
        }
        let mut contracts = BTreeMap::new();
        for (name, declared) in self.properties {
            if let Some(value) = declared.default.clone() {
                defaults.insert(name.clone(), value);
            }
            contracts.insert(
                name,
                PropertyContract {
                    base_type: declared.base_type,
                    css_size: false,
                    source: declared.source,
                    target: declared.target,
                    editable: declared.editable,
                    options: declared.options,
                },
            );
        }
        let services = self.report.then_some(self.services);
        Widget::extension(
            type_id.clone(),
            self.label.unwrap_or_else(|| type_id.clone()),
            self.auto_resize,
            defaults,
            contracts,
            self.events,
            services,
        )
    }
}

/// The `var`/`let`/`const` declarations among a function's own statements.
fn locals(function: &Function) -> HashMap<String, Expr> {
    let mut out = HashMap::new();
    for statement in function.body.iter().flat_map(|body| &body.stmts) {
        if let Stmt::Decl(Decl::Var(declaration)) = statement {
            for declarator in &declaration.decls {
                if let (Pat::Ident(name), Some(init)) = (&declarator.name, &declarator.init) {
                    out.insert(name.id.sym.to_string(), (**init).clone());
                }
            }
        }
    }
    out
}

/// A name bound to a value in the function being read stands for that value.
fn resolve_local<'a>(expr: &'a Expr, locals: &'a HashMap<String, Expr>) -> &'a Expr {
    match expr {
        Expr::Ident(ident) => locals.get(ident.sym.as_ref()).unwrap_or(expr),
        Expr::Paren(inner) => resolve_local(&inner.expr, locals),
        // `return this.x = f(), {...}`: a comma list is worth its last value.
        Expr::Seq(sequence) => sequence
            .exprs
            .last()
            .map_or(expr, |last| resolve_local(last, locals)),
        other => other,
    }
}

/// The object literal a function's last `return` gives: `return {...}`, a parenthesised or comma
/// list ending in one, or a name bound to one. Any other function says why it is not read.
fn returned_object<'a>(
    function: &'a Function,
    locals: &'a HashMap<String, Expr>,
) -> Result<&'a ObjectLit, String> {
    let returned = function
        .body
        .iter()
        .flat_map(|body| &body.stmts)
        .filter_map(|statement| match statement {
            Stmt::Return(returned) => Some(returned),
            _ => None,
        })
        .next_back()
        .ok_or_else(|| "has no return statement".to_string())?;
    let arg = returned
        .arg
        .as_deref()
        .ok_or_else(|| "returns nothing".to_string())?;
    match resolve_local(arg, locals) {
        Expr::Object(object) => Ok(object),
        _ => Err("returns something other than an object literal".to_string()),
    }
}

/// Babel and TypeScript's CommonJS emitter wrap imported helpers in `(0, module.helper)`.
fn decorator_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(ident) => Some(ident.sym.to_string()),
        Expr::Member(member) => property_name(&member.prop),
        Expr::Paren(paren) => decorator_name(&paren.expr),
        Expr::Seq(seq)
            if seq.exprs.len() == 2
                && matches!(&*seq.exprs[0], Expr::Lit(Lit::Num(n)) if n.value == 0.0) =>
        {
            decorator_name(&seq.exprs[1])
        }
        _ => None,
    }
}

fn decorator_expr<'a>(expr: &'a Expr, scope: &DecoratorScope) -> Option<(String, Vec<&'a Expr>)> {
    if let Expr::Call(call) = expr {
        let Callee::Expr(callee) = &call.callee else {
            return None;
        };
        if call.args.iter().any(|arg| arg.spread.is_some()) {
            return None;
        }
        Some((
            scope.name(callee)?,
            call.args.iter().map(|arg| &*arg.expr).collect(),
        ))
    } else {
        Some((scope.name(expr)?, Vec::new()))
    }
}

fn decorator<'a>(
    decorator: &'a Decorator,
    scope: &DecoratorScope,
) -> Option<(String, Vec<&'a Expr>)> {
    decorator_expr(&decorator.expr, scope)
}

impl Declared {
    fn read_binding_type(&mut self, expression: &Expr) -> bool {
        match literal(expression)
            .and_then(|v| v.as_str().map(str::to_ascii_lowercase))
            .as_deref()
        {
            Some("source") => self.source = true,
            Some("target") => self.target = true,
            Some("both") => {
                self.source = true;
                self.target = true;
            }
            Some("none") => {}
            _ => return false,
        }
        true
    }

    fn read_decorator_argument(&mut self, argument: &Expr, scope: &DecoratorScope) -> bool {
        if let Expr::Object(object) = argument {
            if pairs(object).count() != object.props.len() {
                return false;
            }
            return pairs(object).all(|(field, value)| match field.as_str() {
                "baseType" => {
                    self.base_type = string(value);
                    self.base_type.is_some()
                }
                "bindingType" => self.read_binding_type(value),
                "defaultValue" => {
                    self.default = literal(value);
                    self.default.is_some()
                }
                "isBindingSource" | "isBindingTarget" | "isEditable" => {
                    let Some(Value::Bool(flag)) = literal(value) else {
                        return false;
                    };
                    match field.as_str() {
                        "isBindingSource" => self.source = flag,
                        "isBindingTarget" => self.target = flag,
                        _ => self.editable = flag,
                    }
                    true
                }
                _ => true,
            });
        }
        if let Expr::Ident(alias) = argument {
            if let Some((key, flag)) = scope.aspects.get(alias.sym.as_ref()) {
                match key.as_str() {
                    "isBindingSource" => self.source = *flag,
                    "isBindingTarget" => self.target = *flag,
                    "isEditable" => self.editable = *flag,
                    _ => return false,
                }
                return true;
            }
        }
        if let Some((helper, args)) = decorator_expr(argument, scope) {
            return match (helper.as_str(), args.as_slice()) {
                ("bindingSource", []) => {
                    self.source = true;
                    true
                }
                ("bindingTarget", []) => {
                    self.target = true;
                    true
                }
                ("nonEditable", []) => {
                    self.editable = false;
                    true
                }
                ("defaultValue", [value]) => {
                    self.default = literal(value);
                    self.default.is_some()
                }
                ("bindingType", [value]) => self.read_binding_type(value),
                _ => false,
            };
        }
        if self.base_type.is_none() {
            self.base_type = string(argument);
            return self.base_type.is_some();
        }
        false
    }
}

impl Parts {
    fn read_definition(&mut self, arguments: &[&Expr], report: bool, scope: &DecoratorScope) {
        self.saw_properties = true;
        self.report = report;
        for argument in arguments {
            if let Expr::Ident(alias) = argument {
                if let Some((key, flag)) = scope.aspects.get(alias.sym.as_ref()) {
                    if key == "supportsAutoResize" {
                        self.auto_resize = *flag;
                        continue;
                    }
                }
            }
            if scope.name(argument).as_deref() == Some("autoResizable") {
                self.auto_resize = true;
            } else if self.label.is_none() {
                self.label = string(argument);
                if self.label.is_none() {
                    self.unknown
                        .push("TWWidgetDefinition has a nonliteral argument".into());
                }
            } else {
                self.unknown
                    .push("TWWidgetDefinition has an unrecognized argument".into());
            }
        }
    }

    fn read_decorated_member(
        &mut self,
        name: String,
        decorators: &[(String, Vec<&Expr>)],
        scope: &DecoratorScope,
    ) {
        if decorators.iter().any(|(name, _)| name == "event") {
            self.events.push(name);
        } else if decorators.iter().any(|(name, _)| name == "service") {
            self.services.push(name);
        } else if let Some((_, arguments)) = decorators.iter().find(|(name, _)| name == "property")
        {
            let mut declared = Declared {
                editable: true,
                ..Declared::default()
            };
            if !arguments
                .iter()
                .all(|arg| declared.read_decorator_argument(arg, scope))
                || declared.base_type.is_none()
            {
                self.unknown.push(format!("{name}: the property decorator needs a literal base type, binding flags and default"));
            }
            self.properties.push((name, declared));
        }
    }
}

fn typescript_widget(class: &ClassDecl, report: bool, scope: &DecoratorScope) -> Option<Parts> {
    let (_, arguments) = class
        .class
        .decorators
        .iter()
        .filter_map(|value| decorator(value, scope))
        .find(|(name, _)| name == "TWWidgetDefinition")?;
    let mut parts = Parts::default();
    parts.read_definition(&arguments, report, scope);
    for member in &class.class.body {
        let (property, decorators) = match member {
            ClassMember::ClassProp(prop) if !prop.is_static => (&prop.key, &prop.decorators),
            ClassMember::Method(method) if !method.is_static => {
                (&method.key, &method.function.decorators)
            }
            _ => continue,
        };
        let declared: Vec<_> = decorators
            .iter()
            .filter_map(|value| decorator(value, scope))
            .collect();
        if let Some(name) = key(property) {
            parts.read_decorated_member(name, &declared, scope);
        } else if !declared.is_empty() {
            parts
                .unknown
                .push("a decorated member has a computed name".into());
        }
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::read_source;
    use serde_json::json;

    #[test]
    fn a_parser_panic_is_a_source_error() {
        assert!(read_source("( tableName , `t${x}` =>", false).is_err());
    }

    #[test]
    fn a_javascript_ide_widget_gives_its_type_label_defaults_and_contract() {
        let source = r#"
TW.IDE.Widgets.acmegauge = function () {
    this.widgetIconUrl = function () { return 'x.png'; };
    this.widgetProperties = function () {
        return {
            name: 'Acme Gauge',
            supportsAutoResize: true,
            properties: {
                Value: { baseType: 'NUMBER', defaultValue: -1.5, isBindingTarget: true },
                Title: { baseType: 'STRING', defaultValue: "Gauge" },
                Options: { baseType: 'JSON', defaultValue: { theme: 'dark', sizes: [1, 2], 1: 'x' } },
                Partly: { baseType: 'JSON', defaultValue: { theme: TW.theme } },
                Unset: { baseType: 'STRING' },
                Computed: { baseType: 'STRING', defaultValue: TW.IDE.something() },
                Locked: { baseType: 'BOOLEAN', isEditable: false, isBindingSource: true }
            }
        };
    };
    this.widgetEvents = function () { return { Changed: {} }; };
};
TW.IDE.Widgets["other-one"] = function () {};
"#;
        let widgets = read_source(source, false).unwrap();
        let names: Vec<_> = widgets.iter().map(|w| w.type_id.as_str()).collect();
        assert_eq!(names, ["acmegauge", "other-one"]);
        let gauge = &widgets[0];
        assert_eq!(gauge.type_display_name, "Acme Gauge");
        assert!(gauge.auto_resize);
        assert_eq!(gauge.defaults["Value"], json!(-1.5));
        assert_eq!(gauge.defaults["Title"], json!("Gauge"));
        // Composer writes no value for a property without a default.
        assert!(!gauge.defaults.contains_key("Unset"));
        assert!(!gauge.defaults.contains_key("Locked"));
        assert!(!gauge.defaults.contains_key("Computed"));
        assert_eq!(
            gauge.defaults["Options"],
            json!({"theme": "dark", "sizes": [1, 2], "1": "x"})
        );
        assert!(!gauge.defaults.contains_key("Partly"));
        assert_eq!(gauge.defaults["Area"], json!("UI"));
        let value = gauge.property("Value").unwrap();
        assert!(value.target && !value.source);
        assert_eq!(value.base_type.as_deref(), Some("NUMBER"));
        let locked = gauge.property("Locked").unwrap();
        assert!(locked.source && !locked.editable);
        assert!(gauge.has_event("Changed"));
        assert_eq!(widgets[1].type_display_name, "other-one");
    }

    /// The shape of the mashup builder's combined extension code: each web-component widget
    /// registers its configuration, then its IDE function reaches it through a local alias.
    #[test]
    fn a_wrapper_configuration_gives_a_themed_widget() {
        let source = r#"
// ----BEGIN: extensions/acme-chat/ui/acmechat/acmechat.config.js
(function(TW) {
    const widgetName = 'acmechat';
    const config = {"elementName":"acme-chat","flags":{"name":"Acme Chat","supportsAutoResize":true},"properties":{"CustomClass":{"baseType":"STRING","isBindingSource":true},"Placeholder":{"baseType":"STRING","isBindingTarget":true,"defaultValue":"Ask"},"HistoryMaxItems":{"baseType":"INTEGER","defaultValue":500}},"events":{"Sent":{"src":"Sent"}}};
    TW.Widget.widgetWrapper = TW.Widget.widgetWrapper || { configs: {} };
    TW.Widget.widgetWrapper.config(widgetName, config);
}(TW));
// ----BEGIN: extensions/acme-chat/ui/acmechat/acmechat.ide.js
(function(widgetName, isIDE) {
    const widgets = isIDE ? TW.IDE.Widgets : TW.Runtime.Widgets;
    widgets[widgetName] = function() {
        const config = TW.Widget.widgetWrapper.config(widgetName);
    };
}('acmechat', true));
"#;
        let widgets = read_source(source, false).unwrap();
        assert_eq!(widgets.len(), 1, "{widgets:?}");
        let chat = &widgets[0];
        assert_eq!(chat.type_id, "acmechat");
        assert_eq!(chat.type_display_name, "Acme Chat");
        assert!(chat.auto_resize);
        assert_eq!(chat.defaults["Placeholder"], json!("Ask"));
        assert_eq!(chat.defaults["HistoryMaxItems"], json!(500));
        assert_eq!(chat.defaults["UseTheme"], json!(true));
        assert!(!chat.defaults.contains_key("CustomClass"));
        assert!(chat.property("CustomClass").unwrap().source);
        assert!(chat.has_event("Sent"));
    }

    #[test]
    fn a_typescript_widget_class_gives_its_decorated_members() {
        let source = r#"
import { TWWidgetDefinition, autoResizable, property, defaultValue, bindingTarget, event } from 'typescriptwebpacksupport/widgetIDESupport';

@TWWidgetDefinition('Acme Label', autoResizable)
class AcmeLabel extends TWComposerWidget {
    @property('NUMBER', defaultValue(120)) Width: number;
    @property('STRING', bindingTarget, defaultValue('hello')) Value: string;
    @event Clicked;
    plain: string;
    renderHtml(): string { return ''; }
}
"#;
        let widgets = read_source(source, true).unwrap();
        assert_eq!(widgets.len(), 1);
        let label = &widgets[0];
        assert_eq!(label.type_id, "AcmeLabel");
        assert_eq!(label.type_display_name, "Acme Label");
        assert!(label.auto_resize);
        assert_eq!(label.defaults["Width"], json!(120));
        assert_eq!(label.defaults["Value"], json!("hello"));
        assert!(!label.defaults.contains_key("plain"));
        assert!(label.property("Value").unwrap().target);
        assert!(label.has_event("Clicked"));
    }

    fn report_of(source: &str) -> super::Report {
        super::read_report(source, false).unwrap()
    }

    fn contract(found: &super::Found) -> (Vec<String>, Vec<String>, Vec<String>) {
        let widget = &found.widget;
        (
            widget
                .properties
                .iter()
                .flatten()
                .map(|(n, _)| n.clone())
                .collect(),
            widget.events.clone().unwrap_or_default(),
            widget.services.clone().unwrap_or_default(),
        )
    }

    #[test]
    fn a_first_statement_return_gives_properties_events_and_services() {
        let report = report_of(
            r#"
TW.IDE.Widgets.acmefirst = function () {
    this.widgetProperties = function () {
        return { name: 'Acme First', properties: { Value: { baseType: 'STRING', isBindingTarget: true, selectOptions: [{ value: 'a' }, { value: 'b' }] } } };
    };
    this.widgetEvents = function () { return { Changed: {} }; };
    this.widgetServices = function () { return { Reset: {} }; };
};
"#,
        );
        let found = &report.found[0];
        assert_eq!(found.unknown, None);
        assert_eq!(
            contract(found),
            (
                vec!["Value".into()],
                vec!["Changed".into()],
                vec!["Reset".into()]
            )
        );
        assert_eq!(
            found.widget.property("Value").unwrap().options,
            vec![json!("a"), json!("b")]
        );
    }

    #[test]
    fn the_last_return_counts_when_a_side_effect_runs_first() {
        let report = report_of(
            r#"
TW.IDE.Widgets.acmeside = function () {
    this.widgetProperties = function () {
        this.flag = TW.UTIL.check(this, ["10.1.0"]);
        if (this.flag) { return { name: 'early' }; }
        return { name: 'Acme Side', properties: { Size: { baseType: 'NUMBER', defaultValue: 4 } } };
    };
    this.widgetServices = function () { return this.x = 1, { Refresh: { warnIfNotBound: !1 } }; };
};
"#,
        );
        let found = &report.found[0];
        assert_eq!(found.unknown, None);
        assert_eq!(found.widget.type_display_name, "Acme Side");
        assert_eq!(found.widget.defaults["Size"], json!(4));
        assert_eq!(contract(found).2, vec!["Refresh".to_string()]);
    }

    #[test]
    fn a_name_bound_to_a_literal_stands_for_it() {
        let report = report_of(
            r#"
TW.IDE.Widgets.acmebound = function () {
    this.widgetProperties = function () {
        var props = { Title: { baseType: 'STRING', isBindingSource: true } };
        var all = { name: 'Acme Bound', properties: props };
        return all;
    };
    this.widgetEvents = function () { var e = { Picked: {} }; return e; };
};
"#,
        );
        let found = &report.found[0];
        assert_eq!(found.unknown, None);
        assert_eq!(contract(found).0, vec!["Title".to_string()]);
        assert_eq!(contract(found).1, vec!["Picked".to_string()]);
    }

    #[test]
    fn a_webpack_dev_mode_eval_module_is_read() {
        let source = r#"var modules = { "./src/acmeeval.ide.js": (function (module, exports) { eval("TW.IDE.Widgets.acmeeval = function () { this.widgetProperties = function () { return { name: 'Acme Eval', properties: { A: { baseType: 'BOOLEAN' } } }; }; };\n//# sourceURL=webpack:///./src/acmeeval.ide.js"); }) };"#;
        let report = report_of(source);
        assert_eq!(report.found.len(), 1);
        assert_eq!(report.found[0].widget.type_id, "acmeeval");
        assert_eq!(report.found[0].unknown, None);
        assert_eq!(contract(&report.found[0]).0, vec!["A".to_string()]);
    }

    #[test]
    fn translate_calls_are_their_key_and_other_expressions_are_noted() {
        let report = report_of(
            r#"
TW.IDE.Widgets.acmetext = function () {
    this.widgetProperties = function () {
        return {
            name: TW.IDE.I18NController.translate("acme.widget.name"),
            properties: {
                Label: { baseType: 'STRING', defaultValue: TW.IDE.I18NController.translate("acme.label") },
                On: { baseType: 'BOOLEAN', defaultValue: !0 },
                Computed: { baseType: 'STRING', defaultValue: TW.IDE.compute() }
            }
        };
    };
};
"#,
        );
        let found = &report.found[0];
        assert_eq!(found.widget.type_display_name, "acme.widget.name");
        assert_eq!(found.widget.defaults["Label"], json!("acme.label"));
        assert_eq!(found.widget.defaults["On"], json!(true));
        assert!(!found.widget.defaults.contains_key("Computed"));
        assert_eq!(found.notes, vec!["Computed: its default is not a literal"]);
        assert_eq!(found.unknown, None);
    }

    #[test]
    fn a_computed_or_decorated_widget_is_unknown_with_a_reason() {
        let report = report_of(
            r#"
TW.IDE.Widgets.acmecomputed = function () {
    this.widgetProperties = function () { return buildProperties(); };
};
TW.IDE.Widgets.acmeempty = function () { this.renderHtml = function () { return ''; }; };
TW.IDE.Widgets.acmedecorated = __decorate([TWWidgetDefinition('Acme'), AcmeDecorated]);
"#,
        );
        let computed = &report.found[0];
        assert_eq!(computed.widget.type_id, "acmecomputed");
        assert_eq!(
            computed.unknown.as_deref(),
            Some("widgetProperties returns something other than an object literal")
        );
        assert_eq!(
            report.found[1].unknown.as_deref(),
            Some("no widgetProperties function")
        );
        assert_eq!(report.unnamed.len(), 1);
        assert_eq!(report.unnamed[0].0, "acmedecorated");
    }
}
