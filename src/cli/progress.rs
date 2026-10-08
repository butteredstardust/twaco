//! Progress bars on stderr, for commands that make many server requests or wait long.
//!
//! WARNING: Draw on stderr only, and only when stderr is a terminal. With a pipe, a file, a test
//! or a CI run, draw nothing. Draw nothing while logs write to stderr, so the two do not mix.
//! Stdout is never touched.

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::io::IsTerminal;
use std::sync::Mutex;
use twaco::core::progress::Progress;

/// A reporter that draws one bar for the current phase, or nothing.
pub(crate) struct Bars {
    enabled: bool,
    bar: Mutex<Option<ProgressBar>>,
}

/// Decide whether bars draw. They draw only on a terminal, and only when logs stay off stderr.
fn draws(stderr_is_terminal: bool, logs_on_stderr: bool) -> bool {
    stderr_is_terminal && !logs_on_stderr
}

/// The reporter for the given conditions: indicatif bars when they draw, otherwise silent.
fn reporter_for(stderr_is_terminal: bool, logs_on_stderr: bool) -> Bars {
    Bars::new(draws(stderr_is_terminal, logs_on_stderr))
}

/// The reporter for this process.
pub(crate) fn reporter() -> Bars {
    reporter_for(
        std::io::stderr().is_terminal(),
        twaco::core::diagnostics::writes_to_stderr(),
    )
}

impl Bars {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            bar: Mutex::new(None),
        }
    }

    /// True when this reporter builds indicatif bars.
    #[cfg(test)]
    fn is_drawing(&self) -> bool {
        self.enabled
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<ProgressBar>> {
        self.bar
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Progress for Bars {
    fn start(&self, phase: &str, total: Option<u64>) {
        if !self.enabled {
            return;
        }
        let (bar, template) = match total {
            Some(total) => (
                ProgressBar::with_draw_target(Some(total), ProgressDrawTarget::stderr()),
                "{prefix}: [{bar:30}] {pos}/{len} {msg}",
            ),
            None => {
                let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
                bar.enable_steady_tick(std::time::Duration::from_millis(120));
                (bar, "{prefix}: {spinner} {msg}")
            }
        };
        if let Ok(style) = ProgressStyle::with_template(template) {
            bar.set_style(style);
        }
        bar.set_prefix(phase.to_string());
        if let Some(previous) = self.slot().replace(bar) {
            previous.finish_and_clear();
        }
    }

    fn advance(&self, n: u64) {
        if let Some(bar) = self.slot().as_ref() {
            bar.inc(n);
        }
    }

    fn message(&self, text: &str) {
        if let Some(bar) = self.slot().as_ref() {
            bar.set_message(text.to_string());
        }
    }

    fn finish(&self) {
        if let Some(bar) = self.slot().take() {
            bar.finish_and_clear();
        }
    }
}

impl Drop for Bars {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_draw_only_on_a_terminal_without_logs_on_stderr() {
        assert!(draws(true, false));
        assert!(!draws(true, true));
        assert!(!draws(false, false));
        assert!(!draws(false, true));
    }

    #[test]
    fn the_reporter_for_a_terminal_without_logs_builds_indicatif_bars() {
        let bars = reporter_for(true, false);
        assert!(bars.is_drawing());
        bars.start("phase", Some(2));
        assert!(bars.slot().is_some());
        for (terminal, logs) in [(true, true), (false, false), (false, true)] {
            let quiet = reporter_for(terminal, logs);
            assert!(!quiet.is_drawing());
            quiet.start("phase", Some(2));
            assert!(quiet.slot().is_none());
        }
    }

    #[test]
    fn a_disabled_reporter_makes_no_bar() {
        let bars = Bars::new(false);
        bars.start("phase", Some(3));
        bars.advance(1);
        assert!(bars.slot().is_none());
    }

    #[test]
    fn an_enabled_reporter_keeps_one_bar_per_phase_and_clears_it_at_the_end() {
        let bars = Bars::new(true);
        bars.start("one", Some(3));
        bars.start("two", None);
        assert_eq!(bars.slot().as_ref().unwrap().prefix(), "two");
        bars.finish();
        assert!(bars.slot().is_none());
    }
}
