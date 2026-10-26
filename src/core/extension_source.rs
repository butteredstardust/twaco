//! Read the useful, source-level part of a Java extension without loading its jar.

use super::config::Solution;
use super::entity;
use super::scan::{self, Kind};
use super::types::{DataShape, Entity, Member, Parameter, Property, Service, TypedValue};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(crate) fn load(solution: &Solution) -> (Vec<Entity>, Vec<DataShape>, Vec<String>) {
    let mut entities = Vec::new();
    let mut shapes = Vec::new();
    let mut skipped = Vec::new();
    for extension in &solution.extensions {
        let root = solution.root.join(&extension.root);
        let name = extension
            .name
            .clone()
            .unwrap_or_else(|| extension.root.clone());
        match read_extension(&root, &name) {
            Ok((mut found, mut data_shapes)) => {
                entities.append(&mut found);
                shapes.append(&mut data_shapes);
            }
            Err(why) => skipped.push(format!("{}: {why}", solution.relative(&root))),
        }
    }
    (entities, shapes, skipped)
}

fn read_extension(root: &Path, extension: &str) -> Result<(Vec<Entity>, Vec<DataShape>), String> {
    let metadata = [
        root.join("configfiles/metadata.xml"),
        root.join("metadata.xml"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| "no metadata.xml".to_string())?;
    let bytes = std::fs::read(&metadata).map_err(|e| e.to_string())?;
    let entries = metadata_entries(&bytes).map_err(|e| format!("{}: {e}", metadata.display()))?;
    let mut by_class = BTreeMap::new();
    for entry in entries {
        if let Some(class) = entry.class.clone() {
            by_class.entry(class).or_insert_with(Vec::new).push(entry);
        }
    }
    let mut classes = BTreeMap::new();
    for class in by_class.keys() {
        load_class(root, class, &mut classes)?;
    }
    let mut entities = Vec::new();
    for (class, declarations) in by_class {
        let java = &classes[&class];
        let inherited = inherited_members(&class, &classes, &mut BTreeSet::new())?;
        for entry in declarations {
            let mut members = inherited.clone();
            members.extend(java.members.clone());
            members.sort_by(|a, b| a.name().cmp(b.name()));
            members.dedup_by(|a, b| a.name() == b.name());
            entities.push(Entity {
                name: entry.name,
                collection: entry.collection,
                project: String::new(),
                extension: Some(extension.to_string()),
                template: java.template.clone(),
                shapes: java.shapes.clone(),
                members,
                events: java.events.clone(),
                script_services: BTreeSet::new(),
            });
        }
    }
    let mut data_shapes = Vec::new();
    collect_xml(&root.join("Entities"), &mut |path| {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let info = entity::parse(&bytes).map_err(|e| format!("{e:?}"))?;
        if info.collection == "DataShapes" {
            let fields = super::datashape::extract(&bytes).map_err(|e| e.to_string())?;
            data_shapes.push(DataShape {
                name: info.name,
                base_data_shape: None,
                fields,
            });
        }
        Ok(())
    })?;
    Ok((entities, data_shapes))
}

fn load_class(
    root: &Path,
    class: &str,
    classes: &mut BTreeMap<String, JavaClass>,
) -> Result<(), String> {
    if classes.contains_key(class) {
        return Ok(());
    }
    let file = java_path(root, class).ok_or_else(|| format!("{class}: Java source not found"))?;
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let parsed = parse_java(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let parent = parsed.parent.as_ref().map(|parent| {
        if parent.contains('.') {
            parent.clone()
        } else {
            class.rsplit_once('.').map_or_else(
                || parent.clone(),
                |(package, _)| format!("{package}.{parent}"),
            )
        }
    });
    classes.insert(class.to_string(), parsed);
    if let Some(parent) = parent {
        if java_path(root, &parent).is_some() {
            load_class(root, &parent, classes)?;
        }
    }
    Ok(())
}

#[derive(Clone)]
struct MetadataEntry {
    collection: String,
    name: String,
    class: Option<String>,
}

fn metadata_entries(bytes: &[u8]) -> Result<Vec<MetadataEntry>, String> {
    let tokens = scan::tokenize(bytes).map_err(|e| e.to_string())?;
    let mut packages = BTreeMap::new();
    let mut raw = Vec::new();
    for token in &tokens {
        if !matches!(token.kind, Kind::Start | Kind::Empty) {
            continue;
        }
        let tag = std::str::from_utf8(token.name.of(bytes)).unwrap_or("");
        let attr = |name| {
            scan::attribute(bytes, token, name)
                .ok()
                .flatten()
                .map(|span| String::from_utf8_lossy(span.of(bytes)).into_owned())
        };
        if tag == "ThingPackage" {
            if let (Some(name), Some(class)) = (attr("name"), attr("className")) {
                packages.insert(name, class);
            }
        } else if matches!(tag, "ThingTemplate" | "ThingShape" | "Resource" | "Thing") {
            let class = if tag == "ThingTemplate" {
                attr("thingPackage").and_then(|p| packages.get(&p).cloned())
            } else {
                attr("className")
            };
            if let Some(name) = attr("name") {
                raw.push(MetadataEntry {
                    collection: format!("{}s", tag),
                    name,
                    class,
                });
            }
        }
    }
    Ok(raw)
}

fn java_path(root: &Path, class: &str) -> Option<PathBuf> {
    let relative = PathBuf::from(class.replace('.', "/")).with_extension("java");
    [
        root.join("src/main/java").join(&relative),
        root.join("src").join(relative),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn collect_xml(
    dir: &Path,
    visit: &mut dyn FnMut(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    for item in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect_xml(&path, visit)?;
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("xml"))
        {
            visit(&path)?;
        }
    }
    Ok(())
}

#[derive(Clone, Default)]
struct JavaClass {
    parent: Option<String>,
    members: Vec<Member>,
    template: Option<String>,
    shapes: Vec<String>,
    events: Vec<(String, Option<String>)>,
}

fn inherited_members(
    class: &str,
    classes: &BTreeMap<String, JavaClass>,
    seen: &mut BTreeSet<String>,
) -> Result<Vec<Member>, String> {
    if !seen.insert(class.to_string()) {
        return Err(format!("inheritance cycle at {class}"));
    }
    let java = classes
        .get(class)
        .ok_or_else(|| format!("class {class} is not in this extension"))?;
    let mut out = java
        .parent
        .as_ref()
        .and_then(|parent| {
            classes
                .iter()
                .find(|(name, _)| name.rsplit('.').next() == Some(parent))
                .map(|(name, _)| name.clone())
        })
        .map(|parent| inherited_members(&parent, classes, seen))
        .transpose()?
        .unwrap_or_default();
    out.extend(java.members.clone());
    seen.remove(class);
    Ok(out)
}

fn parse_java(source: &str) -> Result<JavaClass, String> {
    let text = strip_comments(source)?;
    let class_at = word_at(&text, "class").ok_or_else(|| "no class declaration".to_string())?;
    let before = &text[..class_at];
    let after = &text[class_at + 5..];
    let class_name = next_word(after).ok_or_else(|| "class has no name".to_string())?;
    let parent = after
        .find("extends")
        .and_then(|at| next_word(&after[at + 7..]));
    let mut out = JavaClass {
        parent,
        ..Default::default()
    };
    validate_annotations(before)?;
    for annotation in annotations(before) {
        match annotation.name.as_str() {
            "ThingworxBaseTemplateDefinition" => {
                out.template = string_arg(&annotation.args, "name")
            }
            "ThingworxImplementedShapeDefinitions" => {
                out.shapes = all_named(
                    &annotation.args,
                    "ThingworxImplementedShapeDefinition",
                    "name",
                )
            }
            "ThingworxPropertyDefinitions" => {
                for args in nested(&annotation.args, "ThingworxPropertyDefinition") {
                    out.members.push(Member::Property(Property {
                        name: string_arg(&args, "name").ok_or("property has no name")?,
                        description: string_arg(&args, "description").unwrap_or_default(),
                        value: value(&args),
                    }));
                }
            }
            "ThingworxEventDefinitions" => {
                for args in nested(&annotation.args, "ThingworxEventDefinition") {
                    out.events.push((
                        string_arg(&args, "name").ok_or("event has no name")?,
                        string_arg(&args, "dataShape"),
                    ));
                }
            }
            _ => {}
        }
    }
    let mut offset = class_at + class_name.len();
    while let Some(relative) = text[offset..].find("@ThingworxServiceDefinition") {
        let start = offset + relative;
        let service = annotation_at(&text, start)?;
        let end = start + service.end;
        let declaration = text[end..]
            .find("public ")
            .or_else(|| text[end..].find("protected "))
            .map(|n| end + n)
            .ok_or("service has no method declaration")?;
        let open = text[declaration..]
            .find('(')
            .map(|n| declaration + n)
            .ok_or("service has no parameter list")?;
        let close = matching(&text, open, '(', ')').ok_or("unbalanced service parameters")?;
        let method_start = text[close..]
            .find('{')
            .map(|n| close + n)
            .ok_or("service has no method body")?;
        let block = &text[end..method_start];
        validate_annotations(block)?;
        let name = string_arg(&service.args, "name")
            .unwrap_or_else(|| method_name(&text[declaration..open]).unwrap_or_default());
        if name.is_empty() {
            return Err("service has no name".to_string());
        }
        let result = annotations(block)
            .into_iter()
            .find(|a| a.name == "ThingworxServiceResult")
            .map(|a| value(&a.args))
            .unwrap_or(TypedValue {
                base_type: "NOTHING".to_string(),
                data_shape: None,
            });
        let parameter_text = &text[open + 1..close];
        validate_annotations(parameter_text)?;
        let parameters = annotations(parameter_text)
            .into_iter()
            .filter(|a| a.name == "ThingworxServiceParameter")
            .map(|a| Parameter {
                name: string_arg(&a.args, "name").unwrap_or_default(),
                description: string_arg(&a.args, "description").unwrap_or_default(),
                value: value(&a.args),
                required: false,
                default: None,
            })
            .collect();
        out.members.push(Member::Service(Service {
            name,
            description: string_arg(&service.args, "description").unwrap_or_default(),
            parameters,
            result,
        }));
        offset = method_start + 1;
    }
    out.members.sort_by(|a, b| a.name().cmp(b.name()));
    out.events.sort();
    Ok(out)
}

#[derive(Clone)]
struct Annotation {
    name: String,
    args: String,
    end: usize,
}
fn annotations(text: &str) -> Vec<Annotation> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(n) = text[at..].find('@') {
        let start = at + n;
        match annotation_at(text, start) {
            Ok(a) => {
                at = start + a.end;
                out.push(a);
            }
            Err(_) => break,
        }
    }
    out
}
fn validate_annotations(text: &str) -> Result<(), String> {
    let mut at = 0;
    while let Some(found) = text[at..].find('@') {
        let start = at + found;
        let annotation = annotation_at(text, start)?;
        at = start + annotation.end;
    }
    Ok(())
}
fn annotation_at(text: &str, start: usize) -> Result<Annotation, String> {
    let tail = &text[start + 1..];
    let name = next_word(tail).ok_or("annotation has no name")?;
    let name_end = start + 1 + name.len();
    let rest = &text[name_end..];
    let open = rest.find('(').ok_or("annotation has no arguments")?;
    let close = matching(rest, open, '(', ')').ok_or("unbalanced annotation")?;
    Ok(Annotation {
        name,
        args: rest[open + 1..close].to_string(),
        end: name_end - start + close + 1,
    })
}
fn word_at(text: &str, word: &str) -> Option<usize> {
    text.match_indices(word).find_map(|(at, _)| {
        let good = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        (good(text[..at].chars().next_back()) && good(text[at + word.len()..].chars().next()))
            .then_some(at)
    })
}
fn next_word(text: &str) -> Option<String> {
    let word: String = text
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
        .collect();
    (!word.is_empty()).then_some(word)
}
fn method_name(text: &str) -> Option<String> {
    text.split_whitespace().last().map(|s| s.to_string())
}
fn matching(text: &str, open: usize, left: char, right: char) -> Option<usize> {
    let mut depth = 0;
    let mut quote = None;
    let mut escape = false;
    for (at, c) in text.char_indices().skip_while(|(at, _)| *at < open) {
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if matches!(c, '\"' | '\'') {
            quote = Some(c);
        } else if c == left {
            depth += 1;
        } else if c == right {
            depth -= 1;
            if depth == 0 {
                return Some(at);
            }
        }
    }
    None
}
fn string_arg(args: &str, key: &str) -> Option<String> {
    let at = args.find(&format!("{key} ="))? + key.len() + 1;
    quoted(&args[at..])
}
fn quoted(text: &str) -> Option<String> {
    let start = text.find('\"')? + 1;
    let mut out = String::new();
    let mut escape = false;
    for c in text[start..].chars() {
        if escape {
            out.push(c);
            escape = false;
        } else if c == '\\' {
            escape = true;
        } else if c == '\"' {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    None
}
fn nested(args: &str, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(n) = args[at..].find(name) {
        let start = at + n + name.len();
        let Some(open) = args[start..].find('(') else {
            break;
        };
        let open = start + open;
        let Some(close) = matching(args, open, '(', ')') else {
            break;
        };
        out.push(args[open + 1..close].to_string());
        at = close + 1;
    }
    out
}
fn all_named(args: &str, annotation: &str, key: &str) -> Vec<String> {
    nested(args, annotation)
        .into_iter()
        .filter_map(|part| string_arg(&part, key))
        .collect()
}
fn value(args: &str) -> TypedValue {
    TypedValue {
        base_type: string_arg(args, "baseType").unwrap_or_default(),
        data_shape: string_arg(args, "dataShape").or_else(|| {
            args.split('"')
                .find_map(|part| part.strip_prefix("dataShape:").map(str::to_string))
        }),
    }
}

fn strip_comments(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut quote = None;
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if matches!(c, '\"' | '\'') {
            quote = Some(c);
            out.push(c);
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for c in chars.by_ref() {
                if c == '\n' {
                    out.push('\n');
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    closed = true;
                    break;
                }
                if c == '\n' {
                    out.push('\n');
                }
            }
            if !closed {
                return Err("unterminated comment".to_string());
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reads_java_annotations_and_ignores_comments() {
        let java = r#"
            // @ThingworxServiceDefinition(name = "Ignored")
            @ThingworxPropertyDefinitions(properties = { @ThingworxPropertyDefinition(name = "Status", baseType = "STRING") })
            @ThingworxBaseTemplateDefinition(name = "GenericThing")
            class Child extends Parent {
              @ThingworxServiceDefinition(name = "Send", description = "quotes \\" and )")
              @ThingworxServiceResult(name = "result", baseType = "INFOTABLE", aspects = {"dataShape:Rows"})
              public InfoTable Send(@ThingworxServiceParameter(name = "row", baseType = "INFOTABLE", aspects = {"dataShape:Rows"}) InfoTable row) { return null; }
            }"#;
        let parsed = parse_java(java).unwrap();
        assert_eq!(parsed.template.as_deref(), Some("GenericThing"));
        assert!(parsed
            .members
            .iter()
            .any(|member| member.name() == "Status"));
        let Member::Service(send) = parsed
            .members
            .iter()
            .find(|member| member.name() == "Send")
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(send.parameters[0].value.data_shape.as_deref(), Some("Rows"));
        assert_eq!(send.result.data_shape.as_deref(), Some("Rows"));
    }

    #[test]
    fn metadata_template_inherits_local_java_members() {
        let root = tempfile::tempdir().unwrap();
        let extension = root.path().join("extension");
        fs::create_dir_all(extension.join("configfiles")).unwrap();
        fs::create_dir_all(extension.join("src/main/java/com/acme")).unwrap();
        fs::write(
            root.path().join("twaco.toml"),
            "[[project]]\nname = 'P'\n\n[[extension]]\nroot = 'extension'\nname = 'Acme Java'\n",
        )
        .unwrap();
        fs::write(
            extension.join("configfiles/metadata.xml"),
            r#"<Entities><ThingPackages><ThingPackage name="P" className="com.acme.Child"/></ThingPackages><ThingTemplates><ThingTemplate name="Acme.Template" thingPackage="P"/></ThingTemplates></Entities>"#,
        )
        .unwrap();
        fs::write(
            extension.join("src/main/java/com/acme/Parent.java"),
            r#"class Parent { @ThingworxServiceDefinition(name = "Inherited") public void Inherited() {} }"#,
        )
        .unwrap();
        fs::write(
            extension.join("src/main/java/com/acme/Child.java"),
            r#"class Child extends Parent { @ThingworxServiceDefinition(name = "Send") public void Send() {} }"#,
        )
        .unwrap();
        let solution = Solution::load(&root.path().join("twaco.toml")).unwrap();
        let (entities, _, skipped) = load(&solution);
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(entities[0].extension.as_deref(), Some("Acme Java"));
        assert_eq!(
            entities[0]
                .members
                .iter()
                .map(Member::name)
                .collect::<Vec<_>>(),
            ["Inherited", "Send"]
        );
    }

    /// A solution beside its extension: impact on the extension's service names the solution's
    /// caller, catalog shows the service, and no workspace writer can see the extension.
    #[test]
    fn a_sibling_extension_shows_in_impact_and_catalog_and_stays_out_of_the_workspace() {
        let root = tempfile::tempdir().unwrap();
        let solution_root = root.path().join("solution");
        let extension = root.path().join("Acme_Extension");
        fs::create_dir_all(solution_root.join("app/Things")).unwrap();
        fs::create_dir_all(solution_root.join("app/Projects")).unwrap();
        fs::create_dir_all(extension.join("configfiles")).unwrap();
        fs::create_dir_all(extension.join("src/main/java/com/acme")).unwrap();
        fs::write(
            solution_root.join("twaco.toml"),
            "[[project]]\nname = 'P'\nroot = 'app'\n\n[[extension]]\nroot = '../Acme_Extension'\nname = 'Acme'\n",
        )
        .unwrap();
        fs::write(
            solution_root.join("app/Projects/P.xml"),
            r#"<Entities><Projects><Project name="P" projectName="P"/></Projects></Entities>"#,
        )
        .unwrap();
        fs::write(
            solution_root.join("app/Things/P.Sender.xml"),
            r#"<Entities><Things><Thing name="P.Sender" projectName="P" thingTemplate="Acme.Producer_TT"/></Things></Entities>"#,
        )
        .unwrap();
        fs::write(
            extension.join("configfiles/metadata.xml"),
            r#"<Entities><ThingPackages><ThingPackage name="Acme.Producer_TP" className="com.acme.Producer"/></ThingPackages><ThingTemplates><ThingTemplate name="Acme.Producer_TT" thingPackage="Acme.Producer_TP"/></ThingTemplates></Entities>"#,
        )
        .unwrap();
        let java = r#"class Producer { @ThingworxServiceDefinition(name = "Send", description = "Send one message") @ThingworxServiceResult(name = "result", baseType = "STRING") public String Send(@ThingworxServiceParameter(name = "topic", baseType = "STRING") String topic) { return topic; } }"#;
        let java_path = extension.join("src/main/java/com/acme/Producer.java");
        fs::write(&java_path, java).unwrap();
        let solution = Solution::load(&solution_root.join("twaco.toml")).unwrap();

        let report = super::super::impact::run(
            &solution,
            &super::super::impact::Request {
                entity: "Acme.Producer_TT".into(),
                member: Some("Send".into()),
                min: super::super::index::Confidence::Review,
                depth: None,
            },
        )
        .unwrap();
        assert_eq!(report.extension.as_deref(), Some("Acme"));
        assert!(
            report
                .dependents
                .iter()
                .any(|dependent| dependent.label == "Things/P.Sender"),
            "{:?}",
            report.dependents
        );

        let catalog = super::super::catalog::build(
            &solution,
            super::super::catalog::Query {
                entity: Some("Acme.Producer_TT"),
                ..Default::default()
            },
        )
        .unwrap();
        let service = &catalog.entities[0].services[0];
        assert_eq!(
            (
                service.name.as_str(),
                catalog.entities[0].extension.as_deref()
            ),
            ("Send", Some("Acme"))
        );

        // Writers work from the workspace scan, which never holds an extension's entity.
        assert!(super::super::workspace::entities(&solution)
            .iter()
            .all(|file| !file.path.starts_with(&extension)));
        assert_eq!(fs::read_to_string(&java_path).unwrap(), java);
    }

    #[test]
    fn missing_root_is_skipped_not_fatal() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("twaco.toml"),
            "[[project]]\nname = 'P'\n\n[[extension]]\nroot = 'missing'\n",
        )
        .unwrap();
        let solution = Solution::load(&root.path().join("twaco.toml")).unwrap();
        let (entities, shapes, skipped) = load(&solution);
        assert!(entities.is_empty() && shapes.is_empty());
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("no metadata.xml"));
    }
}
