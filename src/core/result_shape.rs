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
use std::collections::BTreeMap;
use swc_common::{BytePos, Spanned};
use swc_ecma_ast::{
    AssignExpr, AssignTarget, CallExpr, Callee, Expr, Lit, MemberProp, Pat, Prop, PropName,
    PropOrSpread, SimpleAssignTarget, VarDeclarator,
};
use swc_ecma_parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_ecma_visit::{Visit, VisitWith};

/// A result built from another DataShape than the one declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// The line of the `CreateInfoTableFromDataShape` call, from 1.
    pub line: usize,
    pub declared: String,
    pub built: String,
}

/// The mismatches of one service: its `definition.xml` and its `script.js`.
pub fn check(definition: &str, script: &str) -> Vec<Mismatch> {
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
}

struct Collector<'a> {
    src: &'a str,
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
        src: script,
        found: Found::default(),
    };
    ast.visit_with(&mut collector);
    Some(collector.found)
}

impl Collector<'_> {
    /// A string literal's text, from the source: DataShape names need no unescaping.
    fn string(&self, expr: &Expr) -> Option<String> {
        let Expr::Lit(Lit::Str(string)) = expr else {
            return None;
        };
        let span = string.span;
        let text = self.src.get(span.lo.0 as usize..span.hi.0 as usize)?;
        Some(text.get(1..text.len().checked_sub(1)?)?.to_string())
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
                PropName::Str(_) => self
                    .src
                    .get(pair.key.span().lo.0 as usize + 1..pair.key.span().hi.0 as usize - 1)?
                    .to_string(),
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

impl Visit for Collector<'_> {
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
