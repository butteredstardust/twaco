//! The widgets a solution's own `[[extension]]` projects define, read from their mashup-builder
//! source, so `mashup add` can place them as Composer would.
//!
//! Two forms are read: a JavaScript `TW.IDE.Widgets.<type> = function () { this.widgetProperties
//! = function () { return {...} } }`, and a TypeScript `@TWWidgetDefinition` class in a file that
//! imports `widgetIDESupport`. Only literals count: a default worked out at run time is left out,
//! and Composer fills it when the mashup is next opened.

use super::config::Solution;
use super::widgets::{PropertyContract, Widget};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;
use swc_common::BytePos;
use swc_ecma_ast::{
    AssignExpr, AssignTarget, Callee, ClassDecl, ClassMember, Decorator, Expr, Function, Lit,
    MemberExpr, MemberProp, ObjectLit, Prop, PropName, PropOrSpread, SimpleAssignTarget, Stmt,
    UnaryOp,
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

/// Every widget the solution's extension projects define, and the source files that could not
/// be read.
pub(crate) fn load(solution: &Solution) -> (Vec<Widget>, Vec<String>) {
    let mut found = Vec::new();
    let mut skipped = Vec::new();
    for extension in &solution.extensions {
        let root = solution.root.join(&extension.root);
        let mut files = Vec::new();
        collect_sources(&root, &mut files);
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
    } else if !text.contains("TW.IDE.Widgets") {
        return Ok(Vec::new());
    }
    read_source(&text, typescript)
}

/// The widgets one IDE source file defines.
pub(crate) fn read_source(text: &str, typescript: bool) -> Result<Vec<Widget>, String> {
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
    let program = Parser::new_from(lexer)
        .parse_program()
        .map_err(|error| format!("cannot parse it: {}", error.kind().msg()))?;
    let mut collector = Collector::default();
    program.visit_with(&mut collector);
    Ok(collector.widgets)
}

#[derive(Default)]
struct Collector {
    widgets: Vec<Widget>,
}

impl Visit for Collector {
    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let Some(type_id) = ide_widget_name(&assign.left) {
            if let Expr::Fn(function) = &*assign.right {
                let mut parts = Parts::default();
                function.function.visit_with(&mut parts);
                self.widgets.push(parts.into_widget(type_id));
            }
        }
        assign.visit_children_with(self);
    }

    fn visit_class_decl(&mut self, class: &ClassDecl) {
        if let Some(widget) = typescript_widget(class) {
            self.widgets.push(widget);
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
        Expr::Paren(inner) => literal(&inner.expr),
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
}

#[derive(Default)]
struct Declared {
    base_type: Option<String>,
    default: Option<Value>,
    /// It names a default, literal or not.
    defaulted: bool,
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
        let Expr::Fn(function) = &*assign.right else {
            return;
        };
        let Some(returned) = returned_object(&function.function) else {
            return;
        };
        match name.as_str() {
            "widgetProperties" => self.read_properties(returned),
            "widgetEvents" => self.events.extend(pairs(returned).map(|(name, _)| name)),
            _ => {}
        }
    }
}

impl Parts {
    fn read_properties(&mut self, object: &ObjectLit) {
        for (name, value) in pairs(object) {
            match name.as_str() {
                "name" => self.label = string(value),
                "supportsAutoResize" => {
                    self.auto_resize = literal(value) == Some(Value::from(true))
                }
                "properties" => {
                    let Expr::Object(properties) = value else {
                        continue;
                    };
                    for (property, declaration) in pairs(properties) {
                        let Expr::Object(declaration) = declaration else {
                            continue;
                        };
                        let mut declared = Declared {
                            editable: true,
                            ..Declared::default()
                        };
                        for (field, value) in pairs(declaration) {
                            let flag = literal(value) == Some(Value::from(true));
                            match field.as_str() {
                                "baseType" => declared.base_type = string(value),
                                "defaultValue" => {
                                    declared.defaulted = true;
                                    declared.default = literal(value);
                                }
                                "isBindingSource" => declared.source = flag,
                                "isBindingTarget" => declared.target = flag,
                                "isEditable" => {
                                    declared.editable = literal(value) != Some(Value::from(false))
                                }
                                _ => {}
                            }
                        }
                        self.properties.push((property, declared));
                    }
                }
                _ => {}
            }
        }
    }

    fn into_widget(self, type_id: String) -> Widget {
        let mut defaults = Map::new();
        for (name, value) in dropped() {
            defaults.insert(name.to_string(), value);
        }
        let mut contracts = BTreeMap::new();
        for (name, declared) in self.properties {
            if let Some(value) = declared.default.clone() {
                defaults.insert(name.clone(), value);
            } else if let Some(empty) = (!declared.defaulted)
                .then_some(declared.base_type.as_deref())
                .flatten()
                .and_then(empty_value)
            {
                // Untyped otherwise: `--prop` reads its text by this value's JSON type, and
                // Composer writes the base type's empty value for a property with no default.
                defaults.insert(name.clone(), empty);
            }
            contracts.insert(
                name,
                PropertyContract {
                    base_type: declared.base_type,
                    source: declared.source,
                    target: declared.target,
                    editable: declared.editable,
                    options: Vec::new(),
                },
            );
        }
        Widget::extension(
            type_id.clone(),
            self.label.unwrap_or_else(|| type_id.clone()),
            self.auto_resize,
            defaults,
            contracts,
            self.events,
        )
    }
}

/// The empty value of a base type whose JSON form is a plain value.
fn empty_value(base_type: &str) -> Option<Value> {
    match base_type {
        "STRING" | "TEXT" | "HTML" | "HYPERLINK" | "IMAGELINK" | "MASHUPNAME" | "THINGNAME"
        | "SERVICENAME" | "PROPERTYNAME" | "STYLEDEFINITION" | "STATEDEFINITION"
        | "RENDERERWITHSTATE" | "THINGTEMPLATENAME" | "THINGSHAPENAME" | "USERNAME"
        | "GROUPNAME" | "MEDIAENTITYNAME" | "LOCALIZATIONTOKEN" => Some(Value::from("")),
        "NUMBER" | "INTEGER" | "LONG" => Some(Value::from(0)),
        "BOOLEAN" => Some(Value::from(false)),
        _ => None,
    }
}

/// The object literal a function returns: `function () { return {...}; }`.
fn returned_object(function: &Function) -> Option<&ObjectLit> {
    function.body.as_ref()?.stmts.iter().find_map(|statement| {
        let Stmt::Return(returned) = statement else {
            return None;
        };
        match returned.arg.as_deref()? {
            Expr::Object(object) => Some(object),
            Expr::Paren(inner) => match &*inner.expr {
                Expr::Object(object) => Some(object),
                _ => None,
            },
            _ => None,
        }
    })
}

/// The decorator's name and arguments: `@name` or `@name(arg, ...)`.
fn decorator(decorator: &Decorator) -> Option<(String, Vec<&Expr>)> {
    match &*decorator.expr {
        Expr::Ident(ident) => Some((ident.sym.to_string(), Vec::new())),
        Expr::Call(call) => {
            let Callee::Expr(callee) = &call.callee else {
                return None;
            };
            let Expr::Ident(ident) = &**callee else {
                return None;
            };
            Some((
                ident.sym.to_string(),
                call.args.iter().map(|arg| &*arg.expr).collect(),
            ))
        }
        _ => None,
    }
}

/// A `@TWWidgetDefinition` class: its name is the widget's type; `@property` members are its
/// properties and `@event` members its events.
fn typescript_widget(class: &ClassDecl) -> Option<Widget> {
    let type_id = class.ident.sym.to_string();
    let (_, arguments) = class
        .class
        .decorators
        .iter()
        .filter_map(decorator)
        .find(|(name, _)| name == "TWWidgetDefinition")?;
    let mut parts = Parts {
        label: arguments.first().and_then(|first| string(first)),
        auto_resize: arguments
            .iter()
            .any(|arg| matches!(arg, Expr::Ident(ident) if ident.sym.as_ref() == "autoResizable")),
        ..Parts::default()
    };
    for member in &class.class.body {
        let ClassMember::ClassProp(prop) = member else {
            continue;
        };
        let Some(name) = key(&prop.key) else {
            continue;
        };
        let decorators: Vec<_> = prop.decorators.iter().filter_map(decorator).collect();
        if decorators.iter().any(|(name, _)| name == "event") {
            parts.events.push(name);
            continue;
        }
        let Some((_, arguments)) = decorators.iter().find(|(name, _)| name == "property") else {
            continue;
        };
        let mut declared = Declared {
            editable: true,
            ..Declared::default()
        };
        for argument in arguments {
            match argument {
                Expr::Ident(ident) => match ident.sym.as_ref() {
                    "bindingSource" => declared.source = true,
                    "bindingTarget" => declared.target = true,
                    "nonEditable" => declared.editable = false,
                    _ => {}
                },
                Expr::Call(call) => {
                    let Callee::Expr(callee) = &call.callee else {
                        continue;
                    };
                    if matches!(&**callee, Expr::Ident(ident) if ident.sym.as_ref() == "defaultValue")
                    {
                        declared.defaulted = true;
                        declared.default = call.args.first().and_then(|arg| literal(&arg.expr));
                    }
                }
                other => {
                    if declared.base_type.is_none() {
                        declared.base_type = string(other);
                    }
                }
            }
        }
        parts.properties.push((name, declared));
    }
    Some(parts.into_widget(type_id))
}

#[cfg(test)]
mod tests {
    use super::read_source;
    use serde_json::json;

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
        assert_eq!(gauge.defaults["Unset"], json!(""));
        assert_eq!(gauge.defaults["Locked"], json!(false));
        assert!(!gauge.defaults.contains_key("Computed"));
        assert_eq!(gauge.defaults["Area"], json!("UI"));
        let value = gauge.property("Value").unwrap();
        assert!(value.target && !value.source);
        assert_eq!(value.base_type.as_deref(), Some("NUMBER"));
        let locked = gauge.property("Locked").unwrap();
        assert!(locked.source && !locked.editable);
        assert!(gauge.has_event("Changed"));
        assert_eq!(widgets[1].type_display_name, "other-one");
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
}
