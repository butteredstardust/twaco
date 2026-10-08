//! Colour for terminal output. Every style in the command line comes from this module.
//!
//! A non-empty `NO_COLOR` turns colour off on every stream and wins over `FORCE_COLOR`.
//! Otherwise each helper decides per stream. `FORCE_COLOR`, `CI` and a non-terminal stream
//! are honoured by `owo-colors`. Piped output stays plain.
//!
//! WARNING: ANSI codes add bytes that `{:<8}` counts as width. Pad the plain text first.
//! Then style the padded text.

use owo_colors::{OwoColorize, Stream, Style};

/// True when `NO_COLOR` is set to a non-empty value. `owo-colors` lets `FORCE_COLOR` override
/// `NO_COLOR`, so this check runs first.
fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
}

/// Paint the text with the style, unless `NO_COLOR` is set or the stream does not support colour.
fn paint(text: &str, stream: Stream, style: Style) -> String {
    if no_color() {
        return text.to_string();
    }
    text.if_supports_color(stream, |t| t.style(style))
        .to_string()
}

/// Style a success word, such as `ok`, on stdout.
pub(crate) fn ok(text: &str) -> String {
    paint(text, Stream::Stdout, Style::new().green())
}

/// Style a failure word, such as `FAIL` or `BROKEN`, on stdout.
pub(crate) fn fail(text: &str) -> String {
    paint(text, Stream::Stdout, Style::new().red())
}

/// Style a warning word on stdout.
pub(crate) fn warn(text: &str) -> String {
    paint(text, Stream::Stdout, Style::new().yellow())
}

/// Style secondary text on stdout.
pub(crate) fn dim(text: &str) -> String {
    paint(text, Stream::Stdout, Style::new().dimmed())
}

/// Style a heading on stdout.
pub(crate) fn heading(text: &str) -> String {
    paint(text, Stream::Stdout, Style::new().bold())
}

/// The `twaco:` prefix of a message on stderr. The decision uses the stderr stream.
pub(crate) fn prefix() -> String {
    paint("twaco:", Stream::Stderr, Style::new().red())
}
