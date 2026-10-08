//! Progress reports for long operations: many server requests, or one long wait.
//!
//! WARNING: A message holds an entity name or a phase name only. Never put a URL, a parameter, a
//! body or a credential in one. The front ends show or send it as it is.
//!
//! The purpose is to let a front end show that twaco is working. A report changes no result.
//! Workers on several threads call the same reporter, so every method takes `&self`.
//!
//! A phase runs from `start` to `finish`. [`phase`] pairs the two, so an early return still ends
//! the phase.

/// Receives progress reports from one command. Implementations are thread-safe.
pub trait Progress: Sync {
    /// Begin a phase. `total` is the number of steps, when known. Replaces the previous phase.
    fn start(&self, phase: &str, total: Option<u64>);
    /// Complete `n` steps of the current phase.
    fn advance(&self, n: u64);
    /// Name what the current phase is working on, for example an entity.
    fn message(&self, text: &str);
    /// End the current phase.
    fn finish(&self);
}

/// Reports nothing. The default for callers that do not show progress.
#[derive(Clone, Copy, Debug, Default)]
pub struct Noop;

impl Progress for Noop {
    fn start(&self, _: &str, _: Option<u64>) {}
    fn advance(&self, _: u64) {}
    fn message(&self, _: &str) {}
    fn finish(&self) {}
}

/// A shared reporter that reports nothing.
pub static NONE: Noop = Noop;

/// A started phase. Dropping it finishes the phase.
#[must_use = "the phase ends when this value is dropped"]
pub struct Phase<'a>(&'a dyn Progress);

/// Start a phase that ends when the returned value is dropped.
pub fn phase<'a>(progress: &'a dyn Progress, name: &str, total: Option<u64>) -> Phase<'a> {
    progress.start(name, total);
    Phase(progress)
}

impl Drop for Phase<'_> {
    fn drop(&mut self) {
        self.0.finish();
    }
}

/// Records every report, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct Recorder(std::sync::Mutex<Vec<Event>>);

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Start(String, Option<u64>),
    Advance(u64),
    Message(String),
    Finish,
}

#[cfg(test)]
impl Recorder {
    pub(crate) fn events(&self) -> Vec<Event> {
        self.0.lock().unwrap().clone()
    }

    /// The phases started, with their totals, in order.
    pub(crate) fn phases(&self) -> Vec<(String, Option<u64>)> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Start(name, total) => Some((name, total)),
                _ => None,
            })
            .collect()
    }

    /// The sum of all `advance` calls.
    pub(crate) fn advanced(&self) -> u64 {
        self.events()
            .iter()
            .map(|event| match event {
                Event::Advance(n) => *n,
                _ => 0,
            })
            .sum()
    }
}

#[cfg(test)]
impl Progress for Recorder {
    fn start(&self, phase: &str, total: Option<u64>) {
        self.0
            .lock()
            .unwrap()
            .push(Event::Start(phase.to_string(), total));
    }
    fn advance(&self, n: u64) {
        self.0.lock().unwrap().push(Event::Advance(n));
    }
    fn message(&self, text: &str) {
        self.0
            .lock()
            .unwrap()
            .push(Event::Message(text.to_string()));
    }
    fn finish(&self) {
        self.0.lock().unwrap().push(Event::Finish);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phase_finishes_when_it_is_dropped() {
        let recorder = Recorder::default();
        {
            let _phase = phase(&recorder, "reading", Some(3));
            recorder.advance(3);
        }
        assert_eq!(
            recorder.events(),
            [
                Event::Start("reading".to_string(), Some(3)),
                Event::Advance(3),
                Event::Finish
            ]
        );
    }
}
