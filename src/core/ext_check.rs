//! `ext check`: what would make a Java extension fail at import or at run time, read from its
//! source folder before it is built or uploaded.
//!
//! The project is the one the ThingWorx Eclipse plugin and its Gradle and Ant templates lay out:
//! `configfiles/metadata.xml` (older projects: `metadata.xml` at the root), Java under
//! `src/main/java` or `src`, jars under `lib/` and `twx-lib/`, widgets under `ui/<name>/` and
//! shipped entities under `Entities/`. Nothing here writes to it.

use super::entity::edit::BASE_TYPES;
use super::extension_source::{java_path, parse_java, JavaClass};
use super::scan::{self, Kind};
use super::types::Member;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One problem, in the file it is in; `line` is 0 when it is about the whole file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub rule: &'static str,
    pub message: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    /// The extension package's name and version, when metadata.xml says.
    pub package: Option<String>,
    pub version: Option<String>,
    /// Java classes metadata.xml names, with the superclasses they inherit from.
    pub classes: usize,
    pub findings: Vec<Finding>,
    /// What may be fine but cannot be proven here.
    pub notes: Vec<String>,
}

/// A metadata.xml element twaco checks, with the line it starts on.
pub(crate) struct Declared {
    pub(crate) tag: String,
    pub(crate) name: String,
    line: usize,
    pub(crate) attributes: BTreeMap<String, String>,
    /// FileResource `file` values nested inside it (a widget's files).
    pub(crate) files: Vec<String>,
}

const CLASS_TAGS: &[&str] = &[
    "ThingShape",
    "Resource",
    "Thing",
    "Authenticator",
    "DirectoryService",
    "ScriptFunctionLibrary",
];

/// Check the extension project at `root`.
pub fn check(root: &Path) -> Report {
    if super::ext_widget::read(root).is_some() {
        return super::ext_widget::check(root);
    }
    let mut report = Report::default();
    let Some(metadata) = [
        root.join("configfiles/metadata.xml"),
        root.join("metadata.xml"),
    ]
    .into_iter()
    .find(|path| path.is_file()) else {
        report.findings.push(Finding {
            file: "configfiles/metadata.xml".into(),
            line: 0,
            rule: "metadata-unreadable",
            message:
                "no configfiles/metadata.xml or metadata.xml; this is not an extension project"
                    .into(),
        });
        return report;
    };
    let metadata_file = relative(root, &metadata);
    let declared = match std::fs::read(&metadata)
        .map_err(|error| error.to_string())
        .and_then(|bytes| declarations(&bytes))
    {
        Ok(declared) => declared,
        Err(why) => {
            report.findings.push(Finding {
                file: metadata_file,
                line: 0,
                rule: "metadata-unreadable",
                message: format!("not well-formed XML: {why}"),
            });
            return report;
        }
    };
    let finding = |line: usize, rule: &'static str, message: String| Finding {
        file: metadata_file.clone(),
        line,
        rule,
        message,
    };

    if let Some(package) = declared.iter().find(|item| item.tag == "ExtensionPackage") {
        report.package = Some(package.name.clone());
        report.version = package.attributes.get("packageVersion").cloned();
    }
    let jars = jar_names(root);

    // The classes metadata.xml names: a ThingPackage's for its templates, the others' own.
    let packages: BTreeMap<&str, &Declared> = declared
        .iter()
        .filter(|item| item.tag == "ThingPackage")
        .map(|item| (item.name.as_str(), item))
        .collect();
    let mut wanted: Vec<(&Declared, &str)> = Vec::new();
    for item in &declared {
        match item.tag.as_str() {
            "ThingPackage" => {
                if let Some(class) = item.attributes.get("className") {
                    wanted.push((item, class));
                }
            }
            "ThingTemplate" => {
                if let Some(package) = item.attributes.get("thingPackage") {
                    if !package.is_empty() && !packages.contains_key(package.as_str()) {
                        report.findings.push(finding(
                            item.line,
                            "unknown-thing-package",
                            format!(
                                "ThingTemplate {} names thingPackage {package}, which metadata.xml does not declare",
                                item.name
                            ),
                        ));
                    }
                }
            }
            tag if CLASS_TAGS.contains(&tag) => {
                if let Some(class) = item.attributes.get("className") {
                    wanted.push((item, class));
                }
            }
            _ => {}
        }
    }

    let mut classes: BTreeMap<String, (PathBuf, JavaClass, String)> = BTreeMap::new();
    for (item, class) in &wanted {
        if classes.contains_key(*class) {
            continue;
        }
        match java_path(root, class) {
            Some(path) => read_class(root, class, &path, &mut classes, &mut report),
            None if !jars.lib.is_empty() => report.notes.push(format!(
                "{} {} names class {class}, which is not in the source; it may be in a jar under lib/",
                item.tag, item.name
            )),
            None => report.findings.push(finding(
                item.line,
                "missing-class",
                format!(
                    "{} {} names class {class}, which is neither in the source nor in a jar under lib/",
                    item.tag, item.name
                ),
            )),
        }
    }
    report.classes = classes.len();

    // Members: one name once per kind, counting what a class inherits within the project.
    let named: BTreeSet<&str> = wanted.iter().map(|(_, class)| *class).collect();
    for class in &named {
        let Some((path, _, source)) = classes.get(*class) else {
            continue;
        };
        let chain = chain(class, &classes);
        let mut seen: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for java in &chain {
            for member in &java.members {
                let kind = match member {
                    Member::Service(_) => "service",
                    Member::Property(_) => "property",
                };
                *seen.entry((kind, member.name())).or_default() += 1;
            }
            for (event, _) in &java.events {
                *seen.entry(("event", event.as_str())).or_default() += 1;
            }
        }
        for ((kind, name), count) in seen {
            if count > 1 {
                report.findings.push(Finding {
                    file: relative(root, path),
                    line: line_of(source, name),
                    rule: "duplicate-member",
                    message: format!(
                        "{class} has {count} {kind}s named {name}, its superclasses' included"
                    ),
                });
            }
        }
    }

    // Base types and DataShapes of every class read.
    let shipped = shipped_data_shapes(root, &declared);
    let mut unknown_shapes = BTreeSet::new();
    for (class, (path, java, source)) in &classes {
        let file = relative(root, path);
        let mut typed: Vec<(String, &str, Option<&str>)> = Vec::new();
        for member in &java.members {
            match member {
                Member::Service(service) => {
                    typed.push((
                        format!("service {} result", service.name),
                        &service.result.base_type,
                        service.result.data_shape.as_deref(),
                    ));
                    for parameter in &service.parameters {
                        typed.push((
                            format!("service {} parameter {}", service.name, parameter.name),
                            &parameter.value.base_type,
                            parameter.value.data_shape.as_deref(),
                        ));
                    }
                }
                Member::Property(property) => typed.push((
                    format!("property {}", property.name),
                    &property.value.base_type,
                    property.value.data_shape.as_deref(),
                )),
            }
        }
        for (event, shape) in &java.events {
            typed.push((format!("event {event}"), "", shape.as_deref()));
        }
        for (what, base_type, shape) in typed {
            // NOTHING is a result's type only; the property list does not carry it.
            if !base_type.is_empty() && base_type != "NOTHING" && !BASE_TYPES.contains(&base_type) {
                let name = what.split(' ').nth(1).unwrap_or_default();
                report.findings.push(Finding {
                    file: file.clone(),
                    line: line_of(source, name),
                    rule: "bad-base-type",
                    message: format!("{class} {what} has baseType {base_type}, which is not a ThingWorx base type"),
                });
            }
            if let Some(shape) = shape.filter(|shape| !shape.is_empty()) {
                if !shipped.contains(shape) {
                    unknown_shapes.insert(shape.to_string());
                }
            }
        }
    }
    for shape in unknown_shapes {
        report.notes.push(format!(
            "DataShape {shape} is not shipped by this extension; it must come from the platform or another extension"
        ));
    }

    // Jars the package lists.
    for item in declared.iter().filter(|item| item.tag == "JarResources") {
        for file in &item.files {
            if !jars.present.contains(file.as_str()) && !jars.built.contains(file.as_str()) {
                report.findings.push(finding(
                    item.line,
                    "missing-jar",
                    format!("JarResources lists {file}, which is in neither lib/ nor twx-lib/ and is not the jar this project builds"),
                ));
            }
        }
    }

    // Widgets and their files.
    for item in declared.iter().filter(|item| item.tag == "Widget") {
        let folder = root.join("ui").join(&item.name);
        let files: Vec<String> = if item.files.is_empty() {
            vec![
                format!("{}.ide.js", item.name),
                format!("{}.runtime.js", item.name),
            ]
        } else {
            item.files.clone()
        };
        for file in files {
            let found = folder.join(&file).is_file()
                || root.join("ui").join(&file).is_file()
                || root.join(&file).is_file();
            if !found {
                report.findings.push(finding(
                    item.line,
                    "missing-widget-file",
                    format!(
                        "Widget {} needs {file}, which is not under ui/{}/",
                        item.name, item.name
                    ),
                ));
            }
        }
    }

    // The version Gradle stamps against the one metadata.xml declares.
    if let (Some(declared_version), Some(properties)) = (
        &report.version,
        properties(&root.join("extension.properties")),
    ) {
        if let Some(stamped) = properties.get("package_version") {
            if stamped != declared_version {
                let line = declared
                    .iter()
                    .find(|item| item.tag == "ExtensionPackage")
                    .map_or(0, |item| item.line);
                report.findings.push(finding(
                    line,
                    "version-mismatch",
                    format!("packageVersion is {declared_version} but extension.properties says package_version {stamped}"),
                ));
            }
        }
    }

    // Classes that define ThingWorx members but nothing declares or extends.
    let used: BTreeSet<&str> = classes.keys().map(String::as_str).collect();
    for (class, path) in annotated_classes(root) {
        if !used.contains(class.as_str()) {
            report.notes.push(format!(
                "{} defines ThingWorx members but metadata.xml names no entity with class {class}",
                relative(root, &path)
            ));
        }
    }
    report
        .findings
        .sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report
}

fn read_class(
    root: &Path,
    class: &str,
    path: &Path,
    classes: &mut BTreeMap<String, (PathBuf, JavaClass, String)>,
    report: &mut Report,
) {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            report.findings.push(Finding {
                file: relative(root, path),
                line: 0,
                rule: "java-unreadable",
                message: error.to_string(),
            });
            return;
        }
    };
    let java = match parse_java(&source) {
        Ok(java) => java,
        Err(why) => {
            report.findings.push(Finding {
                file: relative(root, path),
                line: 0,
                rule: "java-unreadable",
                message: format!("{class}: {why}"),
            });
            return;
        }
    };
    let parent = java.parent.as_ref().map(|parent| qualified(class, parent));
    classes.insert(class.to_string(), (path.to_path_buf(), java, source));
    if let Some(parent) = parent {
        if !classes.contains_key(&parent) {
            if let Some(parent_path) = java_path(root, &parent) {
                read_class(root, &parent, &parent_path, classes, report);
            }
        }
    }
}

/// `parent` as written in `class`'s source, qualified by that class's package when it is bare.
fn qualified(class: &str, parent: &str) -> String {
    if parent.contains('.') {
        return parent.to_string();
    }
    class.rsplit_once('.').map_or_else(
        || parent.to_string(),
        |(package, _)| format!("{package}.{parent}"),
    )
}

/// The class and the superclasses of it the project holds, nearest last.
fn chain<'a>(
    class: &str,
    classes: &'a BTreeMap<String, (PathBuf, JavaClass, String)>,
) -> Vec<&'a JavaClass> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = Some(class.to_string());
    while let Some(name) = current {
        if !seen.insert(name.clone()) {
            break;
        }
        let Some((_, java, _)) = classes.get(&name) else {
            break;
        };
        out.push(java);
        current = java.parent.as_ref().map(|parent| qualified(&name, parent));
    }
    out.reverse();
    out
}

pub(crate) fn declarations(bytes: &[u8]) -> Result<Vec<Declared>, String> {
    let tokens = scan::tokenize(bytes).map_err(|error| error.to_string())?;
    let mut out: Vec<Declared> = Vec::new();
    // The open Widget or JarResources whose FileResources are collected.
    let mut holder: Option<usize> = None;
    for token in &tokens {
        let tag = std::str::from_utf8(token.name.of(bytes)).unwrap_or("");
        match token.kind {
            Kind::Start | Kind::Empty => {
                let attributes = attributes(bytes, token);
                if tag == "FileResource" {
                    if let (Some(at), Some(file)) = (holder, attributes.get("file")) {
                        out[at].files.push(file.clone());
                    }
                    continue;
                }
                let interesting = matches!(
                    tag,
                    "ExtensionPackage"
                        | "ThingPackage"
                        | "ThingTemplate"
                        | "Widget"
                        | "JarResources"
                ) || CLASS_TAGS.contains(&tag);
                if !interesting {
                    continue;
                }
                out.push(Declared {
                    tag: tag.to_string(),
                    name: attributes.get("name").cloned().unwrap_or_default(),
                    line: line_at(bytes, token.span.start),
                    attributes,
                    files: Vec::new(),
                });
                if matches!(tag, "Widget" | "JarResources") && token.kind == Kind::Start {
                    holder = Some(out.len() - 1);
                }
            }
            Kind::End if matches!(tag, "Widget" | "JarResources") => holder = None,
            _ => {}
        }
    }
    Ok(out)
}

fn attributes(bytes: &[u8], token: &scan::Token) -> BTreeMap<String, String> {
    [
        "name",
        "className",
        "thingPackage",
        "packageVersion",
        "file",
    ]
    .into_iter()
    .filter_map(|name| {
        scan::attribute(bytes, token, name)
            .ok()
            .flatten()
            .map(|value| {
                (
                    name.to_string(),
                    scan::decode_entities(&String::from_utf8_lossy(value.of(bytes))),
                )
            })
    })
    .collect()
}

/// The jars beside the source, and the names the build gives the extension's own jar.
pub(crate) struct Jars {
    lib: BTreeSet<String>,
    present: BTreeSet<String>,
    pub(crate) built: BTreeSet<String>,
}

pub(crate) fn jar_names(root: &Path) -> Jars {
    // JarResources may list any file (a licence, say), and the Gradle template copies lib/ and
    // lib/common/ into the package.
    let list = |dir: &str| -> BTreeSet<String> {
        std::fs::read_dir(root.join(dir))
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    let lib: BTreeSet<String> = list("lib")
        .into_iter()
        .filter(|name| name.to_ascii_lowercase().ends_with(".jar"))
        .collect();
    let mut present = list("lib");
    present.extend(list("lib/common"));
    present.extend(list("twx-lib"));
    // The Eclipse templates name the jar after the project, lower-cased with spaces as `_`; a
    // build script may name it outright.
    let mut built = BTreeSet::new();
    let mut projects = vec![root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()];
    if let Ok(settings) = std::fs::read_to_string(root.join("settings.gradle")) {
        if let Some(name) = settings
            .lines()
            .find_map(|line| line.trim().strip_prefix("rootProject.name"))
            .and_then(|rest| rest.split(['\'', '"']).nth(1))
        {
            projects.push(name.to_string());
        }
    }
    for project in projects {
        built.insert(format!("{}.jar", project.to_lowercase().replace(' ', "_")));
    }
    for script in ["build.gradle", "build-extension.xml"] {
        if let Ok(text) = std::fs::read_to_string(root.join(script)) {
            for part in text.split(['\'', '"', ' ', '\n', '>', '<', '=']) {
                if part.to_ascii_lowercase().ends_with(".jar")
                    && !part.contains(['$', '|', '/', '\\', '*'])
                {
                    built.insert(part.to_string());
                }
            }
        }
    }
    Jars {
        lib,
        present,
        built,
    }
}

/// The DataShapes the extension ships: in `Entities/`, or declared in metadata.xml.
fn shipped_data_shapes(root: &Path, declared: &[Declared]) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = declared
        .iter()
        .filter(|item| item.tag == "DataShape")
        .map(|item| item.name.clone())
        .collect();
    let mut stack = vec![root.join("Entities")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("xml"))
            {
                if let Ok(bytes) = std::fs::read(&path) {
                    for shape in data_shape_names(&bytes) {
                        out.insert(shape);
                    }
                }
            }
        }
    }
    out
}

/// Every DataShape an export declares, single-entity file or many.
fn data_shape_names(bytes: &[u8]) -> Vec<String> {
    let Ok(tokens) = scan::tokenize(bytes) else {
        return Vec::new();
    };
    tokens
        .iter()
        .filter(|token| {
            matches!(token.kind, Kind::Start | Kind::Empty) && token.name.of(bytes) == b"DataShape"
        })
        .filter_map(|token| {
            scan::attribute(bytes, token, "name")
                .ok()
                .flatten()
                .map(|value| String::from_utf8_lossy(value.of(bytes)).into_owned())
        })
        .collect()
}

/// Java sources under the source roots that declare ThingWorx members, by class name.
fn annotated_classes(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for base in [root.join("src/main/java"), root.join("src")] {
        if !base.is_dir() {
            continue;
        }
        let mut stack = vec![base.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    // Tests and other source sets are not the extension's.
                    if dir == base
                        || !matches!(entry.file_name().to_str(), Some("test" | "integrationTest"))
                    {
                        stack.push(path);
                    }
                } else if path.extension().is_some_and(|ext| ext == "java") {
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    let defines = [
                        "@ThingworxServiceDefinition",
                        "@ThingworxPropertyDefinitions",
                        "@ThingworxEventDefinitions",
                    ]
                    .iter()
                    .any(|annotation| text.contains(annotation));
                    let Ok(relative) = path.strip_prefix(&base) else {
                        continue;
                    };
                    let class = relative
                        .with_extension("")
                        .to_string_lossy()
                        .replace(['\\', '/'], ".");
                    if defines && !out.iter().any(|(known, _)| *known == class) {
                        out.push((class, path));
                    }
                }
            }
        }
        // `src/main/java` holds the project when it exists; `src` would see it again.
        if base.ends_with("java") {
            break;
        }
    }
    out.sort();
    out
}

/// A Java `.properties` file's keys, when it exists.
fn properties(path: &Path) -> Option<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with(['#', '!']))
            .filter_map(|line| line.split_once(['=', ':']))
            .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
            .collect(),
    )
}

fn line_at(bytes: &[u8], at: usize) -> usize {
    bytes[..at.min(bytes.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

/// The line of `name = "<value>"` in a Java source, or 0.
fn line_of(source: &str, value: &str) -> usize {
    if value.is_empty() {
        return 0;
    }
    let needle = format!("\"{value}\"");
    source
        .find(&needle)
        .map_or(0, |at| line_at(source.as_bytes(), at))
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The report as text: a summary, each finding, then the notes.
pub fn render(report: &Report) -> String {
    let mut out = String::new();
    if let Some(package) = &report.package {
        out.push_str(&format!(
            "extension {package} {}\n",
            report.version.as_deref().unwrap_or("")
        ));
    }
    for finding in &report.findings {
        let place = if finding.line > 0 {
            format!("{}:{}", finding.file, finding.line)
        } else {
            finding.file.clone()
        };
        out.push_str(&format!(
            "  {place} [{}] {}\n",
            finding.rule, finding.message
        ));
    }
    out.push_str(&format!(
        "{} finding(s); {} class(es) read\n",
        report.findings.len(),
        report.classes
    ));
    for note in &report.notes {
        out.push_str(&format!("note: {note}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn rules(report: &Report) -> Vec<&'static str> {
        report.findings.iter().map(|finding| finding.rule).collect()
    }

    /// A Gradle project as the Eclipse templates lay it out: a ThingPackage's template, a
    /// shape, a superclass in the project, a widget, its own jar and a library.
    fn clean(root: &Path) {
        write(root, "settings.gradle", "rootProject.name = 'Acme Ext'\n");
        write(root, "extension.properties", "package_version=1.2.0\n");
        write(root, "lib/library.jar", "");
        write(root, "lib/library.lic", "");
        write(root, "ui/gauge/gauge.ide.js", "");
        write(root, "ui/gauge/gauge.runtime.js", "");
        write(
            root,
            "Entities/Acme.Row_DS.xml",
            r#"<Entities><DataShapes><DataShape name="Acme.Row_DS"/></DataShapes></Entities>"#,
        );
        write(
            root,
            "configfiles/metadata.xml",
            r#"<Entities>
<ExtensionPackages><ExtensionPackage name="Acme_Ext" packageVersion="1.2.0">
<JarResources><FileResource file="acme_ext.jar" type="JAR"/><FileResource file="library.jar" type="JAR"/><FileResource file="library.lic" type="UNKNOWN"/></JarResources>
</ExtensionPackage></ExtensionPackages>
<ThingPackages><ThingPackage name="Acme.Sender_TP" className="com.acme.Sender"/></ThingPackages>
<ThingTemplates><ThingTemplate name="Acme.Sender_TT" thingPackage="Acme.Sender_TP"/></ThingTemplates>
<ThingShapes><ThingShape name="Acme.Math_TS" className="com.acme.Math"/></ThingShapes>
<Widgets><Widget name="gauge"/></Widgets>
</Entities>"#,
        );
        write(
            root,
            "src/main/java/com/acme/Base.java",
            r#"package com.acme; public abstract class Base { @ThingworxServiceDefinition(name = "Ping") public String Ping() { return ""; } }"#,
        );
        write(
            root,
            "src/main/java/com/acme/Sender.java",
            r#"package com.acme;
@ThingworxEventDefinitions(events = { @ThingworxEventDefinition(name = "Sent", dataShape = "Acme.Row_DS") })
public class Sender extends Base {
    @ThingworxServiceDefinition(name = "Send", description = "a \"quoted\" ) text")
    @ThingworxServiceResult(name = "result", baseType = "INFOTABLE", aspects = {"dataShape:Acme.Row_DS"})
    public InfoTable Send(@ThingworxServiceParameter(name="topic", baseType="STRING") String topic) { return null; }
}"#,
        );
        write(
            root,
            "src/main/java/com/acme/Math.java",
            r#"package com.acme; public class Math { @ThingworxServiceDefinition(name = "Eval") public Double Eval() { return 0.0; } }"#,
        );
    }

    #[test]
    fn a_clean_project_has_no_finding() {
        let dir = tempfile::tempdir().unwrap();
        clean(dir.path());
        let report = check(dir.path());
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert!(report.notes.is_empty(), "{:?}", report.notes);
        assert_eq!(report.classes, 3);
        assert_eq!(report.version.as_deref(), Some("1.2.0"));
    }

    #[test]
    fn each_rule_names_its_problem() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        clean(root);
        write(root, "extension.properties", "package_version=1.3.0\n");
        std::fs::remove_file(root.join("ui/gauge/gauge.runtime.js")).unwrap();
        std::fs::remove_file(root.join("lib/library.jar")).unwrap();
        std::fs::remove_file(root.join("lib/library.lic")).unwrap();
        write(
            root,
            "configfiles/metadata.xml",
            r#"<Entities>
<ExtensionPackages><ExtensionPackage name="Acme_Ext" packageVersion="1.2.0">
<JarResources><FileResource file="acme_ext.jar" type="JAR"/><FileResource file="missing.jar" type="JAR"/></JarResources>
</ExtensionPackage></ExtensionPackages>
<ThingPackages><ThingPackage name="Acme.Sender_TP" className="com.acme.Sender"/></ThingPackages>
<ThingTemplates><ThingTemplate name="Acme.Sender_TT" thingPackage="Acme.Sender_TP"/><ThingTemplate name="Acme.Lost_TT" thingPackage="Acme.Nowhere_TP"/></ThingTemplates>
<ThingShapes><ThingShape name="Acme.Math_TS" className="com.acme.Math"/><ThingShape name="Acme.Gone_TS" className="com.acme.Gone"/></ThingShapes>
<Widgets><Widget name="gauge"/></Widgets>
</Entities>"#,
        );
        write(
            root,
            "src/main/java/com/acme/Sender.java",
            r#"package com.acme;
@ThingworxEventDefinitions(events = { @ThingworxEventDefinition(name = "Sent", dataShape = "Elsewhere_DS") })
public class Sender extends Base {
    @ThingworxServiceDefinition(name = "Ping")
    public String Ping() { return ""; }
    @ThingworxServiceDefinition(name = "Send")
    @ThingworxServiceResult(name = "result", baseType = "TEXTISH")
    public String Send() { return ""; }
}"#,
        );
        write(
            root,
            "src/main/java/com/acme/Orphan.java",
            r#"package com.acme; public class Orphan { @ThingworxServiceDefinition(name = "X") public void X() {} }"#,
        );
        let report = check(root);
        let mut found = rules(&report);
        found.sort();
        assert_eq!(
            found,
            [
                "bad-base-type",
                "duplicate-member",
                "missing-class",
                "missing-jar",
                "missing-widget-file",
                "unknown-thing-package",
                "version-mismatch",
            ],
            "{:#?}",
            report.findings
        );
        let notes = report.notes.join("\n");
        assert!(notes.contains("Elsewhere_DS"), "{notes}");
        assert!(notes.contains("com.acme.Orphan"), "{notes}");
        let duplicate = report
            .findings
            .iter()
            .find(|finding| finding.rule == "duplicate-member")
            .unwrap();
        assert!(
            duplicate.message.contains("Ping") && duplicate.line > 0,
            "{duplicate:?}"
        );
    }

    #[test]
    fn a_folder_without_metadata_and_a_class_only_in_a_jar() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(rules(&check(dir.path())), ["metadata-unreadable"]);
        clean(dir.path());
        std::fs::remove_file(dir.path().join("src/main/java/com/acme/Math.java")).unwrap();
        let report = check(dir.path());
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("may be in a jar")),
            "{:?}",
            report.notes
        );
    }
}
