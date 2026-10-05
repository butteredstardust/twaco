//! The server's logs: ApplicationLog, ScriptLog and the rest, read as Composer reads them.
//!
//! Composer's Monitoring page makes one call, `Logs/<log>/Services/QueryLogEntries`. What its
//! filters do was measured on a live server rather than taken from their names:
//!
//! - a level range needs both bounds; either alone is ignored;
//! - a plain search never matches, and a regex must match the whole message, so a substring
//!   search is a quoted regex wrapped in `.*`;
//! - an unquoted metacharacter such as `[` makes the server answer HTTP 500;
//! - the window must span at least five seconds;
//! - reaching `maxItems` truncates silently, so it is reported here.

use super::entity_key::ServiceTarget;
use super::server::{Client, ServerError};
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::Timestamp;
use serde_json::{json, Map, Value};
use std::fmt;
use std::time::Duration;

/// Log queries answer in well under a second; a stalled one must not hold `--with-logs` for
/// minutes past its 3 s budget.
const TIMEOUT: Duration = Duration::from_secs(15);
/// The shortest window the platform accepts.
pub const MIN_WINDOW_MS: i64 = 5_000;
pub const MAX_LIMIT: u64 = 10_000;

/// The logs the platform ships. Others are passed through; the server says if one is unknown.
pub const LOGS: [&str; 5] = ["ApplicationLog", "CommunicationLog", "ConfigurationLog", "ScriptLog", "SecurityLog"];

/// A log's own services, as a trait so this module is tested without a server.
pub trait Remote {
    fn service(&self, log: &str, service: &str, body: &Value) -> Result<Option<Value>, ServerError>;
}

impl Remote for Client {
    fn service(&self, log: &str, service: &str, body: &Value) -> Result<Option<Value>, ServerError> {
        let target = ServiceTarget::entity("Logs", log)?;
        self.call_service(&target, service, body, TIMEOUT)
    }
}

#[derive(Debug)]
pub enum LogsError {
    Remote(ServerError),
    Shape(String),
    Invalid(String),
}

impl fmt::Display for LogsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogsError::Remote(error) => write!(f, "{error}"),
            LogsError::Shape(why) => write!(f, "unexpected log query response: {why}"),
            LogsError::Invalid(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for LogsError {}

/// The levels, lowest first.
pub const LEVELS: [&str; 5] = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];

/// A level as the server spells it, from any case.
pub fn level(text: &str) -> Result<&'static str, LogsError> {
    let upper = text.to_ascii_uppercase();
    LEVELS
        .iter()
        .find(|level| **level == upper)
        .copied()
        .ok_or_else(|| LogsError::Invalid(format!("level must be one of {}, not {text:?}", LEVELS.join(", "))))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Search {
    /// A substring, quoted by twaco.
    Grep(String),
    /// A regex as given, which must match the whole message.
    Regex(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub log: String,
    pub from_ms: i64,
    pub to_ms: i64,
    /// This level and above.
    pub level: Option<&'static str>,
    pub search: Option<Search>,
    pub user: Option<String>,
    pub thread: Option<String>,
    pub origin: Option<String>,
    pub limit: u64,
    pub oldest_first: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub timestamp: i64,
    pub level: String,
    pub content: String,
    pub origin: String,
    pub instance: String,
    pub thread: String,
    pub user: String,
    pub session: String,
    pub platform_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub entries: Vec<Entry>,
    /// As many entries came back as were asked for, so there may be more.
    pub truncated: bool,
    /// The window actually queried, after widening.
    pub from_ms: i64,
    pub to_ms: i64,
    pub widened: bool,
}

/// A substring as the server's whole-message regex: anything, the text quoted, anything, with
/// `.` reaching across lines. A `\E` inside the text would end the quoting early, so it is
/// closed, written as a literal, and reopened.
pub fn grep_expression(text: &str) -> String {
    format!("(?s).*\\Q{}\\E.*", text.replace("\\E", "\\E\\\\E\\Q"))
}

/// The window, widened symmetrically to the platform's minimum when shorter.
fn window(from_ms: i64, to_ms: i64) -> (i64, i64, bool) {
    if to_ms - from_ms >= MIN_WINDOW_MS {
        return (from_ms, to_ms, false);
    }
    let middle = from_ms + (to_ms - from_ms) / 2;
    (middle - MIN_WINDOW_MS / 2, middle - MIN_WINDOW_MS / 2 + MIN_WINDOW_MS, true)
}

/// The request body, with only what is set. Returns whether the window was widened.
pub fn body(query: &Query) -> (Value, bool) {
    let (from_ms, to_ms, widened) = window(query.from_ms, query.to_ms);
    let mut body = Map::new();
    body.insert("startDate".into(), json!(iso(from_ms)));
    body.insert("endDate".into(), json!(iso(to_ms)));
    body.insert("maxItems".into(), json!(query.limit));
    if let Some(level) = query.level {
        // Both bounds, or the server ignores the range.
        body.insert("fromLogLevel".into(), json!(level));
        body.insert("toLogLevel".into(), json!("ERROR"));
    }
    match &query.search {
        Some(Search::Grep(text)) => {
            body.insert("searchExpression".into(), json!(grep_expression(text)));
            body.insert("isRegex".into(), json!(true));
        }
        Some(Search::Regex(expression)) => {
            body.insert("searchExpression".into(), json!(expression));
            body.insert("isRegex".into(), json!(true));
        }
        None => {}
    }
    for (key, value) in [("user", &query.user), ("thread", &query.thread), ("origin", &query.origin)] {
        if let Some(value) = value {
            body.insert(key.into(), json!(value));
        }
    }
    if query.oldest_first {
        body.insert("oldestFirst".into(), json!(true));
    }
    (Value::Object(body), widened)
}

pub fn query(remote: &dyn Remote, query: &Query) -> Result<Outcome, LogsError> {
    if query.limit == 0 || query.limit > MAX_LIMIT {
        return Err(LogsError::Invalid(format!("the limit must be between 1 and {MAX_LIMIT}")));
    }
    if query.to_ms < query.from_ms {
        return Err(LogsError::Invalid("the window ends before it starts".to_string()));
    }
    let (body, widened) = body(query);
    let (from_ms, to_ms, _) = window(query.from_ms, query.to_ms);
    let reply = remote
        .service(&query.log, "QueryLogEntries", &body)
        .map_err(LogsError::Remote)?
        .ok_or_else(|| LogsError::Shape("an empty body".to_string()))?;
    let rows = reply
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| LogsError::Shape("no rows".to_string()))?;
    let text = |row: &Value, key: &str| row.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let entries: Vec<Entry> = rows
        .iter()
        .map(|row| Entry {
            timestamp: row.get("timestamp").and_then(Value::as_f64).unwrap_or(0.0) as i64,
            level: text(row, "level"),
            content: text(row, "content"),
            origin: text(row, "origin"),
            instance: text(row, "instance"),
            thread: text(row, "thread"),
            user: text(row, "user"),
            session: text(row, "session"),
            platform_id: text(row, "platformId"),
        })
        .collect();
    let truncated = entries.len() as u64 >= query.limit;
    Ok(Outcome { entries, truncated, from_ms, to_ms, widened })
}

// ---- what a call logged ---------------------------------------------------------------------

/// The logs a service call writes to.
pub const CALL_LOGS: [&str; 2] = ["ScriptLog", "ApplicationLog"];

/// How long to look for a call's entries. Measured on localhost, an entry is readable about
/// 0.3 s after the call returns, so polling stops early once a poll finds nothing new.
#[derive(Debug, Clone, Copy)]
pub struct Wait {
    pub interval: Duration,
    /// Keep polling at least this long after the call.
    pub settle: Duration,
    /// And never longer than this, so delayed log delivery cannot stall a call indefinitely.
    pub most: Duration,
}

impl Default for Wait {
    fn default() -> Self {
        Wait { interval: Duration::from_millis(500), settle: Duration::from_secs(1), most: Duration::from_secs(3) }
    }
}

/// Entries the call wrote: those stamped from its start (less a second for clocks) until now,
/// in every one of `CALL_LOGS`, oldest first, each with its log.
pub fn during_call(
    remote: &dyn Remote,
    start_ms: i64,
    end_ms: i64,
    wait: Wait,
    now: &dyn Fn() -> i64,
    sleep: &dyn Fn(Duration),
) -> Result<Vec<(String, Entry)>, LogsError> {
    // From the call's start to its end, with a second's slack each side for clocks and for an
    // entry written as the response leaves. Not everything that arrives while polling: other
    // traffic logs too.
    let since = start_ms - 1_000;
    let until = end_ms + 1_000;
    let read = || -> Result<Vec<(String, Entry)>, LogsError> {
        let mut found = Vec::new();
        for log in CALL_LOGS {
            let query = Query {
                log: log.to_string(),
                from_ms: since,
                to_ms: now() + 1_000,
                level: None,
                search: None,
                user: None,
                thread: None,
                origin: None,
                limit: 500,
                oldest_first: true,
            };
            for entry in self::query(remote, &query)?.entries {
                if (since..=until).contains(&entry.timestamp) {
                    found.push((log.to_string(), entry));
                }
            }
        }
        found.sort_by_key(|(_, entry)| entry.timestamp);
        Ok(found)
    };
    let mut seen = read()?;
    loop {
        let waited = Duration::from_millis(u64::try_from(now() - end_ms).unwrap_or(0));
        if waited >= wait.most {
            return Ok(seen);
        }
        sleep(wait.interval);
        let again = read()?;
        let settled = again.len() == seen.len();
        seen = again;
        let waited = Duration::from_millis(u64::try_from(now() - end_ms).unwrap_or(0));
        if settled && waited >= wait.settle {
            return Ok(seen);
        }
    }
}

// ---- levels ---------------------------------------------------------------------------------

/// A log's level, and the levels set on its subloggers (a class or package within it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Levels {
    pub level: String,
    /// Sorted by name.
    pub subloggers: Vec<(String, String)>,
}

/// What a level change asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The log's level, or one sublogger's.
    Set { level: &'static str, sublogger: Option<String> },
    /// One sublogger back to its parent's level, or all of them.
    Reset { sublogger: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeReport {
    pub before: Levels,
    /// Read back after an applied change; `None` for a plan.
    pub after: Option<Levels>,
    /// What changes, in words: "ScriptLog: WARN -> DEBUG".
    pub plan: String,
    /// How to put it back, as twaco commands: one for most changes, one per override removed
    /// for a reset of every sublogger.
    pub undo: Vec<String>,
}

fn rows<'a>(reply: &'a Option<Value>, what: &str) -> Result<&'a Vec<Value>, LogsError> {
    reply
        .as_ref()
        .and_then(|value| value.get("rows"))
        .and_then(Value::as_array)
        .ok_or_else(|| LogsError::Shape(format!("{what} returned no rows")))
}

pub fn levels(remote: &dyn Remote, log: &str) -> Result<Levels, LogsError> {
    let reply = remote.service(log, "GetLogLevel", &json!({})).map_err(LogsError::Remote)?;
    let level = rows(&reply, "GetLogLevel")?
        .first()
        .and_then(|row| row.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(|| LogsError::Shape("GetLogLevel returned no level".to_string()))?
        .to_string();
    let reply = remote.service(log, "GetSubLoggerLevels", &json!({})).map_err(LogsError::Remote)?;
    let mut subloggers: Vec<(String, String)> = rows(&reply, "GetSubLoggerLevels")?
        .iter()
        .filter_map(|row| {
            Some((row.get("fieldName")?.as_str()?.to_string(), row.get("fieldValue")?.as_str()?.to_string()))
        })
        .collect();
    subloggers.sort();
    Ok(Levels { level, subloggers })
}

/// Change a level: read what is there, and unless `apply`, stop at the plan. Applied, the
/// change is read back and must show, or it is an error. The server's level is everyone's, so
/// the report says how to put it back.
pub fn change(remote: &dyn Remote, log: &str, change: &Change, apply: bool) -> Result<ChangeReport, LogsError> {
    let before = levels(remote, log)?;
    let current = |sublogger: &Option<String>| match sublogger {
        None => Some(before.level.clone()),
        Some(name) => before.subloggers.iter().find(|(n, _)| n == name).map(|(_, l)| l.clone()),
    };
    let (plan, undo) = match change {
        Change::Set { level, sublogger: None } => (
            format!("{log}: {} -> {level}", before.level),
            vec![format!("twaco logs level {log} {} --apply", before.level)],
        ),
        Change::Set { level, sublogger: Some(name) } => match current(&Some(name.clone())) {
            Some(was) => (
                format!("{log} sublogger {name}: {was} -> {level}"),
                vec![format!("twaco logs level {log} {was} --sublogger {name} --apply")],
            ),
            None => (
                format!("{log} sublogger {name}: (inherits {}) -> {level}", before.level),
                vec![format!("twaco logs level {log} --reset --sublogger {name} --apply")],
            ),
        },
        Change::Reset { sublogger: Some(name) } => (
            format!("{log} sublogger {name}: {} -> inherits {}", current(&Some(name.clone())).unwrap_or_else(|| "(not set)".into()), before.level),
            current(&Some(name.clone())).map(|was| format!("twaco logs level {log} {was} --sublogger {name} --apply")).into_iter().collect(),
        ),
        // Every override goes, so putting it back is one command per override.
        Change::Reset { sublogger: None } => (
            format!("{log}: {} sublogger level(s) -> inherit {}", before.subloggers.len(), before.level),
            before
                .subloggers
                .iter()
                .map(|(name, level)| format!("twaco logs level {log} {level} --sublogger {name} --apply"))
                .collect(),
        ),
    };
    if !apply {
        return Ok(ChangeReport { before, after: None, plan, undo });
    }
    let (service, body) = match change {
        Change::Set { level, sublogger: None } => ("SetLogLevel", json!({ "level": level })),
        Change::Set { level, sublogger: Some(name) } => ("SetSubLoggerLevel", json!({ "sublogger": name, "level": level })),
        Change::Reset { sublogger: Some(name) } => ("ResetSubLoggerLevel", json!({ "sublogger": name })),
        Change::Reset { sublogger: None } => ("ResetAllSubLoggerLevels", json!({})),
    };
    remote.service(log, service, &body).map_err(LogsError::Remote)?;
    let after = levels(remote, log)?;
    let took = match change {
        Change::Set { level, sublogger: None } => after.level == *level,
        Change::Set { level, sublogger: Some(name) } => {
            after.subloggers.iter().any(|(n, l)| n == name && l == level)
        }
        // A reset sublogger reads back as its parent's level, or not at all.
        Change::Reset { sublogger: Some(name) } => {
            after.subloggers.iter().all(|(n, l)| n != name || *l == after.level)
        }
        Change::Reset { sublogger: None } => after.subloggers.iter().all(|(_, l)| *l == after.level),
    };
    if !took {
        return Err(LogsError::Shape(format!(
            "{service} was sent, but reading the levels back does not show it ({plan}); the server holds {}",
            describe(&after)
        )));
    }
    Ok(ChangeReport { before, after: Some(after), plan, undo })
}

/// Levels in one line: `WARN (subloggers: com.thingworx=WARN, ...)`.
pub fn describe(levels: &Levels) -> String {
    if levels.subloggers.is_empty() {
        return levels.level.clone();
    }
    let subs: Vec<String> = levels.subloggers.iter().map(|(n, l)| format!("{n}={l}")).collect();
    format!("{} (subloggers: {})", levels.level, subs.join(", "))
}

/// `90s`, `15m`, `1h`, `2d` as milliseconds.
pub fn parse_since(text: &str) -> Result<i64, LogsError> {
    let bad = || LogsError::Invalid(format!("since must be a number and a unit (s, m, h or d), such as 15m, not {text:?}"));
    let text = text.trim();
    let (number, unit) = text.split_at(text.len().checked_sub(1).ok_or_else(bad)?);
    let number: i64 = number.parse().map_err(|_| bad())?;
    let unit_ms = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(bad()),
    };
    if number <= 0 {
        return Err(bad());
    }
    number.checked_mul(unit_ms).ok_or_else(bad)
}

/// `now`, or an ISO-8601 time. One with an offset or `Z` is that instant; one without, such as
/// `2026-10-01T09:37`, is in the local time zone, as Composer's date pickers are.
pub fn parse_time(text: &str, now_ms: i64) -> Result<i64, LogsError> {
    parse_time_in(text, now_ms, &TimeZone::system())
}

fn parse_time_in(text: &str, now_ms: i64, zone: &TimeZone) -> Result<i64, LogsError> {
    let bad = |why: String| {
        LogsError::Invalid(format!(
            "a time is `now` or ISO-8601 such as 2026-10-01T09:37 (local) or 2026-10-01T06:37:00Z, not {text:?}: {why}"
        ))
    };
    let text = text.trim();
    if text.eq_ignore_ascii_case("now") {
        return Ok(now_ms);
    }
    if let Ok(instant) = text.parse::<Timestamp>() {
        return Ok(instant.as_millisecond());
    }
    let civil: DateTime = text.parse().map_err(|e: jiff::Error| bad(e.to_string()))?;
    civil
        .to_zoned(zone.clone())
        .map(|zoned| zoned.timestamp().as_millisecond())
        .map_err(|e| bad(e.to_string()))
}

/// Epoch milliseconds as the server takes them: `2026-10-01T09:37:37.473Z`.
pub fn iso(ms: i64) -> String {
    match Timestamp::from_millisecond(ms) {
        Ok(instant) => instant.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        Err(_) => ms.to_string(),
    }
}

/// Epoch milliseconds in the local time zone, with its offset, for a person reading the CLI.
pub fn local(ms: i64) -> String {
    local_in(ms, &TimeZone::system())
}

fn local_in(ms: i64, zone: &TimeZone) -> String {
    match Timestamp::from_millisecond(ms) {
        Ok(instant) => instant.to_zoned(zone.clone()).strftime("%Y-%m-%d %H:%M:%S%.3f%:z").to_string(),
        Err(_) => ms.to_string(),
    }
}

pub fn now_ms() -> i64 {
    Timestamp::now().as_millisecond()
}

/// One entry as a line: time, level, origin, content; later lines of the content indented.
pub fn line(entry: &Entry) -> String {
    let mut lines = entry.content.lines();
    let first = lines.next().unwrap_or_default();
    let mut out = format!("{} {:<5} [{}] {first}", local(entry.timestamp), entry.level, entry.origin);
    for rest in lines {
        out.push_str("\n  ");
        out.push_str(rest);
    }
    out
}

pub fn entry_json(entry: &Entry) -> Value {
    json!({
        "timestamp": entry.timestamp,
        "time": iso(entry.timestamp),
        "level": entry.level,
        "content": entry.content,
        "origin": entry.origin,
        "instance": entry.instance,
        "thread": entry.thread,
        "user": entry.user,
        "session": entry.session,
        "platformId": entry.platform_id,
    })
}

/// What an agent reads first: how many, at which levels, from where, which messages repeat,
/// and the newest few in full. `detail` gives every entry instead of the newest few.
pub fn summary(log: &str, outcome: &Outcome, detail: bool) -> Value {
    use std::collections::BTreeMap;
    let mut by_level: BTreeMap<&str, usize> = BTreeMap::new();
    let mut origins: BTreeMap<&str, usize> = BTreeMap::new();
    // Keyed by the message, keeping its first-seen order for ties.
    let mut repeats: Vec<(&Entry, usize)> = Vec::new();
    for entry in &outcome.entries {
        *by_level.entry(entry.level.as_str()).or_default() += 1;
        *origins.entry(entry.origin.as_str()).or_default() += 1;
        match repeats.iter_mut().find(|(seen, _)| seen.content == entry.content) {
            Some((seen, count)) => {
                *count += 1;
                if entry.timestamp > seen.timestamp {
                    *seen = entry;
                }
            }
            None => repeats.push((entry, 1)),
        }
    }
    let mut top_origins: Vec<(&str, usize)> = origins.into_iter().collect();
    top_origins.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let mut repeated: Vec<&(&Entry, usize)> = repeats.iter().filter(|(_, count)| *count > 1).collect();
    repeated.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    let mut newest: Vec<&Entry> = outcome.entries.iter().collect();
    newest.sort_by_key(|entry| std::cmp::Reverse(entry.timestamp));
    let mut result = json!({
        "ok": true,
        "log": log,
        "from": iso(outcome.from_ms),
        "to": iso(outcome.to_ms),
        "entries": outcome.entries.len(),
        "truncated": outcome.truncated,
        "by_level": by_level,
        "top_origins": top_origins.iter().take(5).map(|(origin, count)| json!({ "origin": origin, "count": count })).collect::<Vec<_>>(),
        "repeated": repeated.iter().take(10).map(|(entry, count)| json!({
            "content": entry.content.chars().take(200).collect::<String>(),
            "count": count,
            "level": entry.level,
            "last": iso(entry.timestamp),
        })).collect::<Vec<_>>(),
    });
    if outcome.widened {
        result["note"] = json!("the window was widened to the platform's 5 s minimum");
    }
    if outcome.truncated {
        result["truncated_note"] = json!("the limit was reached; there may be more entries in the window");
    }
    if detail {
        result["entries_list"] = Value::Array(outcome.entries.iter().map(entry_json).collect());
    } else {
        result["newest"] = Value::Array(newest.iter().take(10).map(|e| entry_json(e)).collect());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        rows: Vec<Value>,
        bodies: RefCell<Vec<(String, Value)>>,
    }

    impl Remote for Fake {
        fn service(&self, log: &str, service: &str, body: &Value) -> Result<Option<Value>, ServerError> {
            assert_eq!(service, "QueryLogEntries");
            self.bodies.borrow_mut().push((log.to_string(), body.clone()));
            Ok(Some(json!({ "rows": self.rows })))
        }
    }

    /// A log whose levels behave as the platform's do, recording every service called.
    struct LevelFake {
        level: RefCell<String>,
        subloggers: RefCell<Vec<(String, String)>>,
        calls: RefCell<Vec<String>>,
        ignore_sets: bool,
    }

    impl LevelFake {
        fn new() -> Self {
            LevelFake {
                level: RefCell::new("WARN".into()),
                subloggers: RefCell::new(vec![("com.thingworx".into(), "WARN".into()), ("com.x".into(), "INFO".into())]),
                calls: RefCell::new(Vec::new()),
                ignore_sets: false,
            }
        }
    }

    impl Remote for LevelFake {
        fn service(&self, _: &str, service: &str, body: &Value) -> Result<Option<Value>, ServerError> {
            self.calls.borrow_mut().push(service.to_string());
            let text = |key: &str| body[key].as_str().unwrap().to_string();
            match service {
                "GetLogLevel" => Ok(Some(json!({ "rows": [{ "name": *self.level.borrow() }] }))),
                "GetSubLoggerLevels" => Ok(Some(json!({ "rows": self.subloggers.borrow().iter()
                    .map(|(n, l)| json!({ "fieldName": n, "fieldValue": l })).collect::<Vec<_>>() }))),
                _ if self.ignore_sets => Ok(None),
                "SetLogLevel" => {
                    *self.level.borrow_mut() = text("level");
                    Ok(None)
                }
                "SetSubLoggerLevel" => {
                    let (name, level) = (text("sublogger"), text("level"));
                    let mut subs = self.subloggers.borrow_mut();
                    match subs.iter_mut().find(|(n, _)| *n == name) {
                        Some(entry) => entry.1 = level,
                        None => subs.push((name, level)),
                    }
                    Ok(None)
                }
                "ResetSubLoggerLevel" => {
                    let name = text("sublogger");
                    self.subloggers.borrow_mut().retain(|(n, _)| *n != name);
                    Ok(None)
                }
                "ResetAllSubLoggerLevels" => {
                    self.subloggers.borrow_mut().clear();
                    Ok(None)
                }
                other => panic!("unexpected service {other}"),
            }
        }
    }

    /// Entries that become readable over time, as the platform's do.
    struct Late {
        arrivals: Vec<(i64, &'static str, Value)>,
        clock: RefCell<i64>,
        queries: RefCell<usize>,
    }

    impl Remote for Late {
        fn service(&self, log: &str, _: &str, _: &Value) -> Result<Option<Value>, ServerError> {
            *self.queries.borrow_mut() += 1;
            let now = *self.clock.borrow();
            let rows: Vec<Value> =
                self.arrivals.iter().filter(|(at, l, _)| *at <= now && *l == log).map(|(_, _, row)| row.clone()).collect();
            Ok(Some(json!({ "rows": rows })))
        }
    }

    #[test]
    fn a_calls_entries_are_gathered_until_they_settle_and_never_past_three_seconds() {
        // The call ran from 10.000 s to 10.400 s; its entries arrive 0.3 s and 0.8 s later.
        let late = Late {
            arrivals: vec![
                (10_700, "ScriptLog", row(10_350, "ERROR", "s", "boom")),
                (11_200, "ApplicationLog", row(10_380, "WARN", "a", "after")),
                (0, "ScriptLog", row(5_000, "ERROR", "s", "long before the call")),
            ],
            clock: RefCell::new(10_400),
            queries: RefCell::new(0),
        };
        let now = || *late.clock.borrow();
        let sleep = |d: Duration| *late.clock.borrow_mut() += d.as_millis() as i64;
        let found = during_call(&late, 10_000, 10_400, Wait::default(), &now, &sleep).unwrap();
        let contents: Vec<&str> = found.iter().map(|(_, e)| e.content.as_str()).collect();
        assert_eq!(contents, ["boom", "after"]);
        assert_eq!(found[1].0, "ApplicationLog");
        // Settled at 1.4 s after the call: polls at 0, 0.5, 1.0 and 1.5 s.
        assert!(*late.clock.borrow() - 10_400 <= 1_500, "stopped at {}", *late.clock.borrow() - 10_400);

        // Something that keeps logging is cut off at 3 s.
        let busy = Late { arrivals: Vec::new(), clock: RefCell::new(0), queries: RefCell::new(0) };
        let arrivals: Vec<(i64, &'static str, Value)> =
            (0..100).map(|i| (i * 100, "ScriptLog", row(i * 100, "INFO", "s", "tick"))).collect();
        let busy = Late { arrivals, ..busy };
        let now = || *busy.clock.borrow();
        let sleep = |d: Duration| *busy.clock.borrow_mut() += d.as_millis() as i64;
        during_call(&busy, 0, 0, Wait::default(), &now, &sleep).unwrap();
        assert!(*busy.clock.borrow() <= 3_000, "waited {} ms", *busy.clock.borrow());
    }

    #[test]
    fn levels_are_read_with_their_subloggers_sorted() {
        let fake = LevelFake::new();
        let levels = levels(&fake, "ScriptLog").unwrap();
        assert_eq!(levels.level, "WARN");
        assert_eq!(levels.subloggers[0].0, "com.thingworx");
        assert_eq!(describe(&levels), "WARN (subloggers: com.thingworx=WARN, com.x=INFO)");
    }

    #[test]
    fn a_level_change_is_a_plan_unless_applied() {
        let fake = LevelFake::new();
        let set = Change::Set { level: "DEBUG", sublogger: None };
        let report = change(&fake, "ScriptLog", &set, false).unwrap();
        assert_eq!(report.plan, "ScriptLog: WARN -> DEBUG");
        assert_eq!(report.undo, ["twaco logs level ScriptLog WARN --apply"]);
        assert!(report.after.is_none());
        assert!(fake.calls.borrow().iter().all(|c| c.starts_with("Get")), "{:?}", fake.calls.borrow());
        assert_eq!(*fake.level.borrow(), "WARN");

        let report = change(&fake, "ScriptLog", &set, true).unwrap();
        assert_eq!(report.after.unwrap().level, "DEBUG");
        assert!(fake.calls.borrow().contains(&"SetLogLevel".to_string()));
    }

    #[test]
    fn sublogger_changes_and_resets_say_how_to_undo_them() {
        let fake = LevelFake::new();
        let report = change(&fake, "ScriptLog", &Change::Set { level: "TRACE", sublogger: Some("com.x".into()) }, true).unwrap();
        assert_eq!(report.plan, "ScriptLog sublogger com.x: INFO -> TRACE");
        assert_eq!(report.undo, ["twaco logs level ScriptLog INFO --sublogger com.x --apply"]);
        let fresh = change(&fake, "ScriptLog", &Change::Set { level: "DEBUG", sublogger: Some("com.new".into()) }, false).unwrap();
        assert_eq!(fresh.undo, ["twaco logs level ScriptLog --reset --sublogger com.new --apply"]);
        change(&fake, "ScriptLog", &Change::Reset { sublogger: Some("com.x".into()) }, true).unwrap();
        assert!(fake.subloggers.borrow().iter().all(|(n, _)| n != "com.x"));
        // A reset of every sublogger says how to restore each override it removes.
        let everything = change(&fake, "ScriptLog", &Change::Reset { sublogger: None }, false).unwrap();
        assert_eq!(everything.undo, ["twaco logs level ScriptLog WARN --sublogger com.thingworx --apply"]);
        change(&fake, "ScriptLog", &Change::Reset { sublogger: None }, true).unwrap();
        assert!(fake.subloggers.borrow().is_empty());
    }

    #[test]
    fn a_change_the_server_did_not_take_is_an_error() {
        let mut fake = LevelFake::new();
        fake.ignore_sets = true;
        let error = change(&fake, "ScriptLog", &Change::Set { level: "DEBUG", sublogger: None }, true).unwrap_err();
        assert!(error.to_string().contains("does not show it"), "{error}");
    }

    fn row(ms: i64, level: &str, origin: &str, content: &str) -> Value {
        json!({ "timestamp": ms, "level": level, "origin": origin, "content": content, "thread": "t", "user": "u" })
    }

    fn query_for(from_ms: i64, to_ms: i64) -> Query {
        Query {
            log: "ScriptLog".into(),
            from_ms,
            to_ms,
            level: None,
            search: None,
            user: None,
            thread: None,
            origin: None,
            limit: 100,
            oldest_first: false,
        }
    }

    #[test]
    fn a_level_is_sent_as_both_bounds_and_only_set_fields_are_sent() {
        let mut q = query_for(0, 60_000);
        let (plain, _) = body(&q);
        let keys: Vec<&String> = plain.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["startDate", "endDate", "maxItems"]);
        q.level = Some(level("warn").unwrap());
        q.user = Some("Administrator".into());
        q.oldest_first = true;
        let (b, _) = body(&q);
        assert_eq!(b["fromLogLevel"], "WARN");
        assert_eq!(b["toLogLevel"], "ERROR");
        assert_eq!(b["user"], "Administrator");
        assert_eq!(b["oldestFirst"], true);
        assert!(b.get("sortFieldName").is_none() && b.get("ascendingSearch").is_none());
        assert!(level("loud").is_err());
    }

    #[test]
    fn grep_is_quoted_so_it_neither_errors_nor_misses() {
        assert_eq!(grep_expression("[Acme.Thing"), "(?s).*\\Q[Acme.Thing\\E.*");
        // A \E inside the text cannot close the quoting early.
        assert_eq!(grep_expression("a\\Eb"), "(?s).*\\Qa\\E\\\\E\\Qb\\E.*");
        let mut q = query_for(0, 60_000);
        q.search = Some(Search::Grep("line one\nline two".into()));
        let (b, _) = body(&q);
        assert_eq!(b["isRegex"], true);
        assert!(b["searchExpression"].as_str().unwrap().starts_with("(?s)"));
        q.search = Some(Search::Regex(".*boom.*".into()));
        assert_eq!(body(&q).0["searchExpression"], ".*boom.*");
    }

    #[test]
    fn a_short_window_is_widened_to_five_seconds_around_its_middle() {
        let (b, widened) = body(&query_for(10_000, 11_000));
        assert!(widened);
        assert_eq!(b["startDate"], iso(8_000));
        assert_eq!(b["endDate"], iso(13_000));
        assert!(!body(&query_for(0, 5_000)).1);
    }

    #[test]
    fn times_round_trip_through_iso() {
        let ms = parse_time("2026-10-01T09:37:37.473Z", 0).unwrap();
        assert_eq!(iso(ms), "2026-10-01T09:37:37.473Z");
        assert_eq!(parse_time("2026-10-01T12:37:37.473+03:00", 0).unwrap(), ms);
        // Without an offset, a time is in the given zone.
        let utc = TimeZone::UTC;
        assert_eq!(parse_time_in("2026-10-01T09:37", 0, &utc).unwrap(), parse_time("2026-10-01T09:37:00Z", 0).unwrap());
        let athens = TimeZone::get("Europe/Athens").unwrap();
        assert_eq!(parse_time_in("2026-10-01T12:37:37.473", 0, &athens).unwrap(), ms);
        assert_eq!(local_in(ms, &athens), "2026-10-01 12:37:37.473+03:00");
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(parse_time("now", 42).unwrap(), 42);
        assert!(parse_time("yesterday", 0).is_err());
        assert!(parse_time("2026-13-01T00:00", 0).is_err());
    }

    #[test]
    fn since_takes_a_number_and_a_unit() {
        assert_eq!(parse_since("90s").unwrap(), 90_000);
        assert_eq!(parse_since("15m").unwrap(), 900_000);
        assert_eq!(parse_since("1h").unwrap(), 3_600_000);
        assert_eq!(parse_since("2d").unwrap(), 172_800_000);
        for bad in ["", "h", "1", "1w", "-1h", "0m", "x5m"] {
            assert!(parse_since(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn reaching_the_limit_is_reported_as_truncated() {
        let fake = Fake { rows: (0..3).map(|i| row(i, "ERROR", "o", "x")).collect(), bodies: RefCell::new(Vec::new()) };
        let mut q = query_for(0, 60_000);
        q.limit = 3;
        assert!(query(&fake, &q).unwrap().truncated);
        q.limit = 4;
        assert!(!query(&fake, &q).unwrap().truncated);
        assert_eq!(fake.bodies.borrow()[0].0, "ScriptLog");
        q.limit = 0;
        assert!(query(&fake, &q).is_err());
    }

    #[test]
    fn a_multi_line_message_keeps_its_lines_indented() {
        let entry = Entry {
            timestamp: 0,
            level: "ERROR".into(),
            content: "first\nsecond".into(),
            origin: "o".into(),
            instance: String::new(),
            thread: String::new(),
            user: String::new(),
            session: String::new(),
            platform_id: String::new(),
        };
        assert_eq!(line(&entry), format!("{} ERROR [o] first\n  second", local(0)));
    }

    #[test]
    fn the_summary_counts_levels_origins_and_repeats() {
        let fake = Fake {
            rows: vec![
                row(3, "ERROR", "a", "boom"),
                row(2, "WARN", "b", "slow"),
                row(1, "ERROR", "a", "boom"),
                row(0, "ERROR", "c", "once"),
            ],
            bodies: RefCell::new(Vec::new()),
        };
        let outcome = query(&fake, &query_for(0, 60_000)).unwrap();
        let s = summary("ScriptLog", &outcome, false);
        assert_eq!(s["entries"], 4);
        assert_eq!(s["by_level"]["ERROR"], 3);
        assert_eq!(s["top_origins"][0]["origin"], "a");
        assert_eq!(s["top_origins"][0]["count"], 2);
        assert_eq!(s["repeated"].as_array().unwrap().len(), 1);
        assert_eq!(s["repeated"][0]["content"], "boom");
        assert_eq!(s["repeated"][0]["count"], 2);
        assert_eq!(s["repeated"][0]["last"], iso(3));
        assert_eq!(s["newest"][0]["timestamp"], 3);
        assert!(summary("ScriptLog", &outcome, true).get("entries_list").is_some());
    }
}
