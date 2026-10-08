//! One service script parsed as ECMAScript, reduced to the facts a rename needs.
//!
//! The parser exists for its byte spans only. Nothing here prints JavaScript or rebuilds a
//! script: every edit built from these facts goes through [`splice`](super::splice), so the bytes
//! a rename does not mean to change stay exactly as they were.
//!
//! **It fails closed.** A script the parser rejects, or accepts only after recovering from an
//! error, is a [`ParseError`] and yields no facts; a recovered tree would answer questions about
//! code the author did not write. Rhino accepts a little that this parser does not (`for each`,
//! E4X, assignment to a call), so a refusal is an ordinary outcome that callers handle by leaving
//! the script for a person.
//!
//! Spans are byte offsets into the bytes given to [`parse`], and every span a fact reports lies
//! on UTF-8 boundaries. Text is never unescaped: a name written with an escape is simply not
//! equal to the name being renamed.

use super::scan;
use std::collections::{BTreeMap, BTreeSet};
use swc_common::comments::SingleThreadedComments;
use swc_common::{BytePos, Spanned};
use swc_ecma_ast::{
    ArrayLit, ArrayPat, ArrowExpr, AssignExpr, AssignTarget, AssignTargetPat, CatchClause, Expr,
    ExprOrSpread, FnDecl, FnExpr, ForHead, ForInStmt, ForOfStmt, Ident, Lit, MemberExpr,
    MemberProp, NewExpr, ObjectLit, ObjectPat, ObjectPatProp, OptCall, Param, Pat, Prop, PropName,
    SetterProp, SimpleAssignTarget, Str, UpdateExpr, VarDeclarator,
};
use swc_ecma_parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_ecma_visit::{Visit, VisitWith};

/// Why a script yielded no facts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The bytes are not UTF-8.
    #[error("the script is not UTF-8")]
    NotUtf8,
    /// The parser refused the script, at this byte offset.
    #[error("{message} (at byte {at})")]
    Syntax { at: usize, message: String },
}

/// How an identifier occurs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A use of the name as a value, or the target of an assignment.
    Reference,
    /// Declared by `var`, `let` or `const` (destructured or not), or as a function's own name.
    Declaration,
    /// Declared as a function, arrow or catch parameter.
    Parameter,
    /// `{ name }`, in an object literal or in a destructuring pattern.
    Shorthand,
    /// The key of `{ name: value }`.
    ObjectKey,
    /// The `name` of `x.name`.
    MemberProperty,
}

impl Role {
    /// Whether the occurrence introduces the name rather than using it.
    pub fn is_binding(self) -> bool {
        matches!(self, Role::Declaration | Role::Parameter)
    }
}

/// One occurrence of an identifier; the text is the span's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identifier {
    pub span: scan::Span,
    pub role: Role,
}

/// What stands before the `.` or `[` of a member access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Receiver {
    /// The identifier `me`.
    Me,
    /// `this`.
    This,
    /// `Things.X` or `Things["X"]`, holding the entity name as written.
    Thing(String),
    /// Any other plain identifier, holding its name.
    Variable(String),
    /// Anything else: a call result, a longer chain, a literal.
    Other,
}

/// `obj.name` or `obj["name"]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberAccess {
    /// The property name; for a string index, the bytes inside the quotes.
    pub property: scan::Span,
    /// The property is written as a string literal in brackets.
    pub string_index: bool,
    /// The access is the callee of a call or a `new` with arguments.
    pub is_callee: bool,
    pub receiver: Receiver,
}

/// A key of an object literal given as a call's first argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectKey {
    /// The key as written: an identifier's text, or a string key's text between its quotes.
    pub text: String,
    /// The identifier, or the bytes inside the quotes of a string key.
    pub span: scan::Span,
}

/// The first argument of a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstArgument {
    /// An object literal, with its `key: value` keys. A shorthand `{ name }` is not a key,
    /// because renaming it would rename the variable it reads, and a computed key or a spread
    /// names nothing that can be edited.
    Object(Vec<ObjectKey>),
    /// No argument, a spread, or any expression that is not an object literal.
    Other,
}

/// A call whose callee is a member access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The callee's property name, as for [`MemberAccess::property`].
    pub property: scan::Span,
    pub receiver: Receiver,
    pub first: FirstArgument,
}

/// A string literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringLiteral {
    /// The source text between the quotes, not unescaped.
    pub value: String,
    /// The bytes inside the quotes.
    pub span: scan::Span,
}

/// An object-literal property whose value is a string literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectString {
    /// The key as written: an identifier's text, or a string key's text between its quotes.
    pub key: String,
    /// The value's source text between the quotes, not unescaped.
    pub value: String,
    /// The bytes inside the value's quotes.
    pub value_span: scan::Span,
}

/// A key written in an object literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralKey {
    /// An identifier's text, a string key's text between its quotes, or a number key's text.
    pub text: String,
    /// The identifier, the bytes inside a string key's quotes, or the number literal.
    pub span: scan::Span,
    string: bool,
}

/// One property in an [`ObjectLiteral`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectProperty {
    /// A non-computed identifier, string or number key followed by a value.
    KeyValue { key: LiteralKey, value: Value },
    /// A shorthand, method, getter, setter, spread or computed key.
    Other,
}

/// A JavaScript object literal, held once in [`Script::objects`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectLiteral {
    /// The braces and their contents.
    pub span: scan::Span,
    pub properties: Vec<ObjectProperty>,
}

/// A value in an [`ObjectLiteral`] property or array literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// An object in [`Script::objects`].
    Object(usize),
    /// The elements of an array literal. A hole or spread is [`Value::Other`].
    Array(Vec<Value>),
    /// A string literal, with raw text between its quotes.
    String(StringLiteral),
    /// Any value that is not read structurally.
    Other(scan::Span),
}

/// A comment, delimiters included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub span: scan::Span,
    pub text: String,
}

/// The facts of one script.
#[derive(Debug, Clone, Default)]
pub struct Script {
    /// Every identifier occurrence, in source order, each reported once.
    pub identifiers: Vec<Identifier>,
    /// Every `obj.name` and `obj["name"]`, whether or not it is called.
    pub members: Vec<MemberAccess>,
    /// Every call whose callee is such a member.
    pub calls: Vec<Call>,
    /// Variable name to the entity of `Things.X`, for a variable that can only hold that Thing:
    /// every declaration of the name initialises it from the same `Things` entity, and it is
    /// never assigned again, redeclared differently, or taken as a parameter.
    pub thing_variables: BTreeMap<String, String>,
    pub object_strings: Vec<ObjectString>,
    /// Every object literal, in source order of its opening brace. Nested values refer to this
    /// arena by index so each literal is represented once.
    pub objects: Vec<ObjectLiteral>,
    /// Every string literal, including string keys and string indexes.
    pub strings: Vec<StringLiteral>,
    /// In source order.
    pub comments: Vec<Comment>,
}

impl Script {
    /// The `Things` entity a receiver stands for, if it provably stands for one.
    pub fn thing_of<'a>(&'a self, receiver: &'a Receiver) -> Option<&'a str> {
        match receiver {
            Receiver::Thing(entity) => Some(entity),
            Receiver::Variable(name) => self.thing_variables.get(name).map(String::as_str),
            _ => None,
        }
    }

    /// Checks that every span lies inside `src` on UTF-8 boundaries and covers the text its
    /// fact claims. The parser is trusted for spans; this is how a test, or a corpus run, proves
    /// that trust against the bytes it was given.
    pub fn verify_spans(&self, src: &[u8]) -> Result<(), String> {
        let text = std::str::from_utf8(src).map_err(|error| error.to_string())?;
        let slice = |span: scan::Span, what: &str| -> Result<&str, String> {
            if span.start > span.end
                || span.end > text.len()
                || !text.is_char_boundary(span.start)
                || !text.is_char_boundary(span.end)
            {
                return Err(format!(
                    "{what} span {}..{} is not a text range",
                    span.start, span.end
                ));
            }
            Ok(&text[span.start..span.end])
        };
        let quoted = |span: scan::Span, what: &str| -> Result<(), String> {
            let quote = span
                .start
                .checked_sub(1)
                .and_then(|at| src.get(at))
                .copied();
            match quote {
                Some(b'\'' | b'"') if src.get(span.end) == quote.as_ref() => Ok(()),
                _ => Err(format!(
                    "{what} span {}..{} is not inside quotes",
                    span.start, span.end
                )),
            }
        };
        for identifier in &self.identifiers {
            if slice(identifier.span, "identifier")?.is_empty() {
                return Err(format!("empty identifier at {}", identifier.span.start));
            }
        }
        for member in &self.members {
            let property = slice(member.property, "member")?;
            if member.string_index {
                quoted(member.property, "string index")?;
            } else if property.is_empty() {
                return Err(format!("empty member name at {}", member.property.start));
            }
        }
        for call in &self.calls {
            slice(call.property, "call")?;
            if let FirstArgument::Object(keys) = &call.first {
                for key in keys {
                    if slice(key.span, "object key")? != key.text {
                        return Err(format!("object key text differs at {}", key.span.start));
                    }
                }
            }
        }
        for string in &self.strings {
            if slice(string.span, "string")? != string.value {
                return Err(format!("string text differs at {}", string.span.start));
            }
            quoted(string.span, "string")?;
        }
        for property in &self.object_strings {
            if slice(property.value_span, "object string")? != property.value {
                return Err(format!(
                    "object string differs at {}",
                    property.value_span.start
                ));
            }
            quoted(property.value_span, "object string")?;
        }
        for object in &self.objects {
            let raw = slice(object.span, "object")?;
            if !(raw.starts_with('{') && raw.ends_with('}')) {
                return Err(format!(
                    "object span {}..{} is not an object",
                    object.span.start, object.span.end
                ));
            }
            for property in &object.properties {
                if let ObjectProperty::KeyValue {
                    key,
                    value: property_value,
                } = property
                {
                    if slice(key.span, "object literal key")? != key.text {
                        return Err(format!(
                            "object literal key text differs at {}",
                            key.span.start
                        ));
                    }
                    if key.string {
                        quoted(key.span, "object literal string key")?;
                    }
                    verify_object_value(property_value, &self.objects, src, text)?;
                }
            }
        }
        for comment in &self.comments {
            let found = slice(comment.span, "comment")?;
            if found != comment.text || !(found.starts_with("//") || found.starts_with("/*")) {
                return Err(format!("comment text differs at {}", comment.span.start));
            }
        }
        Ok(())
    }
}

fn verify_object_value(
    value: &Value,
    objects: &[ObjectLiteral],
    src: &[u8],
    text: &str,
) -> Result<(), String> {
    let slice = |span: scan::Span, what: &str| -> Result<&str, String> {
        if span.start > span.end
            || span.end > text.len()
            || !text.is_char_boundary(span.start)
            || !text.is_char_boundary(span.end)
        {
            return Err(format!(
                "{what} span {}..{} is not a text range",
                span.start, span.end
            ));
        }
        Ok(&text[span.start..span.end])
    };
    let quoted = |span: scan::Span, what: &str| -> Result<(), String> {
        let quote = span
            .start
            .checked_sub(1)
            .and_then(|at| src.get(at))
            .copied();
        match quote {
            Some(b'\'' | b'"') if src.get(span.end) == quote.as_ref() => Ok(()),
            _ => Err(format!(
                "{what} span {}..{} is not inside quotes",
                span.start, span.end
            )),
        }
    };
    match value {
        Value::Object(index) => {
            if *index >= objects.len() {
                return Err(format!("object index {index} is out of bounds"));
            }
        }
        Value::Array(values) => {
            for value in values {
                verify_object_value(value, objects, src, text)?;
            }
        }
        Value::String(string) => {
            if slice(string.span, "object value string")? != string.value {
                return Err(format!(
                    "object value string differs at {}",
                    string.span.start
                ));
            }
            quoted(string.span, "object value string")?;
        }
        Value::Other(span) => {
            slice(*span, "object value")?;
        }
    }
    Ok(())
}

/// Parses `src` as a script, not a module. A `return` outside a function is allowed, since a
/// ThingWorx service body is one.
pub fn parse(src: &[u8]) -> Result<Script, ParseError> {
    let text = std::str::from_utf8(src).map_err(|_| ParseError::NotUtf8)?;
    let end = u32::try_from(src.len()).map_err(|_| ParseError::Syntax {
        at: 0,
        message: "the script is too large to parse".to_string(),
    })?;
    let comments = SingleThreadedComments::default();
    let ast = {
        let lexer = Lexer::new(
            Syntax::Es(EsSyntax {
                allow_return_outside_function: true,
                ..Default::default()
            }),
            Default::default(),
            StringInput::new(text, BytePos(0), BytePos(end)),
            Some(&comments),
        );
        let mut parser = Parser::new_from(lexer);
        let ast = parser.parse_script().map_err(syntax_error)?;
        if let Some(error) = parser.take_errors().into_iter().next() {
            return Err(syntax_error(error));
        }
        ast
    };
    let mut object_collector = ObjectCollector::default();
    ast.visit_with(&mut object_collector);
    object_collector.objects.sort_by_key(|span| span.lo);
    let mut builder = Builder::new(src, object_collector.objects);
    ast.visit_with(&mut builder);
    Ok(builder.finish(comments))
}

fn syntax_error(error: swc_ecma_parser::error::Error) -> ParseError {
    ParseError::Syntax {
        at: error.span().lo.0 as usize,
        message: error.kind().msg().into_owned(),
    }
}

/// How a pattern introduces or uses the names in it.
#[derive(Debug, Clone, Copy)]
enum Binds {
    Declaration,
    Parameter,
    Assignment,
}

struct Builder<'a> {
    src: &'a [u8],
    script: Script,
    /// Where each identifier already has a role, so one that is classified before the generic
    /// walk reaches it is not reported again as a plain reference.
    reported: BTreeSet<usize>,
    /// Every way each name is introduced: the entity it is initialised from, or `None` for any
    /// other kind of declaration or for a parameter.
    bindings: BTreeMap<String, Vec<Option<String>>>,
    assigned: BTreeSet<String>,
    object_indexes: BTreeMap<usize, usize>,
}

#[derive(Default)]
struct ObjectCollector {
    objects: Vec<swc_common::Span>,
}

impl Visit for ObjectCollector {
    fn visit_object_lit(&mut self, object: &ObjectLit) {
        self.objects.push(object.span);
        object.visit_children_with(self);
    }
}

impl<'a> Builder<'a> {
    fn new(src: &'a [u8], objects: Vec<swc_common::Span>) -> Self {
        let object_indexes = objects
            .iter()
            .enumerate()
            .map(|(index, span)| (span.lo.0 as usize, index))
            .collect();
        Builder {
            src,
            script: Script {
                objects: objects
                    .into_iter()
                    .map(|span| ObjectLiteral {
                        span: scan::Span::new(span.lo.0 as usize, span.hi.0 as usize),
                        properties: Vec::new(),
                    })
                    .collect(),
                ..Default::default()
            },
            reported: BTreeSet::new(),
            bindings: BTreeMap::new(),
            assigned: BTreeSet::new(),
            object_indexes,
        }
    }

    fn finish(mut self, comments: SingleThreadedComments) -> Script {
        let (leading, trailing) = comments.take_all();
        let mut found = BTreeMap::new();
        for map in [leading, trailing] {
            for list in map.borrow().values() {
                for comment in list {
                    let span = self.span(comment.span);
                    found.insert(span.start, span);
                }
            }
        }
        self.script.comments = found
            .into_values()
            .map(|span| Comment {
                span,
                text: self.text(span),
            })
            .collect();
        self.script
            .identifiers
            .sort_by_key(|identifier| identifier.span.start);
        for (name, declarations) in &self.bindings {
            let Some(Some(entity)) = declarations.first() else {
                continue;
            };
            let same = declarations
                .iter()
                .all(|other| other.as_ref() == Some(entity));
            if same && !self.assigned.contains(name) {
                self.script
                    .thing_variables
                    .insert(name.clone(), entity.clone());
            }
        }
        self.script
    }

    fn span(&self, span: swc_common::Span) -> scan::Span {
        scan::Span::new(span.lo.0 as usize, span.hi.0 as usize)
    }

    fn text(&self, span: scan::Span) -> String {
        String::from_utf8_lossy(span.of(self.src)).into_owned()
    }

    /// The bytes inside the quotes of a string literal's span, or `None` for a span that is not
    /// quoted source text.
    fn inner(&self, span: swc_common::Span) -> Option<scan::Span> {
        let span = self.span(span);
        let raw = span.of(self.src);
        let quote = *raw.first()?;
        let quoted = raw.len() >= 2 && matches!(quote, b'\'' | b'"') && raw.last() == Some(&quote);
        quoted.then(|| scan::Span::new(span.start + 1, span.end - 1))
    }

    fn identifier(&mut self, span: swc_common::Span, role: Role) {
        let span = self.span(span);
        if self.reported.insert(span.start) {
            self.script.identifiers.push(Identifier { span, role });
        }
    }

    fn strip_parens(expr: &Expr) -> &Expr {
        match expr {
            Expr::Paren(paren) => Self::strip_parens(&paren.expr),
            other => other,
        }
    }

    /// The entity of `Things.X` or `Things["X"]`.
    fn thing_entity(&self, expr: &Expr) -> Option<String> {
        let Expr::Member(member) = Self::strip_parens(expr) else {
            return None;
        };
        let Expr::Ident(object) = &*member.obj else {
            return None;
        };
        if self.text(self.span(object.span)) != "Things" {
            return None;
        }
        let (property, _) = self.property_of(member)?;
        Some(self.text(property))
    }

    fn receiver(&self, expr: &Expr) -> Receiver {
        match Self::strip_parens(expr) {
            Expr::This(_) => Receiver::This,
            Expr::Ident(ident) => match self.text(self.span(ident.span)) {
                name if name == "me" => Receiver::Me,
                name => Receiver::Variable(name),
            },
            other => self
                .thing_entity(other)
                .map_or(Receiver::Other, Receiver::Thing),
        }
    }

    /// The property name of a member access and whether it is a string index. `None` for a
    /// computed name that is not a string literal, and for a private name.
    fn property_of(&self, member: &MemberExpr) -> Option<(scan::Span, bool)> {
        match &member.prop {
            MemberProp::Ident(name) => Some((self.span(name.span), false)),
            MemberProp::Computed(computed) => match &*computed.expr {
                Expr::Lit(Lit::Str(string)) => Some((self.inner(string.span)?, true)),
                _ => None,
            },
            MemberProp::PrivateName(_) => None,
        }
    }

    fn member(&mut self, member: &MemberExpr, is_callee: bool) -> Option<(scan::Span, Receiver)> {
        if let MemberProp::Ident(name) = &member.prop {
            self.identifier(name.span, Role::MemberProperty);
        }
        let (property, string_index) = self.property_of(member)?;
        let receiver = self.receiver(&member.obj);
        self.script.members.push(MemberAccess {
            property,
            string_index,
            is_callee,
            receiver: receiver.clone(),
        });
        Some((property, receiver))
    }

    /// What a member access evaluates besides naming its property.
    fn visit_operands(&mut self, member: &MemberExpr) {
        member.obj.visit_with(self);
        if let MemberProp::Computed(computed) = &member.prop {
            computed.visit_with(self);
        }
    }

    fn call(&mut self, callee: &Expr, args: &[ExprOrSpread]) {
        match Self::strip_parens(callee) {
            Expr::Member(member) => {
                if let Some((property, receiver)) = self.member(member, true) {
                    let first = self.first_argument(args);
                    self.script.calls.push(Call {
                        property,
                        receiver,
                        first,
                    });
                }
                self.visit_operands(member);
            }
            other => other.visit_with(self),
        }
        for arg in args {
            arg.visit_with(self);
        }
    }

    fn first_argument(&self, args: &[ExprOrSpread]) -> FirstArgument {
        match args.first() {
            Some(arg) if arg.spread.is_none() => match &*arg.expr {
                Expr::Object(object) => FirstArgument::Object(
                    object
                        .props
                        .iter()
                        .filter_map(|prop| prop.as_prop())
                        .filter_map(|prop| match &**prop {
                            Prop::KeyValue(pair) => self.key(&pair.key),
                            _ => None,
                        })
                        .collect(),
                ),
                _ => FirstArgument::Other,
            },
            _ => FirstArgument::Other,
        }
    }

    /// An identifier or string key. A number or computed key is not one.
    fn key(&self, name: &PropName) -> Option<ObjectKey> {
        let span = match name {
            PropName::Ident(ident) => self.span(ident.span),
            PropName::Str(string) => self.inner(string.span)?,
            _ => return None,
        };
        Some(ObjectKey {
            text: self.text(span),
            span,
        })
    }

    fn literal_key(&self, name: &PropName) -> Option<LiteralKey> {
        let (span, string) = match name {
            PropName::Ident(ident) => (self.span(ident.span), false),
            PropName::Str(string) => (self.inner(string.span)?, true),
            PropName::Num(number) => (self.span(number.span), false),
            _ => return None,
        };
        Some(LiteralKey {
            text: self.text(span),
            span,
            string,
        })
    }

    fn value(&self, expression: &Expr) -> Value {
        match Self::strip_parens(expression) {
            Expr::Object(object) => self
                .object_indexes
                .get(&(object.span.lo.0 as usize))
                .copied()
                .map_or_else(|| Value::Other(self.span(object.span)), Value::Object),
            Expr::Array(array) => self.array_value(array),
            Expr::Lit(Lit::Str(string)) => self.inner(string.span).map_or_else(
                || Value::Other(self.span(string.span)),
                |span| {
                    Value::String(StringLiteral {
                        value: self.text(span),
                        span,
                    })
                },
            ),
            other => Value::Other(self.span(other.span())),
        }
    }

    fn array_value(&self, array: &ArrayLit) -> Value {
        Value::Array(
            array
                .elems
                .iter()
                .map(|element| match element {
                    Some(element) if element.spread.is_none() => self.value(&element.expr),
                    Some(element) => Value::Other(self.span(element.expr.span())),
                    None => Value::Other(self.span(array.span)),
                })
                .collect(),
        )
    }

    fn object(&mut self, object: &ObjectLit) {
        let Some(index) = self
            .object_indexes
            .get(&(object.span.lo.0 as usize))
            .copied()
        else {
            return;
        };
        self.script.objects[index].properties = object
            .props
            .iter()
            .map(|property| {
                match property.as_prop().and_then(|property| match &**property {
                    Prop::KeyValue(pair) => {
                        self.literal_key(&pair.key).map(|key| (key, &*pair.value))
                    }
                    _ => None,
                }) {
                    Some((key, value)) => ObjectProperty::KeyValue {
                        key,
                        value: self.value(value),
                    },
                    None => ObjectProperty::Other,
                }
            })
            .collect();
    }

    /// Introduces or assigns one name, according to how the surrounding pattern uses it.
    fn name(&mut self, ident: &Ident, binds: Binds, entity: Option<String>) {
        let name = self.text(self.span(ident.span));
        let role = match binds {
            Binds::Declaration => Role::Declaration,
            Binds::Parameter => Role::Parameter,
            Binds::Assignment => Role::Reference,
        };
        self.identifier(ident.span, role);
        match binds {
            Binds::Assignment => {
                self.assigned.insert(name);
            }
            Binds::Declaration | Binds::Parameter => {
                self.bindings.entry(name).or_default().push(entity);
            }
        }
    }

    /// Walks a pattern. `entity` is the `Things` entity a plain `var name = ...` is initialised
    /// from; a destructured name never has one.
    fn pattern(&mut self, pat: &Pat, binds: Binds, entity: Option<String>) {
        match pat {
            Pat::Ident(binding) => self.name(&binding.id, binds, entity),
            Pat::Array(array) => self.array_pattern(array, binds),
            Pat::Object(object) => self.object_pattern(object, binds),
            Pat::Rest(rest) => self.pattern(&rest.arg, binds, None),
            Pat::Assign(assign) => {
                self.pattern(&assign.left, binds, None);
                assign.right.visit_with(self);
            }
            Pat::Expr(expr) => match (&**expr, binds) {
                (Expr::Ident(ident), Binds::Assignment) => self.name(ident, binds, None),
                _ => expr.visit_with(self),
            },
            Pat::Invalid(_) => {}
        }
    }

    fn array_pattern(&mut self, array: &ArrayPat, binds: Binds) {
        for element in array.elems.iter().flatten() {
            self.pattern(element, binds, None);
        }
    }

    fn object_pattern(&mut self, object: &ObjectPat, binds: Binds) {
        for prop in &object.props {
            match prop {
                ObjectPatProp::KeyValue(pair) => {
                    pair.key.visit_with(self);
                    self.pattern(&pair.value, binds, None);
                }
                ObjectPatProp::Assign(assign) => {
                    self.shorthand(&assign.key.id, binds);
                    if let Some(value) = &assign.value {
                        value.visit_with(self);
                    }
                }
                ObjectPatProp::Rest(rest) => self.pattern(&rest.arg, binds, None),
            }
        }
    }

    /// `{ name }` in a pattern: the name is introduced or assigned as in any pattern, but
    /// renaming it would also rename the property it reads.
    fn shorthand(&mut self, ident: &Ident, binds: Binds) {
        self.identifier(ident.span, Role::Shorthand);
        let name = self.text(self.span(ident.span));
        match binds {
            Binds::Assignment => {
                self.assigned.insert(name);
            }
            Binds::Declaration | Binds::Parameter => {
                self.bindings.entry(name).or_default().push(None);
            }
        }
    }

    fn for_head(&mut self, head: &ForHead) {
        match head {
            ForHead::Pat(pat) => self.pattern(pat, Binds::Assignment, None),
            other => other.visit_with(self),
        }
    }
}

impl Visit for Builder<'_> {
    fn visit_object_lit(&mut self, object: &ObjectLit) {
        self.object(object);
        object.visit_children_with(self);
    }

    fn visit_ident(&mut self, ident: &Ident) {
        self.identifier(ident.span, Role::Reference);
    }

    fn visit_member_expr(&mut self, member: &MemberExpr) {
        self.member(member, false);
        self.visit_operands(member);
    }

    fn visit_call_expr(&mut self, call: &swc_ecma_ast::CallExpr) {
        match &call.callee {
            swc_ecma_ast::Callee::Expr(callee) => self.call(callee, &call.args),
            other => {
                other.visit_with(self);
                for arg in &call.args {
                    arg.visit_with(self);
                }
            }
        }
    }

    fn visit_opt_call(&mut self, call: &OptCall) {
        self.call(&call.callee, &call.args);
    }

    fn visit_new_expr(&mut self, new: &NewExpr) {
        match &new.args {
            Some(args) => self.call(&new.callee, args),
            None => new.visit_children_with(self),
        }
    }

    fn visit_prop_name(&mut self, name: &PropName) {
        match name {
            PropName::Ident(ident) => self.identifier(ident.span, Role::ObjectKey),
            other => other.visit_children_with(self),
        }
    }

    fn visit_prop(&mut self, prop: &Prop) {
        match prop {
            Prop::Shorthand(ident) => self.identifier(ident.span, Role::Shorthand),
            Prop::KeyValue(pair) => {
                if let (Some(key), Expr::Lit(Lit::Str(value))) = (self.key(&pair.key), &*pair.value)
                {
                    if let Some(value_span) = self.inner(value.span) {
                        self.script.object_strings.push(ObjectString {
                            key: key.text,
                            value: self.text(value_span),
                            value_span,
                        });
                    }
                }
                prop.visit_children_with(self);
            }
            other => other.visit_children_with(self),
        }
    }

    fn visit_str(&mut self, string: &Str) {
        if let Some(span) = self.inner(string.span) {
            self.script.strings.push(StringLiteral {
                value: self.text(span),
                span,
            });
        }
    }

    fn visit_var_declarator(&mut self, declarator: &VarDeclarator) {
        let entity = declarator
            .init
            .as_deref()
            .and_then(|init| self.thing_entity(init));
        self.pattern(&declarator.name, Binds::Declaration, entity);
        declarator.init.visit_with(self);
    }

    fn visit_fn_decl(&mut self, declaration: &FnDecl) {
        self.name(&declaration.ident, Binds::Declaration, None);
        declaration.function.visit_with(self);
    }

    fn visit_fn_expr(&mut self, expression: &FnExpr) {
        if let Some(ident) = &expression.ident {
            self.name(ident, Binds::Declaration, None);
        }
        expression.function.visit_with(self);
    }

    fn visit_param(&mut self, param: &Param) {
        self.pattern(&param.pat, Binds::Parameter, None);
    }

    fn visit_arrow_expr(&mut self, arrow: &ArrowExpr) {
        for param in &arrow.params {
            self.pattern(param, Binds::Parameter, None);
        }
        arrow.body.visit_with(self);
    }

    fn visit_setter_prop(&mut self, setter: &SetterProp) {
        setter.key.visit_with(self);
        self.pattern(&setter.param, Binds::Parameter, None);
        setter.body.visit_with(self);
    }

    fn visit_catch_clause(&mut self, clause: &CatchClause) {
        if let Some(param) = &clause.param {
            self.pattern(param, Binds::Parameter, None);
        }
        clause.body.visit_with(self);
    }

    fn visit_assign_expr(&mut self, assign: &AssignExpr) {
        match &assign.left {
            AssignTarget::Simple(SimpleAssignTarget::Ident(binding)) => {
                self.name(&binding.id, Binds::Assignment, None)
            }
            AssignTarget::Pat(AssignTargetPat::Array(array)) => {
                self.array_pattern(array, Binds::Assignment)
            }
            AssignTarget::Pat(AssignTargetPat::Object(object)) => {
                self.object_pattern(object, Binds::Assignment)
            }
            other => other.visit_with(self),
        }
        assign.right.visit_with(self);
    }

    fn visit_update_expr(&mut self, update: &UpdateExpr) {
        if let Expr::Ident(ident) = Self::strip_parens(&update.arg) {
            let name = self.text(self.span(ident.span));
            self.assigned.insert(name);
        }
        update.visit_children_with(self);
    }

    fn visit_for_in_stmt(&mut self, statement: &ForInStmt) {
        self.for_head(&statement.left);
        statement.right.visit_with(self);
        statement.body.visit_with(self);
    }

    fn visit_for_of_stmt(&mut self, statement: &ForOfStmt) {
        self.for_head(&statement.left);
        statement.right.visit_with(self);
        statement.body.visit_with(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `src`, proves every span against the bytes, and returns the facts.
    fn facts(src: &str) -> Script {
        let script = parse(src.as_bytes()).unwrap_or_else(|error| panic!("{src}: {error}"));
        script
            .verify_spans(src.as_bytes())
            .unwrap_or_else(|error| panic!("{src}: {error}"));
        script
    }

    fn text(src: &str, span: scan::Span) -> &str {
        &src[span.start..span.end]
    }

    /// The text of every identifier with this role, in source order.
    fn with_role<'a>(src: &'a str, script: &Script, role: Role) -> Vec<&'a str> {
        script
            .identifiers
            .iter()
            .filter(|identifier| identifier.role == role)
            .map(|identifier| text(src, identifier.span))
            .collect()
    }

    /// The one member access whose property is written `name`.
    fn member<'a>(src: &str, script: &'a Script, name: &str) -> &'a MemberAccess {
        let mut found = script
            .members
            .iter()
            .filter(|member| text(src, member.property) == name);
        let member = found
            .next()
            .unwrap_or_else(|| panic!("no member {name} in {src}"));
        assert!(found.next().is_none(), "two members named {name} in {src}");
        member
    }

    fn thing(entity: &str) -> Receiver {
        Receiver::Thing(entity.to_string())
    }

    fn variable(name: &str) -> Receiver {
        Receiver::Variable(name.to_string())
    }

    fn keys(call: &Call) -> Vec<&str> {
        match &call.first {
            FirstArgument::Object(keys) => keys.iter().map(|key| key.text.as_str()).collect(),
            FirstArgument::Other => panic!("the first argument is not an object literal"),
        }
    }

    #[test]
    fn spans_are_byte_offsets_after_a_multi_byte_character() {
        // `é` is two bytes and `日本` six, so a character offset would land inside them.
        let src = "var s = 'é日本'; return me.old();";
        let script = facts(src);
        let property = script.members[0].property;
        assert_eq!(text(src, property), "old");
        assert_eq!(property.start, src.find("old").unwrap());
        assert_eq!(script.strings[0].value, "é日本");
        assert_eq!(text(src, script.strings[0].span), "é日本");
    }

    #[test]
    fn a_top_level_return_is_part_of_a_script() {
        assert!(parse(b"if (x) { return 1; } return 2;").is_ok());
    }

    #[test]
    fn the_parser_refuses_what_it_cannot_account_for() {
        for rhino_only in [
            "for each (x in y) {}",
            "delete a.@b;",
            "text.charAt(0) = 'a';",
        ] {
            let error = parse(rhino_only.as_bytes()).unwrap_err();
            assert!(
                matches!(error, ParseError::Syntax { .. }),
                "{rhino_only}: {error:?}"
            );
        }
        assert!(matches!(
            parse(b"var a = ;"),
            Err(ParseError::Syntax { .. })
        ));
        assert_eq!(parse(&[b'a', 0xff]).unwrap_err(), ParseError::NotUtf8);
    }

    #[test]
    fn a_syntax_error_says_where_in_bytes() {
        let src = "var é = 1;\nvar a = ;";
        let ParseError::Syntax { at, .. } = parse(src.as_bytes()).unwrap_err() else {
            panic!("expected a syntax error");
        };
        assert_eq!(at, src.rfind(';').unwrap());
    }

    #[test]
    fn identifiers_are_reported_once_with_their_role() {
        let src = "var a = b; function f(p, { q, r: s }) { return p + t; }\n\
                   try {} catch (e) {} var o = { k: 1, shorthand }; x.prop = (u) => u;";
        let script = facts(src);
        assert_eq!(with_role(src, &script, Role::Declaration), ["a", "f", "o"]);
        assert_eq!(
            with_role(src, &script, Role::Parameter),
            ["p", "s", "e", "u"]
        );
        assert_eq!(with_role(src, &script, Role::Shorthand), ["q", "shorthand"]);
        assert_eq!(with_role(src, &script, Role::ObjectKey), ["r", "k"]);
        assert_eq!(with_role(src, &script, Role::MemberProperty), ["prop"]);
        assert_eq!(
            with_role(src, &script, Role::Reference),
            ["b", "p", "t", "x", "u"]
        );
        let starts: Vec<usize> = script
            .identifiers
            .iter()
            .map(|identifier| identifier.span.start)
            .collect();
        assert!(
            starts.windows(2).all(|pair| pair[0] < pair[1]),
            "source order, no repeats"
        );
    }

    #[test]
    fn a_destructured_declaration_binds_its_names() {
        let src = "var { a, b: c, d: [e] } = x; let [f, ...g] = y;";
        let script = facts(src);
        assert_eq!(
            with_role(src, &script, Role::Declaration),
            ["c", "e", "f", "g"]
        );
        assert_eq!(with_role(src, &script, Role::Shorthand), ["a"]);
        assert_eq!(with_role(src, &script, Role::ObjectKey), ["b", "d"]);
    }

    #[test]
    fn an_assignment_pattern_reads_its_names_but_a_shorthand_stays_a_shorthand() {
        let src = "var a, b; [a] = x; ({ b } = y);";
        let script = facts(src);
        assert_eq!(with_role(src, &script, Role::Reference), ["a", "x", "y"]);
        assert_eq!(with_role(src, &script, Role::Shorthand), ["b"]);
    }

    #[test]
    fn a_function_expression_getter_setter_and_default_bind_their_names() {
        let src = "var o = { get g() { return 1; }, set s(v) {}, m(w = d) {} };\n\
                   var f = function named(z) {};";
        let script = facts(src);
        assert_eq!(with_role(src, &script, Role::Parameter), ["v", "w", "z"]);
        assert_eq!(
            with_role(src, &script, Role::Declaration),
            ["o", "f", "named"]
        );
        assert_eq!(with_role(src, &script, Role::Reference), ["d"]);
    }

    #[test]
    fn a_regex_a_comment_and_a_division_chain_hold_no_member_access() {
        let src = "var r = /me.old(/g; // me.old(\nvar n = a / b / c;";
        let script = facts(src);
        assert!(script.members.is_empty());
        assert_eq!(with_role(src, &script, Role::Reference), ["a", "b", "c"]);
    }

    #[test]
    fn members_report_the_property_span_and_the_receiver() {
        let src = "me.a; this.b; Things.X.c; Things[\"Y\"].d; v.e; w().f; q.p.g;\n\
                   me[\"h\"]; me[i]; (me).j;";
        let script = facts(src);
        assert_eq!(member(src, &script, "a").receiver, Receiver::Me);
        assert_eq!(member(src, &script, "b").receiver, Receiver::This);
        assert_eq!(member(src, &script, "c").receiver, thing("X"));
        assert_eq!(member(src, &script, "d").receiver, thing("Y"));
        assert_eq!(member(src, &script, "e").receiver, variable("v"));
        assert_eq!(member(src, &script, "f").receiver, Receiver::Other);
        assert_eq!(member(src, &script, "g").receiver, Receiver::Other);
        assert_eq!(member(src, &script, "h").receiver, Receiver::Me);
        assert_eq!(member(src, &script, "j").receiver, Receiver::Me);
        assert!(member(src, &script, "h").string_index);
        assert!(!member(src, &script, "a").string_index);
        // `Things.X` is itself an access, on the identifier `Things`.
        assert_eq!(member(src, &script, "X").receiver, variable("Things"));
        // A computed name that is not a string literal names no property.
        assert!(script
            .members
            .iter()
            .all(|member| text(src, member.property) != "i"));
    }

    #[test]
    fn whitespace_and_newlines_between_the_parts_do_not_matter() {
        let src = "Things [ \"A\" ]\n  . old\n  ( );\nThings\n.B\n[ 'old' ]();";
        let script = facts(src);
        assert_eq!(script.calls.len(), 2);
        assert_eq!(script.calls[0].receiver, thing("A"));
        assert_eq!(text(src, script.calls[0].property), "old");
        assert_eq!(script.calls[1].receiver, thing("B"));
        assert_eq!(text(src, script.calls[1].property), "old");
        assert_eq!(
            script
                .members
                .iter()
                .filter(|member| member.is_callee)
                .count(),
            2
        );
    }

    #[test]
    fn a_call_is_reported_with_the_shape_of_its_first_argument() {
        let src = "me.a({ x: 1, 'y z': 2, [k]: 3, ...rest, shorthand, m() {}, 4: 5 }, 2);\n\
                   me.b(); me.c(f); me.d(...args); me.e(({ x: 1 }));";
        let script = facts(src);
        let call = |name: &str| {
            script
                .calls
                .iter()
                .find(|call| text(src, call.property) == name)
                .unwrap()
        };
        assert_eq!(keys(call("a")), ["x", "y z"]);
        let FirstArgument::Object(found) = &call("a").first else {
            unreachable!()
        };
        assert_eq!(text(src, found[0].span), "x");
        assert_eq!(
            text(src, found[1].span),
            "y z",
            "a string key is the bytes inside its quotes"
        );
        for name in ["b", "c", "d", "e"] {
            assert_eq!(call(name).first, FirstArgument::Other, "{name}");
        }
    }

    #[test]
    fn a_call_inside_a_template_literal_is_found_and_text_that_only_looks_like_one_is_not() {
        let src = "var t = `${Things.X.old()} me.old()`; var s = 'me.old(';\n\
                   // me.old(\nvar r = /me.old(/;";
        let script = facts(src);
        assert_eq!(script.calls.len(), 1);
        assert_eq!(script.calls[0].receiver, thing("X"));
        assert_eq!(script.calls[0].property.start, src.find("old()").unwrap());
    }

    #[test]
    fn a_division_chain_before_a_call_is_division_not_a_regex() {
        let src = "var q = a / b / c; me.old();";
        let script = facts(src);
        assert_eq!(script.calls.len(), 1);
        assert_eq!(script.calls[0].receiver, Receiver::Me);
    }

    #[test]
    fn new_with_arguments_is_a_call_and_without_them_is_not() {
        let src = "new me.a(1); new me.b;";
        let script = facts(src);
        assert!(member(src, &script, "a").is_callee);
        assert!(!member(src, &script, "b").is_callee);
        assert_eq!(script.calls.len(), 1);
    }

    #[test]
    fn a_variable_bound_once_to_a_thing_is_known() {
        let src = "var a = Things.A; let b = Things[\"B.C\"], c = other; const d = (Things.D);";
        let script = facts(src);
        let known: Vec<(&str, &str)> = script
            .thing_variables
            .iter()
            .map(|(name, entity)| (name.as_str(), entity.as_str()))
            .collect();
        assert_eq!(known, [("a", "A"), ("b", "B.C"), ("d", "D")]);
        assert_eq!(script.thing_of(&variable("a")), Some("A"));
        assert_eq!(script.thing_of(&thing("Z")), Some("Z"));
        assert_eq!(script.thing_of(&variable("c")), None);
        assert_eq!(script.thing_of(&Receiver::Me), None);
    }

    #[test]
    fn a_variable_declared_twice_is_known_only_if_both_name_the_same_thing() {
        assert!(facts("var t = Things.A; var t = Things.A;")
            .thing_variables
            .contains_key("t"));
        assert!(facts("var t = Things.A; var t = Things.B;")
            .thing_variables
            .is_empty());
        assert!(facts("var t = Things.A; var t = other;")
            .thing_variables
            .is_empty());
        assert!(facts("var t; t = Things.A;").thing_variables.is_empty());
    }

    #[test]
    fn a_variable_that_can_change_is_not_known() {
        for src in [
            "var t = Things.A; t = other; t.old();",
            "var t = Things.A; t += 1;",
            "var t = Things.A; t++;",
            "var t = Things.A; for (t in o) {}",
            "var t = Things.A; for (t of o) {}",
            "var t = Things.A; for (var t in o) {}",
            "var t = Things.A; [t] = x;",
            "var t = Things.A; ({ t } = x);",
            "var t = Things.A; var { t } = x;",
            "var t = Things.A; function f(t) {}",
            "var t = Things.A; var g = (t) => t;",
            "var t = Things.A; try {} catch (t) {}",
            "var t = Things.A; function t() {}",
        ] {
            assert!(facts(src).thing_variables.is_empty(), "{src}");
        }
    }

    #[test]
    fn a_property_assignment_through_a_variable_does_not_make_it_unknown() {
        assert!(facts("var t = Things.A; t.x = 1; t.y++;")
            .thing_variables
            .contains_key("t"));
    }

    #[test]
    fn strings_are_reported_with_the_span_inside_their_quotes() {
        let src = "var a = \"x\\\"y\"; var o = { 'k': 'v', k2: \"w\", n: 1 };\n\
                   me['m']; var t = `no`;";
        let script = facts(src);
        let values: Vec<&str> = script.strings.iter().map(|s| s.value.as_str()).collect();
        assert_eq!(values, ["x\\\"y", "k", "v", "w", "m"]);
        let pairs: Vec<(&str, &str)> = script
            .object_strings
            .iter()
            .map(|property| (property.key.as_str(), property.value.as_str()))
            .collect();
        assert_eq!(pairs, [("k", "v"), ("k2", "w")]);
        assert_eq!(text(src, script.object_strings[1].value_span), "w");
    }

    #[test]
    fn comments_are_reported_with_their_delimiters() {
        let src = "// one\nvar a; /* two */ var b; /** @function Run */";
        let script = facts(src);
        let found: Vec<(&str, &str)> = script
            .comments
            .iter()
            .map(|comment| (text(src, comment.span), comment.text.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("// one", "// one"),
                ("/* two */", "/* two */"),
                ("/** @function Run */", "/** @function Run */"),
            ]
        );
    }

    #[test]
    fn a_script_of_only_comments_still_reports_them() {
        let script = facts("/* only */");
        assert_eq!(script.comments.len(), 1);
    }

    #[test]
    fn verify_spans_refuses_a_span_that_does_not_fit() {
        let src = "me.old();";
        let mut script = facts(src);
        script.members[0].property = scan::Span::new(3, 99);
        assert!(script.verify_spans(src.as_bytes()).is_err());
        let wide = "é;";
        let mut script = facts(wide);
        script.identifiers.push(Identifier {
            span: scan::Span::new(1, 2),
            role: Role::Reference,
        });
        assert!(
            script.verify_spans(wide.as_bytes()).is_err(),
            "inside a character"
        );
    }

    #[test]
    fn object_literals_are_an_arena_with_literal_values_and_other_properties() {
        let src = "let value = { id: { 'items': [\"one\", , ...rest, { 1: 'two' }] }, method() {}, ...more, [computed]: 1 };";
        let script = facts(src);
        assert_eq!(script.objects.len(), 3);
        assert_eq!(&src[script.objects[0].span.start..script.objects[0].span.end], "{ id: { 'items': [\"one\", , ...rest, { 1: 'two' }] }, method() {}, ...more, [computed]: 1 }");
        let ObjectProperty::KeyValue { key, value } = &script.objects[0].properties[0] else {
            panic!("the outer property was not read");
        };
        assert_eq!(key.text, "id");
        assert_eq!(value, &Value::Object(1));
        assert!(script.objects[0]
            .properties
            .iter()
            .skip(1)
            .all(|property| matches!(property, ObjectProperty::Other)));
        let ObjectProperty::KeyValue { key, value } = &script.objects[1].properties[0] else {
            panic!("the nested property was not read");
        };
        assert_eq!(key.text, "items");
        let Value::Array(items) = value else {
            panic!("items was not an array");
        };
        assert!(matches!(items[0], Value::String(_)));
        assert!(matches!(items[1], Value::Other(_)));
        assert!(matches!(items[2], Value::Other(_)));
        assert_eq!(items[3], Value::Object(2));
        let ObjectProperty::KeyValue { key, value } = &script.objects[2].properties[0] else {
            panic!("the number key was not read");
        };
        assert_eq!(key.text, "1");
        assert!(matches!(value, Value::String(_)));
    }

    #[test]
    fn object_literal_spans_stay_correct_after_multi_byte_text() {
        let src = "let note = 'é'; let value = { 'table': [ { name: \"Field\" } ] };";
        let script = facts(src);
        assert_eq!(script.objects.len(), 2);
        assert_eq!(
            &src[script.objects[0].span.start..script.objects[0].span.end],
            "{ 'table': [ { name: \"Field\" } ] }"
        );
        assert_eq!(
            &src[script.objects[1].span.start..script.objects[1].span.end],
            "{ name: \"Field\" }"
        );
    }
}
