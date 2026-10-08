//! Colour for terminal output. Every style in the command line comes from this module.
//!
//! Each helper decides per stream. `NO_COLOR`, `FORCE_COLOR`, `CI` and a non-terminal stream
//! are honoured by `owo-colors`. Piped output stays plain.
//!
//! WARNING: ANSI codes add bytes that `{:<8}` counts as width. Pad the plain text first.
//! Then style the padded text.

use owo_colors::{OwoColorize, Stream};

/// Style a success word, such as `ok`, on stdout.
pub(crate) fn ok(text: &str) -> String {
    text.if_supports_color(Stream::Stdout, |t| t.green())
        .to_string()
}

/// Style a failure word, such as `FAIL` or `BROKEN`, on stdout.
pub(crate) fn fail(text: &str) -> String {
    text.if_supports_color(Stream::Stdout, |t| t.red())
        .to_string()
}

/// Style a warning word on stdout.
pub(crate) fn warn(text: &str) -> String {
    text.if_supports_color(Stream::Stdout, |t| t.yellow())
        .to_string()
}

/// Style secondary text on stdout.
pub(crate) fn dim(text: &str) -> String {
    text.if_supports_color(Stream::Stdout, |t| t.dimmed())
        .to_string()
}

/// Style a heading on stdout.
pub(crate) fn heading(text: &str) -> String {
    text.if_supports_color(Stream::Stdout, |t| t.bold())
        .to_string()
}

/// The `twaco:` prefix of a message on stderr. The decision uses the stderr stream.
pub(crate) fn prefix() -> String {
    "twaco:"
        .if_supports_color(Stream::Stderr, |t| t.red())
        .to_string()
}
