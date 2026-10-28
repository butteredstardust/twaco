//! A service whose script builds its result from one DataShape while its definition declares
//! another.
//!
//! `result = Resources["InfoTableFunctions"].CreateInfoTableFromDataShape({dataShapeName: "X"})`
//! returns rows of X whatever the definition says, and a mashup bound to the declared shape
//! finds its fields gone or renamed only at run time. In 391 real services that build their
//! result this way, 10 named a different DataShape than they declared, often one of the same
//! name in another project. Only a literal name is checked, directly or through a `const`
//! holding one; a name computed at run time says nothing.

use super::scan;
use std::collections::{BTreeMap, BTreeSet};
use swc_common::BytePos;
use swc_ecma_ast::{
    AssignExpr, AssignTarget, CallExpr, Callee, Expr, Lit, MemberProp, Pat, Prop, PropName,
    PropOrSpread, SimpleAssignTarget, VarDeclarator,
};
use swc_ecma_parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_ecma_visit::{Visit, VisitWith};

/// A result built from another DataShape than the one declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mismatch {
    /// The line of the `CreateInfoTableFromDataShape` call, from 1.
    pub(crate) line: usize,
    pub(crate) declared: String,
    pub(crate) built: String,
}

/// The mismatches of one service: its `definition.xml` and its `script.js`.
pub(crate) fn check(definition: &str, script: &str) -> Vec<Mismatch> {
    let Some(declared) = declared_shape(definition) else {
        return Vec::new();
    };
    let Some(found) = builds(script) else {
        return Vec::new();
    };
    let flows_to_result =
        |target: &str| target == "result" || found.into_result.iter().any(|name| name == target);
    found
        .builds
        .iter()
        .filter(|build| flows_to_result(&build.target))
        .filter_map(|build| {
            let built = match &build.shape {
                Shape::Literal(name) => name.clone(),
                Shape::Named(constant) => found.constants.get(constant)?.clone(),
            };
            (built != declared).then(|| Mismatch {
                line: line_of(script, build.at),
                declared: declared.clone(),
                built,
            })
        })
        .collect()
}

/// A row added to the result with a field its declared DataShape lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnknownField {
    /// The line of the `AddRow` call, from 1.
    pub(crate) line: usize,
    pub(crate) shape: String,
    pub(crate) field: String,
}

/// The `AddRow({...})` fields, on the infotable that becomes the result, that the declared
/// DataShape does not have. `fields_of` gives a shape's fields, or `None` when they cannot be
/// known (a shape outside the repository, or one deriving from another); then nothing is said.
pub(crate) fn unknown_fields(
    definition: &str,
    script: &str,
    fields_of: impl Fn(&str) -> Option<BTreeSet<String>>,
) -> Vec<UnknownField> {
    let Some(declared) = declared_shape(definition) else {
        return Vec::new();
    };
    let Some(fields) = fields_of(&declared) else {
        return Vec::new();
    };
    let Some(found) = builds(script) else {
        return Vec::new();
    };
    let flows_to_result =
        |target: &str| target == "result" || found.into_result.iter().any(|name| name == target);
    let mut out = Vec::new();
    for row in found.rows.iter().filter(|row| flows_to_result(&row.target)) {
        for field in &row.fields {
            if !fields.contains(field) {
                out.push(UnknownField {
                    line: line_of(script, row.at),
                    shape: declared.clone(),
                    field: field.clone(),
                });
            }
        }
    }
    out
}

/// The DataShape an INFOTABLE result declares; none for another type or no shape.
fn declared_shape(definition: &str) -> Option<String> {
    let src = definition.as_bytes();
    let tokens = scan::tokenize(src).ok()?;
    let result = scan::find_tag(&tokens, src, "ResultType", 0, tokens.len())?;
    let attribute = |name: &str| {
        scan::attribute(src, &tokens[result], name)
            .ok()
            .flatten()
            .map(|span| scan::decode_entities(&String::from_utf8_lossy(span.of(src))))
    };
    if attribute("baseType").as_deref() != Some("INFOTABLE") {
        return None;
    }
    attribute("aspect.dataShape").filter(|shape| !shape.is_empty())
}

enum Shape {
    Literal(String),
    /// A variable that should hold the name.
    Named(String),
}

struct Build {
    /// The variable the new infotable is put in.
    target: String,
    shape: Shape,
    at: u32,
}

#[derive(Default)]
struct Found {
    /// `const NAME = "literal"`, for every declaration with a string literal value.
    constants: BTreeMap<String, String>,
    builds: Vec<Build>,
    /// Variables assigned to `result` as they are: `result = rows;`.
    into_result: Vec<String>,
    /// `<target>.AddRow({...})` calls whose every key is a plain name.
    rows: Vec<Row>,
}

struct Row {
    target: String,
    fields: Vec<String>,
    at: u32,
}

struct Collector {
    found: Found,
}

fn builds(script: &str) -> Option<Found> {
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
    let ast = Parser::new_from(lexer).parse_script().ok()?;
    let mut collector = Collector {
        found: Found::default(),
    };
    ast.visit_with(&mut collector);
    Some(collector.found)
}

impl Collector {
    /// A string literal's value, its escapes decoded as the engine reads them.
    fn string(&self, expr: &Expr) -> Option<String> {
        let Expr::Lit(Lit::Str(string)) = expr else {
            return None;
        };
        Some(string.value.to_atom_lossy().to_string())
    }

    /// The shape a `CreateInfoTableFromDataShape({dataShapeName: ...})` call names.
    fn creates(&self, expr: &Expr) -> Option<(Shape, u32)> {
        let Expr::Call(CallExpr {
            callee: Callee::Expr(callee),
            args,
            span,
            ..
        }) = expr
        else {
            return None;
        };
        let Expr::Member(member) = &**callee else {
            return None;
        };
        let MemberProp::Ident(property) = &member.prop else {
            return None;
        };
        if property.sym.as_ref() != "CreateInfoTableFromDataShape" {
            return None;
        }
        let Expr::Object(object) = &*args.first()?.expr else {
            return None;
        };
        for property in &object.props {
            let PropOrSpread::Prop(prop) = property else {
                continue;
            };
            let Prop::KeyValue(pair) = &**prop else {
                continue;
            };
            let key = match &pair.key {
                PropName::Ident(ident) => ident.sym.to_string(),
                PropName::Str(key) => key.value.to_atom_lossy().to_string(),
                _ => continue,
            };
            if key != "dataShapeName" {
                continue;
            }
            if let Some(name) = self.string(&pair.value) {
                return Some((Shape::Literal(name), span.lo.0));
            }
            if let Expr::Ident(ident) = &*pair.value {
                return Some((Shape::Named(ident.sym.to_string()), span.lo.0));
            }
        }
        None
    }
}

impl Visit for Collector {
    fn visit_call_expr(&mut self, call: &CallExpr) {
        if let Callee::Expr(callee) = &call.callee {
            if let Expr::Member(member) = &**callee {
                let adds_row = matches!(&member.prop, MemberProp::Ident(name) if name.sym.as_ref() == "AddRow");
                if let (true, Expr::Ident(target), Some(first)) =
                    (adds_row, &*member.obj, call.args.first())
                {
                    if let (None, Expr::Object(object)) = (first.spread, &*first.expr) {
                        // A spread or a computed key could add any field: say nothing then.
                        let fields: Option<Vec<String>> = object
                            .props
                            .iter()
                            .map(|property| match property {
                                PropOrSpread::Prop(prop) => match &**prop {
                                    Prop::KeyValue(pair) => match &pair.key {
                                        PropName::Ident(ident) => Some(ident.sym.to_string()),
                                        PropName::Str(key) => {
                                            Some(key.value.to_atom_lossy().to_string())
                                        }
                                        _ => None,
                                    },
                                    Prop::Shorthand(ident) => Some(ident.sym.to_string()),
                                    _ => None,
                                },
                                PropOrSpread::Spread(_) => None,
                            })
                            .collect();
                        if let Some(fields) = fields {
                            self.found.rows.push(Row {
                                target: target.sym.to_string(),
                                fields,
                                at: call.span.lo.0,
                            });
                        }
                    }
                }
            }
        }
        call.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        if let (Pat::Ident(name), Some(init)) = (&declarator.name, &declarator.init) {
            let name = name.id.sym.to_string();
            if let Some(text) = self.string(init) {
                self.found.constants.insert(name.clone(), text);
            }
            if let Some((shape, at)) = self.creates(init) {
                self.found.builds.push(Build {
                    target: name,
                    shape,
                    at,
                });
            }
        }
        declarator.visit_children_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Ident(left)) = &assign.left {
            let left = left.id.sym.to_string();
            if let Some((shape, at)) = self.creates(&assign.right) {
                self.found.builds.push(Build {
                    target: left.clone(),
                    shape,
                    at,
                });
            }
            if left == "result" {
                if let Expr::Ident(right) = &*assign.right {
                    self.found.into_result.push(right.sym.to_string());
                }
            }
        }
        assign.visit_children_with(self);
    }
}

fn line_of(src: &str, at: u32) -> usize {
    let at = (at as usize).min(src.len());
    src[..at].matches('\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(shape: &str) -> String {
        format!(
            "<ServiceDefinition name=\"Get\"><ResultType aspect.dataShape=\"{shape}\" \
             baseType=\"INFOTABLE\" description=\"\" name=\"result\" ordinal=\"0\"></ResultType>\
             <ParameterDefinitions></ParameterDefinitions></ServiceDefinition>"
        )
    }

    const CREATE: &str = "Resources[\"InfoTableFunctions\"].CreateInfoTableFromDataShape";

    #[test]
    fn a_row_field_the_declared_shape_lacks_is_found() {
        let fields = |shape: &str| {
            (shape == "Acme.Rows_DS")
                .then(|| BTreeSet::from(["id".to_string(), "name".to_string()]))
        };
        let script = format!(
            "let rows = {CREATE}({{ dataShapeName: \"Acme.Rows_DS\" }});\n\
             rows.AddRow({{ id: 1, name: \"a\" }});\n\
             rows.AddRow({{ id: 2, qty: 3 }});\n\
             other.AddRow({{ nope: 1 }});\n\
             rows.AddRow({{ ...extra }});\n\
             result = rows;\n"
        );
        assert_eq!(
            unknown_fields(&definition("Acme.Rows_DS"), &script, fields),
            vec![UnknownField {
                line: 3,
                shape: "Acme.Rows_DS".into(),
                field: "qty".into(),
            }]
        );
        // Fields that cannot be known say nothing.
        assert!(unknown_fields(&definition("Acme.Elsewhere_DS"), &script, fields).is_empty());
    }

    #[test]
    fn a_result_built_from_another_shape_is_found() {
        let script =
            format!("const x = 1;\nresult = {CREATE}({{ dataShapeName: \"Acme.Other_DS\" }});\n");
        assert_eq!(
            check(&definition("Acme.Rows_DS"), &script),
            vec![Mismatch {
                line: 2,
                declared: "Acme.Rows_DS".into(),
                built: "Acme.Other_DS".into(),
            }]
        );
        assert!(check(&definition("Acme.Other_DS"), &script).is_empty());
    }

    #[test]
    fn a_constant_and_a_variable_put_into_result_count() {
        let script = format!(
            "const SHAPE = \"Acme.Other_DS\";\n\
             let rows = {CREATE}({{\n    dataShapeName: SHAPE\n}});\n\
             rows.AddRow({{ a: 1 }});\n\
             result = rows;\n"
        );
        let found = check(&definition("Acme.Rows_DS"), &script);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].line, found[0].built.as_str()),
            (2, "Acme.Other_DS")
        );
        // `var result = ...` is the same as assigning it.
        let declared = format!("var result = {CREATE}({{ 'dataShapeName': 'Acme.Other_DS' }});\n");
        assert_eq!(check(&definition("Acme.Rows_DS"), &declared).len(), 1);
    }

    #[test]
    fn an_escaped_name_is_read_as_the_engine_reads_it() {
        let script =
            format!("result = {CREATE}({{ \"data\\u0053hapeName\": \"Acme\\u002eRows_DS\" }});\n");
        assert!(
            check(&definition("Acme.Rows_DS"), &script).is_empty(),
            "{script}"
        );
        assert_eq!(
            check(&definition("Acme.Other_DS"), &script).len(),
            1,
            "{script}"
        );
    }

    #[test]
    fn what_cannot_be_known_is_not_reported() {
        let definition = definition("Acme.Rows_DS");
        // A name computed at run time, a table that never becomes the result, a parse error.
        for script in [
            format!("result = {CREATE}({{ dataShapeName: me.shapeName }});\n"),
            format!("result = {CREATE}({{ dataShapeName: shape }});\n"),
            format!("const helper = {CREATE}({{ dataShapeName: \"Acme.Other_DS\" }});\nresult = build();\n"),
            "result = (;\n".to_string(),
        ] {
            assert!(check(&definition, &script).is_empty(), "{script}");
        }
        // A result that is not an infotable, or one with no declared shape.
        let script = format!("result = {CREATE}({{ dataShapeName: \"Acme.Other_DS\" }});\n");
        let number = "<ServiceDefinition><ResultType baseType=\"NUMBER\" name=\"result\"></ResultType></ServiceDefinition>";
        let shapeless = "<ServiceDefinition><ResultType baseType=\"INFOTABLE\" name=\"result\"></ResultType></ServiceDefinition>";
        assert!(check(number, &script).is_empty());
        assert!(check(shapeless, &script).is_empty());
    }
}
