//! Design tokens and the native StyleTheme variable mapping.
use crate::core::config::Solution;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const PRESETS: &[&str] = &["convergence", "clean", "brutalist"];
const MAPPING: &str = include_str!("mapping.txt");

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tokens {
    version: u32,
    pub name: String,
    #[serde(skip_serializing)]
    extends: Option<String>,
    #[serde(default)]
    colors: BTreeMap<String, String>,
    #[serde(default)]
    roles: BTreeMap<String, String>,
    #[serde(default)]
    fonts: BTreeMap<String, String>,
    #[serde(default)]
    radius: BTreeMap<String, String>,
    #[serde(default)]
    border: BTreeMap<String, String>,
    #[serde(default)]
    shadow: BTreeMap<String, String>,
    #[serde(default)]
    space: BTreeMap<String, String>,
    #[serde(default)]
    chart: Vec<String>,
    #[serde(default)]
    vars: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) block: BTreeMap<String, String>,
    #[serde(skip)]
    origins: BTreeMap<String, (String, u64, u64)>,
    #[serde(skip)]
    pub(crate) inputs: BTreeMap<PathBuf, Vec<u8>>,
}

pub(crate) fn is_preset(name: &str) -> bool {
    PRESETS.contains(&name)
}

/// `path` with `.` and `..` resolved lexically, so a folder that does not exist yet (a generated
/// cell's) can still lead to a token file on every platform.
pub(crate) fn lexical(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn preset(name: &str) -> Option<&'static str> {
    Some(match name {
        "convergence" => include_str!("convergence.yaml"),
        "clean" => include_str!("clean.yaml"),
        "brutalist" => include_str!("brutalist.yaml"),
        _ => return None,
    })
}

impl Tokens {
    pub(crate) fn load(source: &str, base: &Path) -> Result<Self, String> {
        fn read(source: &str, base: &Path, seen: &mut BTreeSet<String>) -> Result<Tokens, String> {
            let path = lexical(&base.join(source));
            let (text, identity, parent) = if let Some(text) = preset(source) {
                (text.to_string(), source.to_string(), base.to_path_buf())
            } else {
                let path = std::fs::canonicalize(&path).map_err(|e| {
                    format!(
                        "{}: {e}; use a token file or {}",
                        path.display(),
                        PRESETS.join(", ")
                    )
                })?;
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                (
                    text,
                    path.display().to_string(),
                    path.parent().unwrap().to_path_buf(),
                )
            };
            if !seen.insert(identity.clone()) {
                return Err(format!(
                    "{identity}: extends cycle; remove the circular extends"
                ));
            }
            let mut tokens: Tokens =
                serde_saphyr::from_str(&text).map_err(|e| format!("{identity}: {e}"))?;
            if preset(source).is_none() {
                tokens
                    .inputs
                    .insert(PathBuf::from(&identity), text.as_bytes().to_vec());
            }
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Locations {
                version: Option<serde_saphyr::Spanned<u32>>,
                name: Option<serde_saphyr::Spanned<String>>,
                extends: Option<serde_saphyr::Spanned<String>>,
                chart: Vec<serde_saphyr::Spanned<String>>,
                colors: BTreeMap<String, serde_saphyr::Spanned<String>>,
                roles: BTreeMap<String, serde_saphyr::Spanned<String>>,
                fonts: BTreeMap<String, serde_saphyr::Spanned<String>>,
                radius: BTreeMap<String, serde_saphyr::Spanned<String>>,
                border: BTreeMap<String, serde_saphyr::Spanned<String>>,
                shadow: BTreeMap<String, serde_saphyr::Spanned<String>>,
                space: BTreeMap<String, serde_saphyr::Spanned<String>>,
                vars: BTreeMap<String, serde_saphyr::Spanned<String>>,
                block: BTreeMap<String, serde_saphyr::Spanned<String>>,
            }
            let locations: Locations =
                serde_saphyr::from_str(&text).map_err(|e| format!("{identity}: {e}"))?;
            for (key, location) in [
                ("version", locations.version.as_ref().map(|v| v.referenced)),
                ("name", locations.name.as_ref().map(|v| v.referenced)),
                ("extends", locations.extends.as_ref().map(|v| v.referenced)),
            ] {
                if let Some(at) = location {
                    tokens
                        .origins
                        .insert(key.to_string(), (identity.clone(), at.line(), at.column()));
                }
            }
            for (i, val) in locations.chart.iter().enumerate() {
                tokens.origins.insert(
                    format!("chart.{}", i + 1),
                    (
                        identity.clone(),
                        val.referenced.line(),
                        val.referenced.column(),
                    ),
                );
            }
            for (section, map) in [
                ("colors", locations.colors),
                ("roles", locations.roles),
                ("fonts", locations.fonts),
                ("radius", locations.radius),
                ("border", locations.border),
                ("shadow", locations.shadow),
                ("space", locations.space),
                ("vars", locations.vars),
                ("block", locations.block),
            ] {
                for (key, value) in map {
                    tokens.origins.insert(
                        format!("{section}.{key}"),
                        (
                            identity.clone(),
                            value.referenced.line(),
                            value.referenced.column(),
                        ),
                    );
                }
            }
            for (section, map, allowed) in [
                (
                    "roles",
                    &tokens.roles,
                    &[
                        "primary",
                        "success",
                        "warning",
                        "danger",
                        "info",
                        "background",
                        "surface",
                        "surface-alt",
                        "text",
                        "text-muted",
                        "heading",
                        "line",
                        "on-primary",
                        "on-dark",
                        "selected",
                        "card",
                        "page-card",
                        "band",
                        "card-line",
                        "panel-line",
                        "nav-selected",
                        "nav-text",
                        "nav-caption",
                        "input-focus",
                    ][..],
                ),
                ("fonts", &tokens.fonts, &["sans", "mono", "display"][..]),
                (
                    "radius",
                    &tokens.radius,
                    &["none", "sm", "md", "lg", "default", "card"][..],
                ),
                (
                    "border",
                    &tokens.border,
                    &["width", "color", "line", "card", "band"][..],
                ),
                ("space", &tokens.space, &["xs", "sm", "md", "lg", "xl"][..]),
                ("block", &tokens.block, &["border", "shadow", "radius"][..]),
            ] {
                if let Some(key) = map.keys().find(|key| !allowed.contains(&key.as_str())) {
                    return Err(tokens.located(
                        &format!("{section}.{key}"),
                        &format!("unknown field {key:?}; use {}", allowed.join(", ")),
                    ));
                }
            }
            if tokens.version != 1 || tokens.name.is_empty() {
                let key = if tokens.version != 1 {
                    "version"
                } else {
                    "name"
                };
                return Err(tokens.located(key, "version must be 1 and name must be nonempty"));
            }
            if let Some(source) = tokens.extends.take() {
                let mut inherited =
                    read(&source, &parent, seen).map_err(|e| tokens.located("extends", &e))?;
                macro_rules! overlay {
                    ($($field:ident),*) => {$(inherited.$field.extend(tokens.$field);)*};
                }
                overlay!(
                    colors, roles, fonts, radius, border, shadow, space, vars, block, origins,
                    inputs
                );
                if !tokens.chart.is_empty() {
                    inherited.chart = tokens.chart;
                }
                inherited.name = tokens.name;
                tokens = inherited;
            }
            seen.remove(&identity);
            Ok(tokens)
        }
        let mut tokens = read(source, base, &mut BTreeSet::new())?;
        tokens.resolve().map_err(|e| format!("{source}: {e}"))?;
        Ok(tokens)
    }

    fn located(&self, key: &str, error: &str) -> String {
        match self.origins.get(key) {
            Some((file, line, column)) => format!("{file}:{line}:{column}: {key}: {error}"),
            None => format!("{key}: {error}"),
        }
    }

    pub(crate) fn active(solution: &Solution) -> Result<Self, String> {
        let source = solution.mashup.tokens.as_deref().unwrap_or("convergence");
        if source == "convergence" {
            return Ok(Self::convergence().clone());
        }
        Self::load(source, &solution.root)
    }

    pub(crate) fn convergence() -> &'static Self {
        static TOKENS: std::sync::OnceLock<Tokens> = std::sync::OnceLock::new();
        TOKENS.get_or_init(|| Self::load("convergence", Path::new(".")).expect("embedded tokens"))
    }

    fn entries(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (section, map) in [
            ("colors", &self.colors),
            ("roles", &self.roles),
            ("fonts", &self.fonts),
            ("radius", &self.radius),
            ("border", &self.border),
            ("shadow", &self.shadow),
            ("space", &self.space),
        ] {
            for (name, value) in map {
                out.insert(format!("{section}.{name}"), value.clone());
            }
        }
        for (i, value) in self.chart.iter().enumerate() {
            out.insert(format!("chart.{}", i + 1), value.clone());
        }
        out
    }

    fn reference(text: &str) -> Option<&str> {
        text.strip_prefix("${")
            .and_then(|s| s.strip_suffix('}'))
            .or_else(|| text.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
            .or_else(|| text.strip_prefix('$'))
            .or_else(|| {
                ([
                    "colors.", "roles.", "fonts.", "radius.", "border.", "shadow.", "space.",
                    "chart.",
                ]
                .iter()
                .any(|prefix| text.starts_with(prefix)))
                .then_some(text)
            })
    }

    fn resolve(&mut self) -> Result<(), String> {
        let entries = self.entries();
        fn value(
            key: &str,
            entries: &BTreeMap<String, String>,
            seen: &mut BTreeSet<String>,
        ) -> Result<String, String> {
            let raw = entries
                .get(key)
                .ok_or_else(|| unknown(key, entries.keys().map(String::as_str)))?;
            if !seen.insert(key.to_string()) {
                return Err(format!("token reference cycle at {key}"));
            }
            let result = if let Some(reference) = Tokens::reference(raw) {
                value(reference, entries, seen)?
            } else if key.starts_with("colors.")
                || key.starts_with("roles.")
                || key.starts_with("chart.")
                || key == "border.color"
            {
                if hex(raw) {
                    raw.clone()
                } else {
                    let reference = [format!("colors.{raw}"), format!("roles.{raw}")]
                        .into_iter()
                        .find(|candidate| entries.contains_key(candidate))
                        .ok_or_else(|| unknown(raw, entries.keys().map(String::as_str)))?;
                    value(&reference, entries, seen)?
                }
            } else if key.starts_with("radius.") && entries.contains_key(&format!("radius.{raw}")) {
                value(&format!("radius.{raw}"), entries, seen)?
            } else if key.starts_with("shadow.")
                && key != format!("shadow.{raw}")
                && entries.contains_key(&format!("shadow.{raw}"))
            {
                value(&format!("shadow.{raw}"), entries, seen)?
            } else if key.starts_with("shadow.") {
                let mut words = vec![];
                for word in raw.split_whitespace() {
                    if entries.contains_key(&format!("colors.{word}")) {
                        words.push(value(&format!("colors.{word}"), entries, seen)?);
                    } else if entries.contains_key(&format!("roles.{word}")) {
                        words.push(value(&format!("roles.{word}"), entries, seen)?);
                    } else if let Some(reference) = Tokens::reference(word) {
                        words.push(value(reference, entries, seen)?);
                    } else {
                        words.push(word.to_string());
                    }
                }
                words.join(" ")
            } else {
                raw.clone()
            };
            seen.remove(key);
            Ok(result)
        }
        let mut resolved = BTreeMap::new();
        for key in entries.keys() {
            let result =
                value(key, &entries, &mut BTreeSet::new()).map_err(|e| self.located(key, &e))?;
            resolved.insert(key.clone(), result);
        }
        for (section, map) in [
            ("colors", &mut self.colors),
            ("roles", &mut self.roles),
            ("fonts", &mut self.fonts),
            ("radius", &mut self.radius),
            ("border", &mut self.border),
            ("shadow", &mut self.shadow),
            ("space", &mut self.space),
        ] {
            for (name, val) in map {
                *val = resolved[&format!("{section}.{name}")].clone();
            }
        }
        for (i, val) in self.chart.iter_mut().enumerate() {
            *val = resolved[&format!("chart.{}", i + 1)].clone();
        }
        for (name, val) in &mut self.vars {
            let location = self.origins.get(&format!("vars.{name}"));
            let located = |error: String| match location {
                Some((file, line, column)) => {
                    format!("{file}:{line}:{column}: vars.{name}: {error}")
                }
                None => format!("vars.{name}: {error}"),
            };
            validate_variable(name).map_err(located)?;
            if let Some(reference) = Self::reference(val) {
                *val = resolved
                    .get(reference)
                    .ok_or_else(|| {
                        located(unknown(reference, resolved.keys().map(String::as_str)))
                    })?
                    .clone();
            } else if let Some(color) = self.colors.get(val).or_else(|| self.roles.get(val)) {
                *val = color.clone();
            }
        }
        Ok(())
    }

    pub(crate) fn colour(&self, text: &str) -> Result<String, String> {
        if hex(text) {
            return Ok(text.to_string());
        }
        self.roles
            .get(text)
            .or_else(|| self.colors.get(text))
            .cloned()
            .ok_or_else(|| {
                unknown(
                    text,
                    self.colors
                        .keys()
                        .chain(self.roles.keys())
                        .map(String::as_str),
                )
            })
            .map_err(|e| format!("unknown colour: {e}"))
    }

    pub(crate) fn font(&self, text: &str) -> String {
        self.fonts
            .get(text)
            .cloned()
            .unwrap_or_else(|| text.to_string())
    }

    pub(crate) fn border_size(&self, name: &str) -> &str {
        self.border.get(name).map_or("1px", String::as_str)
    }

    pub(crate) fn border_width(&self) -> &str {
        self.border.get("width").map_or("2px", String::as_str)
    }

    pub(crate) fn style(&self, property: &str, text: &str) -> Result<String, String> {
        if let Some(reference) = Self::reference(text) {
            let entries = self.entries();
            return entries
                .get(reference)
                .cloned()
                .ok_or_else(|| unknown(reference, entries.keys().map(String::as_str)));
        }
        Ok(match property {
            "color" | "background" | "background-color" | "border-color" | "fill" | "stroke" => {
                self.roles
                    .get(text)
                    .or_else(|| self.colors.get(text))
                    .cloned()
                    .unwrap_or_else(|| text.to_string())
            }
            "font-family" => self.font(text),
            "border-radius" => self
                .radius
                .get(text)
                .cloned()
                .unwrap_or_else(|| text.to_string()),
            "box-shadow" => self
                .shadow
                .get(text)
                .cloned()
                .unwrap_or_else(|| text.to_string()),
            "border" if self.colour(text).is_ok() => {
                format!("{} solid {}", self.border_width(), self.colour(text)?)
            }
            _ => text.to_string(),
        })
    }

    pub(crate) fn card_styles(&self) -> Vec<String> {
        let Some(radius) = self.radius.get("card") else {
            return Vec::new();
        };
        let mut styles = vec![
            format!("border-radius={radius}"),
            format!("background={}", self.roles["card"]),
        ];
        if let Some(shadow) = self.shadow.get("card") {
            styles.push(format!("box-shadow={shadow}"));
        }
        if self.font("sans") != "inherit" {
            styles.push(format!("font-family={}", self.font("sans")));
        }
        styles
    }

    pub(crate) fn chart_palette(&self) -> Vec<String> {
        if self.chart.is_empty() {
            ["primary", "success", "warning", "danger", "info"]
                .into_iter()
                .map(|role| self.roles[role].clone())
                .collect()
        } else {
            self.chart.clone()
        }
    }

    pub(crate) fn apply_page(
        &self,
        document: &mut crate::core::mashup::edit::MashupDocument,
    ) -> Result<(), crate::core::mashup::authoring::AuthoringError> {
        if self.card_styles().is_empty() {
            return Ok(());
        }
        if let Some(page) = document
            .widgets()
            .into_iter()
            .find(|w| w.type_id == "flexcontainer")
        {
            crate::core::mashup::authoring::merge_widget_styles(
                document,
                &page.id,
                &page.type_id,
                &[
                    format!("background={}", self.roles["background"]),
                    format!("font-family={}", self.font("sans")),
                ],
                "mashup",
            )?;
        }
        Ok(())
    }

    pub(crate) fn props(&self) -> Vec<Value> {
        let entries = self.entries();
        let mut vars = self.vars.clone();
        let mut props = vec![];
        for line in MAPPING.lines() {
            let (name, key) = line.split_once(' ').expect("mapping row");
            if key == "chart.*" {
                for (i, value) in self.chart.iter().enumerate() {
                    let name = name.replace("{index}", &(i + 1).to_string());
                    let value = vars.remove(&name).unwrap_or_else(|| value.clone());
                    props.push(json!({"name":name,"value":value}));
                }
                continue;
            }
            // A token set to `inherit` keeps the platform's value, so it writes no variable.
            if let Some(value) = vars.remove(name).or_else(|| {
                key.strip_prefix('=')
                    .map(str::to_string)
                    .or_else(|| entries.get(key).cloned())
                    .filter(|value| value != "inherit")
            }) {
                props.push(json!({"name": name, "value": value}));
            }
        }
        props.extend(
            vars.into_iter()
                .map(|(name, value)| json!({"name": name, "value": value})),
        );
        props
    }

    pub(crate) fn view(&self) -> Value {
        json!({"tokens": self, "props": self.props()})
    }
}

fn unknown<'a>(name: &str, known: impl Iterator<Item = &'a str>) -> String {
    format!(
        "unknown token {name:?}; known names: {}",
        known.collect::<Vec<_>>().join(", ")
    )
}

fn hex(text: &str) -> bool {
    text.strip_prefix('#').is_some_and(|s| {
        matches!(s.len(), 3 | 4 | 6 | 8) && s.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

pub(crate) fn mapped(name: &str) -> bool {
    MAPPING.lines().any(|line| {
        let (pattern, key) = line.split_once(' ').unwrap();
        pattern == name
            || (key == "chart.*"
                && name
                    .strip_prefix("e-chart-series-")
                    .and_then(|s| s.strip_suffix("-color"))
                    .and_then(|s| s.parse::<usize>().ok())
                    .is_some_and(|i| i > 0))
    })
}

pub(crate) fn validate_variable(name: &str) -> Result<(), String> {
    if name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && name.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
    {
        return Ok(());
    }
    let hint = match name {
        "backgroundColor" => "global-color-bg-primary".to_string(),
        "textColor" => "global-color-text-body".to_string(),
        "fontFamily" => "global-text-body-fontfamily".to_string(),
        _ => MAPPING
            .lines()
            .map(|line| line.split_once(' ').unwrap().0.replace("{index}", "1"))
            .min_by_key(|candidate| distance(name, candidate))
            .unwrap(),
    };
    Err(format!(
        "theme variable {name:?} must use lowercase-hyphen names; did you mean {hint}?"
    ))
}

fn distance(a: &str, b: &str) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, left) in a.bytes().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, right) in b.bytes().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(old + 1)
                .min(previous + usize::from(left != right));
            previous = old;
        }
    }
    row[b.len()]
}

/// Inspect embedded presets or resolve one file without contacting a server.
pub fn inspect(source: Option<&str>, base: &Path) -> Result<Value, String> {
    match source {
        Some(source) => Tokens::load(source, base).map(|t| t.view()),
        None => Ok(json!({"presets": PRESETS})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_presets_are_deterministic_and_convergence_keeps_the_old_table() {
        for name in PRESETS {
            let a = Tokens::load(name, Path::new(".")).unwrap();
            assert_eq!(a.view(), Tokens::load(name, Path::new(".")).unwrap().view());
            let props = a.props();
            let names: BTreeSet<_> = props.iter().map(|p| p["name"].as_str().unwrap()).collect();
            assert_eq!(names.len(), props.len());
            assert!(props
                .iter()
                .all(|p| validate_variable(p["name"].as_str().unwrap()).is_ok()));
            assert!(props
                .iter()
                .any(|p| p["name"] == "global-color-core-primary"));
        }
        let tokens = Tokens::convergence();
        for (name, expected) in [
            ("primary", "#0088CE"),
            ("ink", "#0B0B0B"),
            ("lime", "#CCFF00"),
            ("pink", "#FF2E93"),
            ("purple", "#8B5CFF"),
            ("orange", "#FF8C3C"),
            ("mint", "#4EE0AE"),
        ] {
            assert_eq!(tokens.colour(name).unwrap(), expected);
        }
        assert_eq!(
            tokens.font("mono"),
            "ui-monospace, Menlo, Consolas, monospace"
        );
        assert_eq!(tokens.font("sans"), "inherit");
    }

    #[test]
    fn tokens_reject_unknown_fields_references_and_inheritance_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tokens.yaml");
        for (yaml, expected) in [
            ("version: 1\nname: Acme\ncolours: {}\n", "unknown field"),
            (
                "version: 1\nname: Acme\nroles: {text: missing}\n",
                "known names",
            ),
            (
                "version: 1\nname: Acme\nfonts: {sans: '${fonts.missing}'}\n",
                "fonts.sans",
            ),
            (
                "version: 1\nname: Acme\nvars: {backgroundColor: '#FFFFFF'}\n",
                "global-color-bg-primary",
            ),
            (
                "version: 1\nname: Acme\ncolors: {a: b, b: a}\n",
                "reference cycle",
            ),
        ] {
            std::fs::write(&file, yaml).unwrap();
            let error = Tokens::load("tokens.yaml", dir.path()).unwrap_err();
            assert!(error.contains("tokens.yaml"), "{error}");
            assert!(error.contains(expected), "{error}");
        }
        std::fs::write(&file, "version: 1\nname: Acme\nextends: ./other.yaml\n").unwrap();
        std::fs::write(
            dir.path().join("other.yaml"),
            "version: 1\nname: Other\nextends: ./tokens.yaml\n",
        )
        .unwrap();
        assert!(Tokens::load("tokens.yaml", dir.path())
            .unwrap_err()
            .contains("extends cycle"));
    }

    #[test]
    fn tokens_inheritance_resolves_after_overlay_and_vars_override_mapping() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tokens.yaml"), "version: 1\nname: Acme\nextends: brutalist\ncolors: {ink: '#112233'}\nvars: {global-color-text-body: '${colors.paper}', acme-custom-color: '${colors.ink}'}\n").unwrap();
        let tokens = Tokens::load("tokens.yaml", dir.path()).unwrap();
        assert_eq!(
            tokens.style("box-shadow", "hard").unwrap(),
            "4px 4px 0 #112233"
        );
        assert_eq!(tokens.colour("text").unwrap(), "#112233");
        let props = tokens.props();
        assert!(props
            .iter()
            .any(|p| p == &json!({"name":"global-color-text-body","value":"#F3F0E6"})));
        assert!(props
            .iter()
            .any(|p| p == &json!({"name":"acme-custom-color","value":"#112233"})));
        assert!(tokens
            .style("color", "${colors.missing}")
            .unwrap_err()
            .contains("known names"));
    }

    #[test]
    fn preset_values_never_lose_a_hash_to_a_yaml_comment() {
        // ` #` starts a YAML comment, so `sm: 0 1px 3px #0000001A` would silently drop the colour.
        for name in PRESETS {
            for line in preset(name).unwrap().lines() {
                if let Some((_, value)) = line.split_once(": ") {
                    assert!(
                        !value.contains(" #") || value.starts_with('\''),
                        "{name}: quote {line:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn convergence_theme_props_keep_native_containers_lines_and_fonts() {
        let props = Tokens::convergence().props();
        let value = |name: &str| {
            props
                .iter()
                .find(|p| p["name"] == name)
                .map(|p| p["value"].as_str().unwrap().to_string())
        };
        // Layout containers stay borderless and lines stay 1px: the theme must not frame
        // every flex container or thicken every input.
        for name in [
            "flexcontainer-border-style",
            "flexcontainer-border-width",
            "global-line-border-thickness",
            "global-color-core-secondary1",
            "global-color-bg-selected",
            "global-color-bg-tertiary",
            "e-chart-series-1-color",
        ] {
            assert_eq!(value(name), None, "{name}");
        }
        // `inherit` fonts keep Open Sans instead of writing `inherit` into the theme.
        assert_eq!(value("global-text-body-fontfamily"), None);
        assert_eq!(
            value("global-color-core-danger").as_deref(),
            Some("#AF3231")
        );
        assert_eq!(value("global-color-text-body").as_deref(), Some("#232B2D"));
    }

    #[test]
    fn tokens_brutalist_targets_native_chart_text_and_borders() {
        let tokens = Tokens::load("brutalist", Path::new(".")).unwrap();
        let props = tokens.props();
        for name in [
            "e-chart-tick-label-color",
            "e-chart-value-color",
            "e-chart-ruler-color",
        ] {
            assert!(props
                .iter()
                .any(|p| p == &json!({"name":name,"value":"#F3F0E6"})));
        }
        assert_eq!(tokens.style("border", "ink").unwrap(), "2px solid #0B0B0B");
        assert_eq!(tokens.style("border-radius", "none").unwrap(), "0px");
        assert_eq!(
            tokens.font("display"),
            "Archivo Black, Archivo, Arial Black, sans-serif"
        );
        let mut longer = tokens;
        longer.chart = vec!["#CCFF00".into(); 12];
        let props = longer.props();
        assert_eq!(
            props
                .iter()
                .filter(|p| p["name"].as_str().unwrap().starts_with("e-chart-series-"))
                .count(),
            12
        );
        assert!(mapped("e-chart-series-12-color"));
    }
}
