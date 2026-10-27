//! Widget extension projects driven by package.json, laid out as the ThingWorx widget template
//! (ptc-iot-sharing/ThingworxDemoWebpackWidget) lays them out.
//!
//! package.json is the source of truth: `packageName` names the extension package and its
//! widget, `version`, `author`, `description` and `minimumThingWorxVersion` fill the
//! metadata.xml the build generates, and `npm run build` writes the package zip under `zip/`.
//! `ext new --widget` lays out one with TypeScript widgets written against
//! `typescriptwebpacksupport` and a small esbuild script; `ext check`, `ext build` and
//! `ext deploy` read such a project whichever script builds it.

use super::ext_check::{Finding, Report};
use super::ext_new::{create, NewOptions, Plan, Write};
use serde_json::Value;
use std::path::Path;

/// A package.json widget project: what it calls its package and its version, and which package
/// manager runs its scripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NpmProject {
    pub(crate) package: String,
    pub(crate) version: String,
    pub(crate) runner: &'static str,
}

/// The project at `root`, when it is a package.json widget project: a package.json, and no
/// metadata.xml naming a package (the template's own metadata.xml leaves the name empty).
pub(crate) fn read(root: &Path) -> Option<Result<NpmProject, String>> {
    let text = std::fs::read_to_string(root.join("package.json")).ok()?;
    if names_a_package(&root.join("configfiles/metadata.xml"))
        || names_a_package(&root.join("metadata.xml"))
    {
        return None;
    }
    Some(parse(&text).map(|(package, version)| NpmProject {
        package,
        version,
        runner: runner(root),
    }))
}

fn names_a_package(metadata: &Path) -> bool {
    std::fs::read(metadata)
        .ok()
        .and_then(|bytes| super::ext_check::declarations(&bytes).ok())
        .is_some_and(|declared| {
            declared
                .iter()
                .any(|item| item.tag == "ExtensionPackage" && !item.name.is_empty())
        })
}

/// The package name and version package.json gives, as the template reads them: `packageName`,
/// else `<name>_ExtensionPackage`.
fn parse(text: &str) -> Result<(String, String), String> {
    let json: Value =
        serde_json::from_str(text).map_err(|error| format!("package.json: {error}"))?;
    let field = |key: &str| json.get(key).and_then(Value::as_str).map(str::to_string);
    let package = field("packageName")
        .or_else(|| field("name").map(|name| format!("{name}_ExtensionPackage")))
        .ok_or("package.json names no packageName or name")?;
    let version = field("version").ok_or("package.json has no version")?;
    Ok((package, version))
}

/// The package manager the lock file says, else npm.
fn runner(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else {
        "npm"
    }
}

/// Write `version` as package.json's top-level `version`, changing nothing else in the file.
pub(crate) fn set_version(root: &Path, version: &str) -> Result<(), String> {
    let path = root.join("package.json");
    let text = std::fs::read_to_string(&path).map_err(|error| format!("package.json: {error}"))?;
    let span = top_level_string(&text, "version")
        .ok_or("package.json has no top-level \"version\" string")?;
    let edited = format!("{}{version}{}", &text[..span.0], &text[span.1..]);
    super::ext_new::replace_whole(&path, edited.as_bytes())
}

/// The byte span of the text of `key`'s string value in `text`'s top-level object.
fn top_level_string(text: &str, key: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut at = 0;
    // The key last read at depth 1, waiting for its value.
    let mut pending: Option<String> = None;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => {
                let start = at + 1;
                let end = string_end(bytes, start)?;
                let value = &text[start..end];
                at = end + 1;
                if depth != 1 {
                    continue;
                }
                let next = text[at..].trim_start();
                if next.starts_with(':') {
                    pending = Some(value.to_string());
                } else if pending.take().as_deref() == Some(key) {
                    return Some((start, end));
                }
                continue;
            }
            b'{' | b'[' => {
                depth += 1;
                pending = None;
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            b',' => pending = None,
            _ => {}
        }
        at += 1;
    }
    None
}

fn string_end(bytes: &[u8], mut at: usize) -> Option<usize> {
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return Some(at),
            _ => at += 1,
        }
    }
    None
}

/// `ext check` for a package.json widget project.
pub(crate) fn check(root: &Path) -> Report {
    let mut report = Report::default();
    let finding = |rule: &'static str, message: String| Finding {
        file: "package.json".into(),
        line: 0,
        rule,
        message,
    };
    let text = match std::fs::read_to_string(root.join("package.json")) {
        Ok(text) => text,
        Err(error) => {
            report
                .findings
                .push(finding("package-unreadable", error.to_string()));
            return report;
        }
    };
    let json: Value = match serde_json::from_str(&text) {
        Ok(json) => json,
        Err(error) => {
            report
                .findings
                .push(finding("package-unreadable", format!("not JSON: {error}")));
            return report;
        }
    };
    let field = |key: &str| json.get(key).and_then(Value::as_str);
    match parse(&text) {
        Ok((package, version)) => {
            if !is_package_name(&package) {
                report.findings.push(finding(
                    "package-name",
                    format!("{package:?} cannot name a widget package: letters, digits and `_`, not starting with a digit"),
                ));
            }
            if !is_version(&version) {
                report.findings.push(finding(
                    "version",
                    format!("version {version:?} is not major.minor.patch"),
                ));
            }
            report.package = Some(package);
            report.version = Some(version);
        }
        Err(why) => report.findings.push(finding("package-unreadable", why)),
    }
    if json
        .pointer("/scripts/build")
        .and_then(Value::as_str)
        .is_none()
    {
        report.findings.push(finding(
            "no-build-script",
            "package.json has no scripts.build, which ext build and ext deploy run".into(),
        ));
    }
    if field("minimumThingWorxVersion").is_none() {
        report.notes.push(
            "package.json names no minimumThingWorxVersion; the package will not say which ThingWorx it needs".into(),
        );
    }
    // The entry points the scaffold's build script bundles; the template's webpack names the same.
    for entry in ["src/ide/index.ts", "src/runtime/index.ts"] {
        if root.join("build.mjs").is_file() && !root.join(entry).is_file() {
            report.findings.push(Finding {
                file: entry.into(),
                line: 0,
                rule: "missing-entry",
                message: format!("build.mjs bundles {entry}, which is not there"),
            });
        }
    }
    if !root.join("node_modules").is_dir() {
        report.notes.push(format!(
            "no node_modules yet: `{} install` (ext build runs it)",
            runner(root)
        ));
    }
    report
}

pub(crate) fn is_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// A new widget project in `root`, which must be missing or empty. `name` is the package name and
/// the widget's.
pub fn new_widget(root: &Path, options: &NewOptions) -> Result<Plan, String> {
    let name = options.name;
    if !is_package_name(name) {
        return Err(format!(
            "{name:?} is not a widget package name: letters, digits and `_`, not starting with a digit"
        ));
    }
    if root
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_some())
    {
        return Err(format!("{} is not empty", root.display()));
    }
    let version = options.version.unwrap_or("1.0.0");
    let min_thingworx = options.min_thingworx.unwrap_or("9.0.0");
    for (value, what) in [
        (version, "--package-version"),
        (min_thingworx, "--min-thingworx"),
    ] {
        if !is_version(value) {
            return Err(format!("{what} {value:?} is not major.minor.patch"));
        }
    }
    let json = |text: &str| serde_json::to_string(text).expect("a string serialises");
    let npm_name: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let css = name.to_ascii_lowercase();
    let fill = |template: &str| template.replace("NAME", name).replace("CSSCLASS", &css);
    let package_json = PACKAGE_JSON
        .replace("\"NPMNAME\"", &json(&npm_name))
        .replace("\"PACKAGENAME\"", &json(name))
        .replace("\"VERSION\"", &json(version))
        .replace(
            "\"DESCRIPTION\"",
            &json(options.description.unwrap_or("A ThingWorx widget")),
        )
        .replace("\"VENDOR\"", &json(options.vendor.unwrap_or("")))
        .replace("\"MINTHINGWORX\"", &json(min_thingworx));
    let files: Vec<Write> = vec![
        create("package.json", package_json),
        create("tsconfig.json", TSCONFIG.to_string()),
        create("build.mjs", BUILD_SCRIPT.to_string()),
        create(".gitignore", "node_modules/\nbuild/\nzip/\n".to_string()),
        create("src/ide/index.ts", fill(IDE_TS)),
        create("src/ide/ide.css", fill(WIDGET_CSS)),
        create("src/runtime/index.ts", fill(RUNTIME_TS)),
        create("src/runtime/runtime.css", fill(WIDGET_CSS)),
        create("src/static/.gitkeep", String::new()),
        create("Entities/.gitkeep", String::new()),
    ];
    Ok(Plan {
        root: root.to_path_buf(),
        summary: format!("widget extension {name} {version}"),
        files,
        notes: vec![
            "`npm install` once (twaco ext build runs it), then `npm run build` or `twaco ext build`: the zip lands in zip/".to_string(),
        ],
    })
}

const PACKAGE_JSON: &str = r#"{
    "name": "NPMNAME",
    "packageName": "PACKAGENAME",
    "version": "VERSION",
    "description": "DESCRIPTION",
    "author": "VENDOR",
    "minimumThingWorxVersion": "MINTHINGWORX",
    "private": true,
    "type": "module",
    "scripts": {
        "build": "node build.mjs",
        "buildDev": "node build.mjs --dev",
        "check": "tsc --noEmit"
    },
    "devDependencies": {
        "@types/jquery": "3.5.34",
        "esbuild": "0.28.2",
        "fflate": "0.8.3",
        "typescript": "5.9.3"
    },
    "dependencies": {
        "typescriptwebpacksupport": "2.4.1"
    }
}
"#;

const TSCONFIG: &str = r#"{
    "compilerOptions": {
        "target": "es2017",
        "module": "esnext",
        "moduleResolution": "bundler",
        "experimentalDecorators": true,
        "strict": false,
        "noEmit": true,
        "lib": ["es2017", "dom"],
        "types": ["jquery"]
    },
    "include": ["src"]
}
"#;

const IDE_TS: &str = r#"import './ide.css';
import {
    TWWidgetDefinition,
    autoResizable,
    description,
    property,
    defaultValue,
    bindingTarget,
    event,
} from 'typescriptwebpacksupport/widgetIDESupport';

// The class name is the widget's name in the mashup builder, and must match the runtime class.
@description('A widget that shows a value.')
@TWWidgetDefinition('NAME', autoResizable)
class NAME extends TWComposerWidget {
    @property('NUMBER', defaultValue(120)) Width: number;

    @property('NUMBER', defaultValue(40)) Height: number;

    @description('The text the widget shows.')
    @property('STRING', bindingTarget, defaultValue('NAME'))
    Value: string;

    @description('Triggered when the widget is clicked.')
    @event
    Clicked;

    widgetIconUrl(): string {
        return '';
    }

    renderHtml(): string {
        return `<div class="widget-content CSSCLASS">${this.Value ?? ''}</div>`;
    }

    afterRender(): void {}

    beforeDestroy(): void {}
}
"#;

const RUNTIME_TS: &str = r#"import './runtime.css';
import {
    TWWidgetDefinition,
    property,
    event,
    TWEvent,
} from 'typescriptwebpacksupport/widgetRuntimeSupport';

@TWWidgetDefinition
class NAME extends TWRuntimeWidget {
    @property Value: string;

    @event Clicked: TWEvent;

    renderHtml(): string {
        return '<div class="widget-content CSSCLASS"></div>';
    }

    async afterRender(): Promise<void> {
        this.jqElement.text(this.Value ?? '');
        this.jqElement.on('click', () => this.Clicked());
    }

    updateProperty(info: TWUpdatePropertyInfo): void {
        if (info.TargetProperty === 'Value') {
            this.jqElement.text(info.SinglePropertyValue ?? '');
        }
    }

    beforeDestroy(): void {
        this.jqElement.off('click');
    }
}
"#;

const WIDGET_CSS: &str = r#".CSSCLASS {
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px dashed #888;
}
"#;

const BUILD_SCRIPT: &str = include_str!("ext_widget/build.mjs");

#[cfg(test)]
mod tests;
