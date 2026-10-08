//! MCP progress notifications: `notifications/progress` lines on stdout, before the response.
//!
//! WARNING: The progress token comes from the caller. Echo it in a notification, but never log
//! it. Stdout carries responses too: write each notification as one whole line under the same
//! lock as the responses, and flush it.
//!
//! The purpose is to let a client show that a long tool call is alive. A call without a token
//! sends nothing. `progress` counts completed steps across all phases, so it rises on every
//! notification. `total` is the sum of the phase totals so far, and is left out once a phase has
//! no known total. A burst of steps sends at most one notification per interval. A phase end
//! sends the latest count at once, so the client sees the final value.

use crate::core::progress::Progress;
use serde_json::{json, Value};
use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The shared output of the server: responses and notifications both go through this lock.
pub(crate) type Sink<'a> = Mutex<dyn Write + Send + 'a>;

/// At most about ten notifications per second, plus one at the end of each phase.
const INTERVAL: Duration = Duration::from_millis(100);

#[derive(Default)]
struct State {
    /// Steps completed in all phases so far.
    done: u64,
    /// Sum of the phase totals, or `None` once a phase has no total.
    total: Option<u64>,
    phase: String,
    message: Option<String>,
    last_sent: Option<Instant>,
    /// The `progress` value of the last notification.
    sent: Option<u64>,
}

/// Turns progress reports of one `tools/call` into notifications.
pub(crate) struct Notifier<'a> {
    sink: &'a Sink<'a>,
    token: Value,
    interval: Duration,
    state: Mutex<State>,
}

/// The progress token of a request: a string or an integer, in `params._meta.progressToken`.
pub(crate) fn token_of(params: &Value) -> Option<Value> {
    params
        .get("_meta")?
        .get("progressToken")
        .filter(|token| token.is_string() || token.is_i64() || token.is_u64())
        .cloned()
}

impl<'a> Notifier<'a> {
    pub(crate) fn new(sink: &'a Sink<'a>, token: Value) -> Self {
        Self::with_interval(sink, token, INTERVAL)
    }

    pub(crate) fn with_interval(sink: &'a Sink<'a>, token: Value, interval: Duration) -> Self {
        Self {
            sink,
            token,
            interval,
            state: Mutex::new(State {
                total: Some(0),
                ..State::default()
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Write one notification. The caller holds the state lock, so values never go backwards.
    fn emit(&self, state: &mut State) {
        let mut params = json!({ "progressToken": self.token, "progress": state.done });
        if let Some(total) = state.total {
            params["total"] = json!(total);
        }
        let text = match (&state.phase, &state.message) {
            (phase, Some(message)) if !phase.is_empty() => Some(format!("{phase}: {message}")),
            (phase, None) if !phase.is_empty() => Some(phase.clone()),
            (_, Some(message)) => Some(message.clone()),
            _ => None,
        };
        if let Some(text) = text {
            params["message"] = json!(text);
        }
        let notification =
            json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": params });
        let mut line = serde_json::to_string(&notification).expect("JSON values serialise");
        line.push('\n');
        let mut sink = self
            .sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A broken pipe also fails the response write, which reports it.
        let _ = sink.write_all(line.as_bytes()).and_then(|()| sink.flush());
        state.last_sent = Some(Instant::now());
        state.sent = Some(state.done);
    }
}

impl Progress for Notifier<'_> {
    fn start(&self, phase: &str, total: Option<u64>) {
        let mut state = self.state();
        state.phase = phase.to_string();
        state.message = None;
        state.total = state.total.zip(total).map(|(sum, total)| sum + total);
        if state.sent.is_none() {
            self.emit(&mut state);
        }
    }

    fn advance(&self, n: u64) {
        let mut state = self.state();
        state.done += n;
        let due = state
            .last_sent
            .is_none_or(|sent| sent.elapsed() >= self.interval);
        if due && n > 0 {
            self.emit(&mut state);
        }
    }

    fn message(&self, text: &str) {
        self.state().message = Some(text.to_string());
    }

    fn finish(&self) {
        let mut state = self.state();
        if state.sent != Some(state.done) {
            self.emit(&mut state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A writer the test can read after the notifier wrote to it.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn lines(shared: &Shared) -> Vec<Value> {
        String::from_utf8(shared.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).expect("every line is JSON"))
            .collect()
    }

    #[test]
    fn a_burst_sends_few_notifications_and_the_last_one_has_the_final_count() {
        let shared = Shared::default();
        let sink: Mutex<Shared> = Mutex::new(shared.clone());
        let sink: &Sink<'_> = &sink;
        let notifier = Notifier::new(sink, json!("t"));
        notifier.start("working", Some(1000));
        for _ in 0..1000 {
            notifier.advance(1);
        }
        notifier.finish();
        let sent = lines(&shared);
        assert!(sent.len() <= 3, "{} notifications", sent.len());
        let last = sent.last().unwrap();
        assert_eq!(last["params"]["progress"], 1000);
        assert_eq!(last["params"]["total"], 1000);
        assert_eq!(last["method"], "notifications/progress");
    }

    #[test]
    fn progress_rises_across_threads_and_phases() {
        let shared = Shared::default();
        let sink: Mutex<Shared> = Mutex::new(shared.clone());
        let sink: &Sink<'_> = &sink;
        let notifier = Notifier::with_interval(sink, json!(7), Duration::ZERO);
        for phase in ["one", "two"] {
            notifier.start(phase, Some(40));
            std::thread::scope(|scope| {
                for _ in 0..4 {
                    scope.spawn(|| {
                        for _ in 0..10 {
                            notifier.message("Acme.Thing");
                            notifier.advance(1);
                        }
                    });
                }
            });
            notifier.finish();
        }
        let sent = lines(&shared);
        let values: Vec<u64> = sent
            .iter()
            .map(|line| line["params"]["progress"].as_u64().unwrap())
            .collect();
        assert!(
            values.windows(2).all(|pair| pair[0] < pair[1]),
            "{values:?}"
        );
        assert_eq!(*values.last().unwrap(), 80);
        assert!(sent.iter().all(|line| line["params"]["progressToken"] == 7));
    }

    #[test]
    fn only_strings_and_integers_are_tokens() {
        let token = |value: Value| token_of(&json!({ "_meta": { "progressToken": value } }));
        assert_eq!(token(json!("a")), Some(json!("a")));
        assert_eq!(token(json!(3)), Some(json!(3)));
        assert_eq!(token(json!(1.5)), None);
        assert_eq!(token(json!({})), None);
        assert_eq!(token_of(&json!({})), None);
    }
}
