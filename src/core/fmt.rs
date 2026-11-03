//! Formatting ThingWorx service scripts, in process.
//!
//! ThingWorx service bodies are bare statement sequences full of implicit globals (`me`,
//! `logger`, `Things`, `result`), not modules. dprint parses representative exported service
//! bodies without requiring module wrappers.
//!
//! **Scope.** This lays out code; it does not certify that the code runs on the platform's
//! script engine. It guarantees only that formatting introduces nothing Rhino would reject.
//! Syntax that was already in the source passes straight through, so a script using a feature
//! the server's engine lacks formats happily and still fails there. The gate for *that* is the
//! live parse through `CheckScriptWithLinesAndColumns`, which is handled by the live gate.
//! Do not mistake a clean `fmt` for a working service.

use dprint_plugin_typescript::configuration::{
    Configuration, ConfigurationBuilder, QuoteStyle as DprintQuoteStyle, SemiColons,
    TrailingCommas, UseBraces,
};
use dprint_plugin_typescript::FormatTextOptions;
use std::collections::BTreeMap;
use std::path::Path;

use super::{config, sidecar};

/// The script styles found in a set of service bodies.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Survey {
    pub(crate) scripts: usize,
    pub(crate) indent: BTreeMap<u8, usize>,
    pub(crate) tabs: usize,
    pub(crate) double: usize,
    pub(crate) single: usize,
}

impl Survey {
    pub(crate) fn majority_indent(&self) -> Option<(u8, usize, usize)> {
        let determined = self.indent.values().sum();
        let (&width, &count) = self.indent.iter().max_by_key(|(_, count)| *count)?;
        (count > determined / 2).then_some((width, count, determined))
    }

    pub(crate) fn majority_quote(&self) -> Option<(config::QuoteStyle, usize, usize)> {
        let determined = self.double + self.single;
        let (style, count) = if self.double > self.single {
            (config::QuoteStyle::Double, self.double)
        } else {
            (config::QuoteStyle::Single, self.single)
        };
        (count > determined / 2).then_some((style, count, determined))
    }
}

/// Survey the script style without parsing or changing the scripts.
pub(crate) fn survey<'a>(scripts: impl IntoIterator<Item = &'a str>) -> Survey {
    let mut found = Survey::default();
    for script in scripts {
        found.scripts += 1;
        survey_indent(script, &mut found);
        match quote_style(script) {
            Some(config::QuoteStyle::Double) => found.double += 1,
            Some(config::QuoteStyle::Single) => found.single += 1,
            None => {}
        }
    }
    found
}

fn survey_indent(script: &str, found: &mut Survey) {
    let script = sidecar::dedent(script);
    let mut smallest = None;
    for line in script.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('*') {
            continue;
        }
        if line.starts_with('\t') {
            found.tabs += 1;
            return;
        }
        let spaces = line.len() - line.trim_start_matches(' ').len();
        if spaces > 0 {
            smallest = Some(smallest.map_or(spaces, |current: usize| current.min(spaces)));
        }
    }
    if let Some(width @ 1..=8) = smallest.and_then(|width| u8::try_from(width).ok()) {
        *found.indent.entry(width).or_default() += 1;
    }
}

fn quote_style(script: &str) -> Option<config::QuoteStyle> {
    let bytes = script.as_bytes();
    let mut at = 0;
    let (mut double, mut single) = (0, 0);
    while at < bytes.len() {
        match bytes[at] {
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                at += 2;
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at += 2;
                while at + 1 < bytes.len() && (bytes[at], bytes[at + 1]) != (b'*', b'/') {
                    at += 1;
                }
                at = (at + 2).min(bytes.len());
            }
            b'\'' | b'"' => {
                let quote = bytes[at];
                if quote == b'\'' {
                    single += 1;
                } else {
                    double += 1;
                }
                at += 1;
                while at < bytes.len() {
                    if bytes[at] == b'\\' {
                        at += 2;
                    } else if bytes[at] == quote {
                        at += 1;
                        break;
                    } else {
                        at += 1;
                    }
                }
            }
            b'`' => {
                at += 1;
                while at < bytes.len() {
                    if bytes[at] == b'\\' {
                        at += 2;
                    } else if bytes[at] == b'`' {
                        at += 1;
                        break;
                    } else {
                        at += 1;
                    }
                }
            }
            _ => at += 1,
        }
    }
    match double.cmp(&single) {
        std::cmp::Ordering::Greater => Some(config::QuoteStyle::Double),
        std::cmp::Ordering::Less => Some(config::QuoteStyle::Single),
        std::cmp::Ordering::Equal => None,
    }
}

/// Explain format settings that would restyle most of the repository's scripts.
pub(crate) fn style_mismatches(survey: &Survey, configured: &config::ScriptFormat) -> Vec<String> {
    let mut messages = Vec::new();
    if let Some((width, count, determined)) = survey.majority_indent() {
        if width != configured.indent_width {
            messages.push(format!(
                "{count} of {determined} indented scripts use {width} spaces but [format.script] indent_width = {}: `twaco fmt` would rewrite them, and each rewritten service differs from the server until deployed",
                configured.indent_width
            ));
        }
    }
    if let Some((quote, count, determined)) = survey.majority_quote() {
        if quote != configured.quote_style {
            messages.push(format!(
                "{count} of {determined} scripts with strings use {} quotes but [format.script] quote_style = \"{}\": `twaco fmt` would rewrite them, and each rewritten service differs from the server until deployed",
                quote_name(quote),
                quote_name(configured.quote_style)
            ));
        }
    }
    messages
}

pub(crate) fn quote_name(style: config::QuoteStyle) -> &'static str {
    match style {
        config::QuoteStyle::Double => "double",
        config::QuoteStyle::Single => "single",
    }
}

/// How a project wants its scripts formatted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Style {
    pub(crate) indent_width: u8,
    /// jsbeautifier is configured with no wrapping, so the default is effectively unlimited.
    pub(crate) line_width: u32,
    pub(crate) quote_style: config::QuoteStyle,
}

impl Default for Style {
    fn default() -> Self {
        Self::from(&config::ScriptFormat::default())
    }
}

impl From<&config::ScriptFormat> for Style {
    fn from(script: &config::ScriptFormat) -> Self {
        Self {
            indent_width: script.indent_width,
            line_width: script.line_width,
            quote_style: script.quote_style,
        }
    }
}

impl Style {
    fn build(&self) -> Configuration {
        ConfigurationBuilder::new()
            .indent_width(self.indent_width)
            .line_width(self.line_width)
            // Not a style preference. dprint's default emits a trailing comma in a *call*
            // argument list, which is ES2017; ThingWorx runs Rhino.
            .trailing_commas(TrailingCommas::Never)
            // Pinned so a dprint upgrade cannot silently restyle every script in every project.
            // Only trailing commas are syntax-critical; the rest is determinism.
            .quote_style(match self.quote_style {
                config::QuoteStyle::Double => DprintQuoteStyle::PreferDouble,
                config::QuoteStyle::Single => DprintQuoteStyle::PreferSingle,
            })
            .semi_colons(SemiColons::Always)
            .use_braces(UseBraces::Always)
            // Line endings are the sidecar writer's business, not the formatter's.
            .build()
    }
}

/// Format one script as a sidecar file. Returns `None` when it is already formatted.
///
/// **The result never ends in a newline.** Extraction strips blank lines from both edges of a
/// script, so a sidecar has no trailing newline; dprint always adds one. Left alone, the
/// formatter would add a newline that the next extract removed, and the two would disagree
/// forever with neither ever settling. Trimming here makes `fmt` a fixed point of `extract`.
pub(crate) fn format(source: &str, style: &Style) -> Result<Option<String>, String> {
    // dprint returning None means "already formatted by my rules", which still allows the
    // trailing newline extraction would strip. The invariant has to be applied either way.
    let text = format_raw(source, style)?.unwrap_or_else(|| source.to_string());
    let trimmed = text.trim_end_matches('\n');
    if trimmed == source {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

/// dprint's own output, newline and all. Exposed for tests that care about the difference.
pub(crate) fn format_raw(source: &str, style: &Style) -> Result<Option<String>, String> {
    let config = style.build();
    let options = FormatTextOptions {
        path: Path::new("service.js"),
        extension: None,
        text: source.to_string(),
        config: &config,
        external_formatter: None,
    };
    dprint_plugin_typescript::format_text(options).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survey_finds_indent_widths_and_ignores_comment_continuations() {
        let found = survey([
            "if (ok) {\n  result = 1;\n}",
            "if (ok) {\n    result = 1;\n}",
            "/**\n * documentation\n */\nresult = 1;",
            "if (ok) {\n\tresult = 1;\n}",
            "result = 1;",
            "\n            if (ok) {\n              result = 1;\n            }\n",
        ]);
        assert_eq!(found.scripts, 6);
        assert_eq!(found.indent.get(&2), Some(&2));
        assert_eq!(found.indent.get(&4), Some(&1));
        assert!(!found.indent.contains_key(&1));
        assert_eq!(found.tabs, 1);
    }

    #[test]
    fn survey_counts_quotes_without_reading_comments_templates_or_escapes() {
        let found = survey([
            "var a = \"double\"; var b = \"again\";",
            "var a = 'single';",
            "var a = 'one'; var b = \"two\";",
            "var a = \"escaped \\\" quote\"; // 'comment'\n/* \"comment\" */\nvar b = `template ' \"`;",
        ]);
        assert_eq!((found.double, found.single), (2, 1));
    }

    #[test]
    fn survey_majorities_require_more_than_half_of_determined_scripts() {
        let majority = survey([
            "if (ok) {\n  result = \"yes\";\n}",
            "if (ok) {\n  result = \"yes\";\n}",
            "if (ok) {\n    result = 'yes';\n}",
        ]);
        assert_eq!(majority.majority_indent(), Some((2, 2, 3)));
        assert_eq!(
            majority.majority_quote(),
            Some((config::QuoteStyle::Double, 2, 3))
        );

        let tied = survey(["  var a = \"yes\";", "    var a = 'yes';"]);
        assert_eq!(tied.majority_indent(), None);
        assert_eq!(tied.majority_quote(), None);
    }

    #[test]
    fn configurable_indent_and_quotes_are_applied() {
        let style = Style::from(&config::ScriptFormat {
            indent_width: 2,
            line_width: 100_000,
            quote_style: config::QuoteStyle::Single,
        });
        assert_eq!(
            format("if(true){result=\"ready\";result=\"again\";}", &style).unwrap(),
            Some("if (true) {\n  result = 'ready';\n  result = 'again';\n}".to_string())
        );
    }

    #[test]
    fn the_default_style_matches_the_default_configuration() {
        assert_eq!(
            Style::default(),
            Style::from(&config::ScriptFormat::default())
        );
    }

    #[test]
    fn a_call_argument_list_never_gets_a_trailing_comma() {
        // Long enough that a narrow width would tempt dprint into breaking and adding one.
        let src = "logger.debug(\"{} - a rather long message indeed, padded out\", me.name, other, third);\n";
        let out = format(src, &Style::default()).unwrap();
        let text = out.unwrap_or_else(|| src.to_string());
        assert!(
            !text.contains(",)"),
            "Rhino-unsafe trailing comma in a call: {text}"
        );
        assert!(
            !text.contains(",\n)"),
            "Rhino-unsafe trailing comma in a call: {text}"
        );
    }

    #[test]
    fn a_thingworx_service_body_parses() {
        // Implicit globals, no module wrapper, no imports: this is what a service body is.
        let src = "var out = me.GetThing();\nlogger.warn(\"{}\", me.name);\nresult = out;\n";
        assert!(format(src, &Style::default()).is_ok());
    }

    #[test]
    fn modern_syntax_passes_through_untouched() {
        // Documents the boundary rather than asserting it is safe: formatting does not and
        // cannot tell you whether the platform's engine accepts this.
        let src = "const f = (a) => a + 1;
";
        assert!(format(src, &Style::default()).is_ok());
    }

    #[test]
    fn a_formatted_script_never_ends_in_a_newline() {
        // Regression: dprint always adds one, extraction always strips it, and the two would
        // have taken turns rewriting the same file.
        let src = "var a = 1;";
        assert_eq!(
            format(src, &Style::default()).unwrap(),
            None,
            "already formatted"
        );
        let raw = format_raw(src, &Style::default()).unwrap().unwrap();
        assert!(
            raw.ends_with('\n'),
            "dprint does add one, so the trim is doing real work"
        );
    }

    #[test]
    fn an_already_formatted_script_still_loses_its_trailing_newline() {
        // dprint returns None here, so an earlier version left the newline in place and the
        // formatter disagreed with extraction on a file it claimed was fine.
        let out = format("var a = 1;\n", &Style::default()).unwrap();
        assert_eq!(out.as_deref(), Some("var a = 1;"));
    }

    #[test]
    fn formatting_settles_immediately() {
        let src = "const a={b:1};\nif(a.b){result=1;}";
        let once = format(src, &Style::default())
            .unwrap()
            .expect("needs formatting");
        assert_eq!(
            format(&once, &Style::default()).unwrap(),
            None,
            "a second pass must be a no-op"
        );
    }

    #[test]
    fn formatting_is_idempotent() {
        let src = "const a={b:1,c:2};\nif(a.b){result=a.c;}\n";
        let once = format(src, &Style::default()).unwrap().unwrap();
        let twice = format(&once, &Style::default()).unwrap();
        assert!(twice.is_none() || twice.unwrap() == once);
    }
}
