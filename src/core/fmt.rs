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
    Configuration, ConfigurationBuilder, QuoteStyle, SemiColons, TrailingCommas, UseBraces,
};
use dprint_plugin_typescript::FormatTextOptions;
use std::path::Path;

/// How a project wants its scripts formatted.
pub struct Style {
    pub indent_width: u8,
    /// jsbeautifier is configured with no wrapping, so the default is effectively unlimited.
    pub line_width: u32,
}

impl Default for Style {
    fn default() -> Self {
        Style { indent_width: 4, line_width: 100_000 }
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
            .quote_style(QuoteStyle::PreferDouble)
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
pub fn format(source: &str, style: &Style) -> Result<Option<String>, String> {
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
pub fn format_raw(source: &str, style: &Style) -> Result<Option<String>, String> {
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

/// Whether a script is already formatted. Formats in memory and writes nothing, so a
/// verification pass can never mutate a file.
pub fn is_formatted(source: &str, style: &Style) -> Result<bool, String> {
    Ok(match format(source, style)? {
        None => true,
        Some(out) => out == source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_argument_list_never_gets_a_trailing_comma() {
        // Long enough that a narrow width would tempt dprint into breaking and adding one.
        let src = "logger.debug(\"{} - a rather long message indeed, padded out\", me.name, other, third);\n";
        let out = format(src, &Style::default()).unwrap();
        let text = out.unwrap_or_else(|| src.to_string());
        assert!(!text.contains(",)"), "Rhino-unsafe trailing comma in a call: {text}");
        assert!(!text.contains(",\n)"), "Rhino-unsafe trailing comma in a call: {text}");
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
        assert_eq!(format(src, &Style::default()).unwrap(), None, "already formatted");
        let raw = format_raw(src, &Style::default()).unwrap().unwrap();
        assert!(raw.ends_with('\n'), "dprint does add one, so the trim is doing real work");
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
        let once = format(src, &Style::default()).unwrap().expect("needs formatting");
        assert_eq!(format(&once, &Style::default()).unwrap(), None, "a second pass must be a no-op");
    }

    #[test]
    fn formatting_is_idempotent() {
        let src = "const a={b:1,c:2};\nif(a.b){result=a.c;}\n";
        let once = format(src, &Style::default()).unwrap().unwrap();
        let twice = format(&once, &Style::default()).unwrap();
        assert!(twice.is_none() || twice.unwrap() == once);
    }
}
