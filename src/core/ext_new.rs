//! `ext new` and `ext add`: lay out a Java extension project, and add a template, shape,
//! resource, widget or jar to one, as the ThingWorx Eclipse plugin's wizards did.
//!
//! Each writes the Java class or widget files together with the metadata.xml entry naming them,
//! so that `ext check` passes on the result. metadata.xml is edited as text: the entry goes into
//! its collection with the indentation already there, and nothing else in the file changes.

use super::ext_check::{declarations, jar_names};
use super::scan::escape_attribute;
use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};

/// One file a plan writes.
#[derive(Debug, Serialize)]
pub struct Write {
    pub path: String,
    /// `create`, or `update` for metadata.xml.
    pub action: &'static str,
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct Plan {
    #[serde(skip)]
    pub root: PathBuf,
    pub summary: String,
    pub files: Vec<Write>,
    pub notes: Vec<String>,
}

impl Plan {
    /// Write every file, creating the folders it needs.
    pub fn apply(&self) -> Result<(), String> {
        for write in &self.files {
            let path = self.root.join(&write.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            }
            std::fs::write(&path, &write.bytes)
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        }
        Ok(())
    }
}

pub struct NewOptions<'a> {
    pub name: &'a str,
    pub package: Option<&'a str>,
    pub vendor: Option<&'a str>,
    pub version: Option<&'a str>,
    pub min_thingworx: Option<&'a str>,
    pub description: Option<&'a str>,
    /// The Extension SDK zip or folder whose jars go into `twx-lib/`.
    pub sdk: Option<&'a Path>,
}

/// A new extension project in `root`, which must be missing or empty.
pub fn new_extension(root: &Path, options: &NewOptions) -> Result<Plan, String> {
    let name = options.name;
    if !is_entity_name(name) {
        return Err(format!(
            "{name:?} is not an extension name: start with a letter; letters, digits, `_`, `-` and `.`"
        ));
    }
    if root
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_some())
    {
        return Err(format!("{} is not empty", root.display()));
    }
    let vendor = options.vendor.unwrap_or("");
    let package = match options.package {
        Some(package) => {
            check_package(package)?;
            package.to_string()
        }
        None => format!(
            "com.{}.{}",
            identifier_part(if vendor.is_empty() { "example" } else { vendor }),
            identifier_part(name)
        ),
    };
    let version = options.version.unwrap_or("1.0.0");
    let min_thingworx = options.min_thingworx.unwrap_or("10.0.0");
    let attribute = |value: &str| {
        escape_attribute(value).map_err(|_| format!("{value:?} holds a character XML cannot"))
    };
    let metadata = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Entities>
    <ExtensionPackages>
        <ExtensionPackage name="{}" description="{}" vendor="{}" packageVersion="{}"
         minimumThingWorxVersion="{}" dependsOn="" haCompatible="false"/>
    </ExtensionPackages>
</Entities>
"#,
        attribute(name)?,
        attribute(options.description.unwrap_or(""))?,
        attribute(vendor)?,
        attribute(version)?,
        attribute(min_thingworx)?
    );
    let properties = format!(
        "# The version is metadata.xml's packageVersion; the build reads it from there.\n\
         build_framework=GRADLE\nproject_vendor={vendor}\nmin_thingworx_version={min_thingworx}\n"
    );
    let mut files = vec![
        create("settings.gradle", format!("rootProject.name = '{name}'\n")),
        create("build.gradle", BUILD_GRADLE.to_string()),
        create("extension.properties", properties),
        create("configfiles/metadata.xml", metadata),
        create(".gitignore", ".gradle/\nbuild/\ntwx-lib/\n".to_string()),
        create(
            &format!("src/main/java/{}/.gitkeep", package.replace('.', "/")),
            String::new(),
        ),
    ];
    let mut notes = Vec::new();
    match options.sdk {
        Some(sdk) => {
            for (jar, bytes) in sdk_jars(sdk)? {
                files.push(Write {
                    path: format!("twx-lib/{jar}"),
                    action: "create",
                    bytes,
                });
            }
        }
        None => notes.push(
            "no SDK given (--sdk or TWACO_SDK): copy the Extension SDK's jars into twx-lib/ \
             before building"
                .to_string(),
        ),
    }
    notes.push("run `gradle wrapper` once to add the Gradle wrapper".to_string());
    Ok(Plan {
        root: root.to_path_buf(),
        summary: format!("extension {name} {version} (Java package {package})"),
        files,
        notes,
    })
}

/// What `ext add` adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    ThingTemplate,
    ThingShape,
    Resource,
    Widget,
    Jar,
}

impl Kind {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "thing-template" => Self::ThingTemplate,
            "thing-shape" => Self::ThingShape,
            "resource" => Self::Resource,
            "widget" => Self::Widget,
            "jar" => Self::Jar,
            _ => return None,
        })
    }

    pub const WORDS: &'static str = "thing-template, thing-shape, resource, widget or jar";
}

pub struct AddOptions<'a> {
    pub kind: Kind,
    /// The entity or widget name; for a jar, the file to copy into `lib/`.
    pub name: &'a str,
    pub package: Option<&'a str>,
    /// A ThingTemplate's base template (default GenericThing).
    pub base: Option<&'a str>,
    pub description: Option<&'a str>,
}

/// Add one thing to the extension project at `root`.
pub fn add(root: &Path, options: &AddOptions) -> Result<Plan, String> {
    let (metadata_path, text) = read_metadata(root)?;
    let declared = declarations(text.as_bytes())
        .map_err(|error| format!("{metadata_path} is not well-formed XML: {error}"))?;
    let description = escape_attribute(options.description.unwrap_or(""))
        .map_err(|_| "the description holds a character XML cannot".to_string())?;
    let mut files = Vec::new();
    let mut notes = Vec::new();
    let mut metadata = text.clone();
    let name = options.name;
    let summary;
    match options.kind {
        Kind::Jar => {
            let source = Path::new(name);
            let file = source
                .file_name()
                .map(|file| file.to_string_lossy().into_owned())
                .filter(|file| file.to_ascii_lowercase().ends_with(".jar"))
                .ok_or_else(|| format!("{name} is not a .jar file"))?;
            if declared
                .iter()
                .any(|entry| entry.tag == "JarResources" && entry.files.contains(&file))
            {
                return Err(format!("{metadata_path} already lists {file}"));
            }
            let target = format!("lib/{file}");
            if !root.join(&target).is_file() {
                let bytes = std::fs::read(source)
                    .map_err(|error| format!("cannot read {name}: {error}"))?;
                files.push(Write {
                    path: target,
                    action: "create",
                    bytes,
                });
            }
            metadata = add_jar_resource(&metadata, &jar_entry(&file, &description)?)?;
            summary = format!("jar {file}");
        }
        Kind::Widget => {
            if !is_js_identifier(name) {
                return Err(format!(
                    "{name:?} is not a widget name: letters, digits and `_`, not starting with a digit"
                ));
            }
            if declared
                .iter()
                .any(|entry| entry.tag == "Widget" && entry.name == name)
            {
                return Err(format!("{metadata_path} already has a widget {name}"));
            }
            let folder = format!("ui/{name}");
            if root.join(&folder).exists() {
                return Err(format!("{folder} already exists"));
            }
            for (suffix, body) in [
                ("ide.js", WIDGET_IDE_JS),
                ("runtime.js", WIDGET_RUNTIME_JS),
                ("ide.css", WIDGET_CSS),
                ("runtime.css", WIDGET_CSS),
            ] {
                files.push(create(
                    &format!("{folder}/{name}.{suffix}"),
                    body.replace("NAME", name),
                ));
            }
            let entry = format!(
                "<Widget name=\"{name}\" description=\"{description}\">\n    <UIResources>\n\
                 {}    </UIResources>\n</Widget>",
                [
                    ("ide.js", "JS", true),
                    ("ide.css", "CSS", true),
                    ("runtime.js", "JS", false),
                    ("runtime.css", "CSS", false),
                ]
                .iter()
                .map(|(suffix, kind, ide)| {
                    format!(
                        "        <FileResource type=\"{kind}\" file=\"{name}.{suffix}\" \
                         description=\"\" isDevelopment=\"{ide}\" isRuntime=\"{}\"/>\n",
                        !ide
                    )
                })
                .collect::<String>()
            );
            metadata = insert_entry(&metadata, "Widgets", &entry)?;
            summary = format!("widget {name}");
        }
        Kind::ThingTemplate | Kind::ThingShape | Kind::Resource => {
            if !is_entity_name(name) {
                return Err(format!(
                    "{name:?} is not an entity name: start with a letter; letters, digits, `_`, `-` and `.`"
                ));
            }
            let tag = match options.kind {
                Kind::ThingTemplate => "ThingTemplate",
                Kind::ThingShape => "ThingShape",
                _ => "Resource",
            };
            if declared
                .iter()
                .any(|entry| entry.tag == tag && entry.name == name)
            {
                return Err(format!("{metadata_path} already has a {tag} {name}"));
            }
            let package = java_package(root, &declared, options.package)?;
            let class = java_class_name(name);
            let qualified = format!("{package}.{class}");
            let source_root = source_root(root);
            let java = format!("{source_root}/{}/{class}.java", package.replace('.', "/"));
            if root.join(&java).exists() {
                return Err(format!("{java} already exists"));
            }
            let source = match options.kind {
                Kind::ThingTemplate => {
                    let base = options.base.unwrap_or("GenericThing");
                    if !is_entity_name(base) {
                        return Err(format!("{base:?} is not a ThingTemplate name"));
                    }
                    let thing_package = thing_package_name(name);
                    if declared
                        .iter()
                        .any(|entry| entry.tag == "ThingPackage" && entry.name == thing_package)
                    {
                        return Err(format!(
                            "{metadata_path} already has a ThingPackage {thing_package}"
                        ));
                    }
                    metadata = insert_entry(
                        &metadata,
                        "ThingPackages",
                        &format!(
                            "<ThingPackage name=\"{thing_package}\" className=\"{qualified}\" \
                             description=\"{description}\"/>"
                        ),
                    )?;
                    metadata = insert_entry(
                        &metadata,
                        "ThingTemplates",
                        &format!(
                            "<ThingTemplate name=\"{name}\" thingPackage=\"{thing_package}\" \
                             description=\"{description}\"/>"
                        ),
                    )?;
                    format!(
                        "package {package};\n\n\
                         import com.thingworx.metadata.annotations.ThingworxBaseTemplateDefinition;\n\
                         import com.thingworx.things.Thing;\n\n\
                         @ThingworxBaseTemplateDefinition(name = \"{base}\")\n\
                         public class {class} extends Thing {{\n\n    \
                         private static final long serialVersionUID = 1L;\n}}\n"
                    )
                }
                Kind::ThingShape => {
                    metadata = insert_entry(
                        &metadata,
                        "ThingShapes",
                        &format!(
                            "<ThingShape name=\"{name}\" className=\"{qualified}\" \
                             description=\"{description}\"/>"
                        ),
                    )?;
                    format!("package {package};\n\npublic class {class} {{\n}}\n")
                }
                _ => {
                    metadata = insert_entry(
                        &metadata,
                        "Resources",
                        &format!(
                            "<Resource name=\"{name}\" className=\"{qualified}\" \
                             description=\"{description}\"/>"
                        ),
                    )?;
                    format!(
                        "package {package};\n\n\
                         import com.thingworx.resources.Resource;\n\n\
                         public class {class} extends Resource {{\n\n    \
                         private static final long serialVersionUID = 1L;\n}}\n"
                    )
                }
            };
            files.push(create(&java, source));
            // The first Java class needs the jar the build makes listed, or nothing loads it.
            let jars = jar_names(root);
            let mut listed = declared
                .iter()
                .filter(|entry| entry.tag == "JarResources")
                .flat_map(|entry| entry.files.iter());
            if !listed.any(|file| jars.built.contains(file)) {
                match own_jar(root) {
                    Some(jar) => {
                        metadata = add_jar_resource(&metadata, &jar_entry(&jar, "")?)?;
                        notes.push(format!("lists {jar}, the jar the build makes, in JarResources"));
                    }
                    None => notes.push(
                        "add the jar the build makes to JarResources: settings.gradle names no project"
                            .to_string(),
                    ),
                }
            }
            summary = format!("{tag} {name} ({qualified})");
        }
    }
    files.push(Write {
        path: metadata_path,
        action: "update",
        bytes: metadata.into_bytes(),
    });
    Ok(Plan {
        root: root.to_path_buf(),
        summary,
        files,
        notes,
    })
}

fn create(path: &str, text: String) -> Write {
    Write {
        path: path.to_string(),
        action: "create",
        bytes: text.into_bytes(),
    }
}

fn read_metadata(root: &Path) -> Result<(String, String), String> {
    for path in ["configfiles/metadata.xml", "metadata.xml"] {
        if let Ok(bytes) = std::fs::read(root.join(path)) {
            let text = String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8"))?;
            return Ok((path.to_string(), text));
        }
    }
    Err(format!(
        "{} has no configfiles/metadata.xml: not an extension project (twaco ext new makes one)",
        root.display()
    ))
}

/// The SDK's jars but its Javadoc, from its zip or the folder it was unpacked into.
fn sdk_jars(sdk: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let wanted = |name: &str| {
        let lower = name.to_ascii_lowercase();
        lower.ends_with(".jar") && !lower.contains("javadoc")
    };
    let mut out = Vec::new();
    if sdk.is_dir() {
        let entries = std::fs::read_dir(sdk)
            .map_err(|error| format!("cannot read {}: {error}", sdk.display()))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if wanted(&name) && entry.path().is_file() {
                let bytes = std::fs::read(entry.path())
                    .map_err(|error| format!("cannot read {name}: {error}"))?;
                out.push((name, bytes));
            }
        }
    } else {
        let file = std::fs::File::open(sdk)
            .map_err(|error| format!("cannot read {}: {error}", sdk.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|error| format!("{} is not the SDK zip: {error}", sdk.display()))?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            let Ok(name) = entry.name().map(|name| name.to_string()) else {
                continue;
            };
            if name.contains('/') || !wanted(&name) {
                continue;
            }
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|error| format!("cannot read {name} from the SDK: {error}"))?;
            out.push((name, bytes));
        }
    }
    if !out
        .iter()
        .any(|(name, _)| name.to_ascii_lowercase().contains("thingworx-ext-sdk"))
    {
        return Err(format!(
            "{} holds no thingworx-ext-sdk jar: give the Extension SDK zip or its folder",
            sdk.display()
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn jar_entry(file: &str, description: &str) -> Result<String, String> {
    let file = escape_attribute(file).map_err(|_| format!("{file:?} is not a file name"))?;
    Ok(format!(
        "<FileResource type=\"JAR\" file=\"{file}\" description=\"{description}\"/>"
    ))
}

/// The jar `build.gradle` makes: the project's name, lower-cased with spaces as `_`.
fn own_jar(root: &Path) -> Option<String> {
    let settings = std::fs::read_to_string(root.join("settings.gradle")).ok()?;
    let name = settings
        .lines()
        .find_map(|line| line.trim().strip_prefix("rootProject.name"))
        .and_then(|rest| rest.split(['\'', '"']).nth(1))?;
    Some(format!("{}.jar", name.to_lowercase().replace(' ', "_")))
}

/// `--package`, else the package of a class metadata.xml names, else the one folder chain under
/// the source root that `ext new` made.
fn java_package(
    root: &Path,
    declared: &[super::ext_check::Declared],
    given: Option<&str>,
) -> Result<String, String> {
    if let Some(package) = given {
        check_package(package)?;
        return Ok(package.to_string());
    }
    if let Some(package) = declared
        .iter()
        .filter_map(|entry| entry.attributes.get("className"))
        .find_map(|class| {
            class
                .rsplit_once('.')
                .map(|(package, _)| package.to_string())
        })
    {
        return Ok(package);
    }
    let base = root.join(source_root(root));
    let mut dir = base.clone();
    loop {
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .map(|entries| entries.flatten().collect())
            .unwrap_or_default();
        let dirs: Vec<_> = entries
            .iter()
            .filter(|entry| entry.path().is_dir())
            .collect();
        let has_java = entries
            .iter()
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "java"));
        if dirs.len() != 1 || has_java {
            break;
        }
        dir = dirs[0].path();
    }
    let package = dir
        .strip_prefix(&base)
        .ok()
        .map(|relative| {
            relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(".")
        })
        .unwrap_or_default();
    if package.is_empty() || check_package(&package).is_err() {
        return Err("give the Java package with --package".to_string());
    }
    Ok(package)
}

/// `src/main/java`, or `src` in a project laid out the older way.
fn source_root(root: &Path) -> &'static str {
    if !root.join("src/main/java").is_dir()
        && root.join("src").is_dir()
        && !root.join("src/main").is_dir()
    {
        "src"
    } else {
        "src/main/java"
    }
}

fn check_package(package: &str) -> Result<(), String> {
    let ok = package.split('.').all(|part| {
        part.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    });
    if ok {
        Ok(())
    } else {
        Err(format!("{package:?} is not a Java package"))
    }
}

/// A Java identifier for an entity name: what is not a letter, digit or `_` becomes `_`.
fn java_class_name(name: &str) -> String {
    let class: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if class.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{class}")
    } else {
        class
    }
}

/// A lower-case package segment.
fn identifier_part(text: &str) -> String {
    let part: String = text
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    match part.chars().next() {
        None => "example".to_string(),
        Some(c) if c.is_ascii_digit() => format!("x{part}"),
        Some(_) => part,
    }
}

/// `Acme.Pump_TT` -> `Acme.Pump_TP`; any other name gets `_TP` appended.
fn thing_package_name(template: &str) -> String {
    match template.strip_suffix("_TT") {
        Some(stem) => format!("{stem}_TP"),
        None => format!("{template}_TP"),
    }
}

fn is_entity_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn is_js_identifier(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// The start of the line holding the tag at `at` and its indentation, when only whitespace
/// precedes the tag on that line.
fn line_indent(text: &str, at: usize) -> Option<(usize, &str)> {
    let line_start = text[..at].rfind('\n').map_or(0, |index| index + 1);
    let before = &text[line_start..at];
    before
        .chars()
        .all(char::is_whitespace)
        .then_some((line_start, before))
}

/// The indentation of the last child before the closing tag on the line at `line_start`, or
/// four spaces deeper than that tag when there is none.
fn child_indent(text: &str, line_start: usize, indent: &str) -> String {
    let previous = text[..line_start]
        .trim_end_matches(['\r', '\n'])
        .rsplit('\n')
        .next()
        .unwrap_or("");
    let own = &previous[..previous.len() - previous.trim_start().len()];
    if own.len() > indent.len() && own.starts_with(indent) {
        own.to_string()
    } else {
        format!("{indent}    ")
    }
}

/// `entry` with every line indented by `indent`.
fn indented(entry: &str, indent: &str, nl: &str) -> String {
    entry
        .lines()
        .map(|line| format!("{indent}{line}{nl}"))
        .collect()
}

/// The end of the tag starting at `at`: the index just past its `>`.
fn tag_end(text: &str, at: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, c) in text[at..].char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '>') => return Some(at + offset + 1),
            _ => {}
        }
    }
    None
}

/// The start of the first `<tag` element (not one whose name merely begins with `tag`).
fn find_start(text: &str, tag: &str) -> Option<usize> {
    let open = format!("<{tag}");
    let mut from = 0;
    while let Some(found) = text[from..].find(&open) {
        let at = from + found;
        let next = text[at + open.len()..].chars().next();
        if next.is_some_and(|c| c.is_whitespace() || c == '>' || c == '/') {
            return Some(at);
        }
        from = at + open.len();
    }
    None
}

/// Put `entry` last in the `collection` element, creating it before `</Entities>` when absent.
fn insert_entry(text: &str, collection: &str, entry: &str) -> Result<String, String> {
    insert_into(text, collection, entry, "Entities")
}

/// Put a FileResource last in the ExtensionPackage's JarResources, creating that when absent.
fn add_jar_resource(text: &str, entry: &str) -> Result<String, String> {
    if find_start(text, "JarResources").is_some() {
        return insert_into(text, "JarResources", entry, "ExtensionPackage");
    }
    let at = find_start(text, "ExtensionPackage")
        .ok_or_else(|| "metadata.xml has no ExtensionPackage".to_string())?;
    let end = tag_end(text, at).ok_or_else(|| "metadata.xml ends inside a tag".to_string())?;
    if text[..end].ends_with("/>") {
        // A self-closing ExtensionPackage gets a body.
        let nl = newline(text);
        let indent = line_indent(text, at).map_or("", |(_, indent)| indent);
        let inner = format!("{indent}    ");
        let body = format!(
            ">{nl}{inner}<JarResources>{nl}{}{inner}</JarResources>{nl}{indent}</ExtensionPackage>",
            indented(entry, &format!("{inner}    "), nl)
        );
        let close = end - 2;
        let close = text[..close].trim_end().len();
        return Ok(format!("{}{body}{}", &text[..close], &text[end..]));
    }
    insert_into(text, "JarResources", entry, "ExtensionPackage")
}

fn insert_into(text: &str, collection: &str, entry: &str, parent: &str) -> Result<String, String> {
    let nl = newline(text);
    if let Some(at) = text.find(&format!("</{collection}>")) {
        return Ok(match line_indent(text, at) {
            // The closing tag starts its own line: whole lines go in before it.
            Some((line_start, indent)) => format!(
                "{}{}{}",
                &text[..line_start],
                indented(entry, &child_indent(text, line_start, indent), nl),
                &text[line_start..]
            ),
            None => format!(
                "{}{nl}{}{}",
                &text[..at],
                indented(entry, "    ", nl),
                &text[at..]
            ),
        });
    }
    if let Some(at) = find_start(text, collection) {
        let end = tag_end(text, at).ok_or_else(|| "metadata.xml ends inside a tag".to_string())?;
        if text[..end].ends_with("/>") {
            let indent = line_indent(text, at).map_or("", |(_, indent)| indent);
            let body = format!(
                "<{collection}>{nl}{}{indent}</{collection}>",
                indented(entry, &format!("{indent}    "), nl)
            );
            return Ok(format!("{}{body}{}", &text[..at], &text[end..]));
        }
    }
    let at = text
        .rfind(&format!("</{parent}>"))
        .ok_or_else(|| format!("metadata.xml has no </{parent}>"))?;
    let (split, indent, lead) = match line_indent(text, at) {
        Some((line_start, indent)) => (line_start, indent, ""),
        None => (at, "", nl),
    };
    let inner = format!("{indent}    ");
    Ok(format!(
        "{}{lead}{inner}<{collection}>{nl}{}{inner}</{collection}>{nl}{}",
        &text[..split],
        indented(entry, &format!("{inner}    "), nl),
        &text[split..]
    ))
}

const BUILD_GRADLE: &str = r#"// Builds the extension zip to import into ThingWorx: build/distributions/<project>.zip.
// The version and vendor come from configfiles/metadata.xml.
plugins {
    id 'java'
}

def extensionPackage = new groovy.xml.XmlSlurper()
    .parse(file('configfiles/metadata.xml'))
    .ExtensionPackages.ExtensionPackage[0]
def jarName = rootProject.name.toLowerCase().replace(' ', '_')
def hasJava = !fileTree('src/main/java').matching { include '**/*.java' }.isEmpty()

version = extensionPackage.@packageVersion.text()

dependencies {
    compileOnly fileTree(dir: 'twx-lib', include: '*.jar')
    implementation fileTree(dir: 'lib', include: '*.jar')
}

tasks.withType(JavaCompile).configureEach {
    options.release = 11
    options.encoding = 'UTF-8'
}

jar {
    archiveFileName = "${jarName}.jar"
    manifest {
        attributes(
            'Package-Title': extensionPackage.@name.text(),
            'Package-Version': version,
            'Package-Vendor': extensionPackage.@vendor.text())
    }
}

def packageExtension = tasks.register('packageExtension', Zip) {
    group = 'build'
    description = 'The extension zip to import into ThingWorx.'
    archiveFileName = "${rootProject.name}.zip"
    destinationDirectory = layout.buildDirectory.dir('distributions')
    from('configfiles') { include 'metadata.xml' }
    if (hasJava) {
        from(jar) { into 'lib/common' }
    }
    from('lib') { include '*.jar'; into 'lib/common' }
    from('Entities') { into 'Entities' }
    from('Localization') { into 'Localization' }
    from('ui') { into 'ui' }
}

build.finalizedBy(packageExtension)
"#;

const WIDGET_IDE_JS: &str = r#"TW.IDE.Widgets.NAME = function () {
    this.widgetProperties = function () {
        return {
            name: 'NAME',
            description: '',
            category: ['Common'],
            supportsAutoResize: true,
            properties: {
                Text: {
                    baseType: 'STRING',
                    defaultValue: '',
                    isBindingTarget: true
                }
            }
        };
    };

    this.renderHtml = function () {
        return '<div class="widget-content widget-NAME"></div>';
    };

    this.afterRender = function () {
        this.jqElement.text(this.getProperty('Text') || 'NAME');
    };
};
"#;

const WIDGET_RUNTIME_JS: &str = r#"TW.Runtime.Widgets.NAME = function () {
    this.renderHtml = function () {
        return '<div class="widget-content widget-NAME"></div>';
    };

    this.afterRender = function () {
        this.jqElement.text(this.getProperty('Text') || '');
    };

    this.updateProperty = function (updatePropertyInfo) {
        if (updatePropertyInfo.TargetProperty === 'Text') {
            this.setProperty('Text', updatePropertyInfo.SinglePropertyValue);
            this.jqElement.text(updatePropertyInfo.SinglePropertyValue || '');
        }
    };
};
"#;

const WIDGET_CSS: &str = ".widget-NAME {\n}\n";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ext_check;
    use std::io::Write as _;

    fn options(name: &str) -> NewOptions<'_> {
        NewOptions {
            name,
            package: None,
            vendor: Some("Acme"),
            version: None,
            min_thingworx: None,
            description: None,
            sdk: None,
        }
    }

    fn add_one(root: &Path, kind: Kind, name: &str) -> Plan {
        let plan = add(
            root,
            &AddOptions {
                kind,
                name,
                package: None,
                base: None,
                description: Some("a <test>"),
            },
        )
        .unwrap();
        plan.apply().unwrap();
        plan
    }

    #[test]
    fn a_new_project_with_one_of_everything_passes_ext_check() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("Acme_Pumps");
        let plan = new_extension(&root, &options("Acme_Pumps")).unwrap();
        assert!(plan.notes.iter().any(|note| note.contains("twx-lib")));
        plan.apply().unwrap();
        assert!(root
            .join("src/main/java/com/acme/acmepumps/.gitkeep")
            .is_file());
        assert!(ext_check::check(&root).findings.is_empty());

        let template = add_one(&root, Kind::ThingTemplate, "Acme.Pump_TT");
        assert!(template
            .notes
            .iter()
            .any(|note| note.contains("acme_pumps.jar")));
        add_one(&root, Kind::ThingShape, "Acme.Valve_TS");
        add_one(&root, Kind::Resource, "Acme.Tools");
        add_one(&root, Kind::Widget, "gauge");
        let jar = temporary.path().join("helper-1.0.jar");
        std::fs::write(&jar, b"jar").unwrap();
        add_one(&root, Kind::Jar, jar.to_str().unwrap());

        let report = ext_check::check(&root);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(report.classes, 3);
        let metadata = std::fs::read_to_string(root.join("configfiles/metadata.xml")).unwrap();
        for wanted in [
            r#"<ThingPackage name="Acme.Pump_TP" className="com.acme.acmepumps.Acme_Pump_TT" description="a &lt;test&gt;"/>"#,
            r#"<ThingTemplate name="Acme.Pump_TT" thingPackage="Acme.Pump_TP""#,
            r#"<ThingShape name="Acme.Valve_TS" className="com.acme.acmepumps.Acme_Valve_TS""#,
            r#"<Resource name="Acme.Tools" className="com.acme.acmepumps.Acme_Tools""#,
            r#"<FileResource type="JAR" file="acme_pumps.jar" description=""/>"#,
            r#"<FileResource type="JAR" file="helper-1.0.jar""#,
            r#"<FileResource type="JS" file="gauge.runtime.js""#,
        ] {
            assert!(metadata.contains(wanted), "{wanted} in\n{metadata}");
        }
        assert!(root.join("lib/helper-1.0.jar").is_file());
        assert!(root.join("ui/gauge/gauge.ide.js").is_file());
        let java = std::fs::read_to_string(
            root.join("src/main/java/com/acme/acmepumps/Acme_Pump_TT.java"),
        )
        .unwrap();
        assert!(java.contains("@ThingworxBaseTemplateDefinition(name = \"GenericThing\")"));
        // The own jar is listed once, not again for the second class.
        assert_eq!(metadata.matches("acme_pumps.jar").count(), 1);
    }

    #[test]
    fn refusals_name_what_is_in_the_way() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("README.md"), "x").unwrap();
        let error = new_extension(temporary.path(), &options("Acme")).unwrap_err();
        assert!(error.contains("not empty"), "{error}");
        assert!(new_extension(&temporary.path().join("x"), &options("1bad"))
            .unwrap_err()
            .contains("not an extension name"));

        let root = temporary.path().join("Acme");
        new_extension(&root, &options("Acme"))
            .unwrap()
            .apply()
            .unwrap();
        add_one(&root, Kind::ThingShape, "Acme.Shape");
        let again = AddOptions {
            kind: Kind::ThingShape,
            name: "Acme.Shape",
            package: None,
            base: None,
            description: None,
        };
        assert!(add(&root, &again)
            .unwrap_err()
            .contains("already has a ThingShape"));
        let widget = AddOptions {
            kind: Kind::Widget,
            name: "my-widget",
            ..again
        };
        assert!(add(&root, &widget)
            .unwrap_err()
            .contains("not a widget name"));
        let error = add(temporary.path(), &again).unwrap_err();
        assert!(error.contains("not an extension project"), "{error}");
    }

    #[test]
    fn entries_go_into_existing_collections_with_their_indentation_and_line_endings() {
        let text = "<Entities>\r\n  <ExtensionPackages>\r\n    <ExtensionPackage name=\"A\" packageVersion=\"1\"/>\r\n  </ExtensionPackages>\r\n  <Widgets/>\r\n  <ThingShapes>\r\n    <ThingShape name=\"S\" className=\"a.S\"/>\r\n  </ThingShapes>\r\n</Entities>\r\n";
        let shapes = insert_entry(text, "ThingShapes", "<ThingShape name=\"T\"/>").unwrap();
        assert!(shapes.contains(
            "    <ThingShape name=\"S\" className=\"a.S\"/>\r\n    <ThingShape name=\"T\"/>\r\n  </ThingShapes>"
        ), "{shapes}");
        let widgets = insert_entry(text, "Widgets", "<Widget name=\"w\"/>").unwrap();
        assert!(
            widgets.contains("  <Widgets>\r\n      <Widget name=\"w\"/>\r\n  </Widgets>\r\n"),
            "{widgets}"
        );
        let resources = insert_entry(text, "Resources", "<Resource name=\"R\"/>").unwrap();
        assert!(resources.ends_with("  </ThingShapes>\r\n    <Resources>\r\n        <Resource name=\"R\"/>\r\n    </Resources>\r\n</Entities>\r\n"), "{resources}");
        let jar = add_jar_resource(text, "<FileResource file=\"a.jar\"/>").unwrap();
        assert!(jar.contains("<ExtensionPackage name=\"A\" packageVersion=\"1\">\r\n        <JarResources>\r\n            <FileResource file=\"a.jar\"/>\r\n        </JarResources>\r\n    </ExtensionPackage>\r\n"), "{jar}");
        assert!(ext_check_parses(&jar));
    }

    fn ext_check_parses(text: &str) -> bool {
        declarations(text.as_bytes()).is_ok()
    }

    #[test]
    fn the_sdk_gives_its_jars_but_the_javadoc() {
        let temporary = tempfile::tempdir().unwrap();
        let zip_path = temporary.path().join("sdk.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        for name in [
            "thingworx-ext-sdk-10.2.0-b1.jar",
            "thingworx-javadocs-10.2.0-b1.jar",
            "slf4j-api-2.0.17.jar",
            "Common/",
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"x").unwrap();
        }
        zip.finish().unwrap();
        let names: Vec<String> = sdk_jars(&zip_path)
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            names,
            ["slf4j-api-2.0.17.jar", "thingworx-ext-sdk-10.2.0-b1.jar"]
        );
        let folder = temporary.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("slf4j-api-2.0.17.jar"), b"x").unwrap();
        assert!(sdk_jars(&folder)
            .unwrap_err()
            .contains("no thingworx-ext-sdk jar"));
    }
}
