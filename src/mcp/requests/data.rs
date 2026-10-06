use super::common::{default_profile, default_true, free_object, Absent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DbRunRequest {
    /// A UTF-8 SQL file relative to the solution; give file or sql.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) file: Absent<String>,
    /// Inline SQL; give sql or file.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) sql: Absent<String>,
    /// Database Thing; otherwise resolve the one in the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) thing: Absent<String>,
    /// Prefix COMMIT; for statements such as CREATE DATABASE.
    #[serde(default)]
    pub(crate) no_transaction: bool,
    #[serde(default = "default_db_run_timeout")]
    #[schemars(range(min = 1))]
    pub(crate) timeout: u64,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

fn default_db_run_timeout() -> u64 {
    120
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DbQueryRequest {
    /// A UTF-8 SQL file relative to the solution; give file or sql.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) file: Absent<String>,
    /// Inline SQL; give sql or file.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) sql: Absent<String>,
    /// Database Thing; otherwise resolve the one in the solution.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) thing: Absent<String>,
    #[serde(default = "default_db_query_max_rows")]
    #[schemars(range(min = 1))]
    pub(crate) max_rows: u64,
    #[serde(default = "default_db_query_timeout")]
    #[schemars(range(min = 1))]
    pub(crate) timeout: u64,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

fn default_db_query_max_rows() -> u64 {
    500
}

fn default_db_query_timeout() -> u64 {
    120
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DbCleanRequest {
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatatableCopyRequest {
    /// The source DataTable's name.
    pub(crate) old: String,
    /// The target DataTable's name.
    pub(crate) new: String,
    /// Source field to target field.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) map: Absent<BTreeMap<String, String>>,
    #[serde(default)]
    pub(crate) drop_unmapped: bool,
    #[serde(default)]
    pub(crate) append: bool,
    #[serde(default = "default_datatable_copy_max_rows")]
    #[schemars(range(min = 1))]
    pub(crate) max_rows: u64,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

fn default_datatable_copy_max_rows() -> u64 {
    100000
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum ConfigTableAction {
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "diff")]
    Diff,
    #[serde(rename = "restore")]
    Restore,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigTableRequest {
    pub(crate) thing: String,
    pub(crate) table: String,
    #[serde(default = "default_config_table_action")]
    pub(crate) action: ConfigTableAction,
    /// restore only: the backup file, as `twaco config-table --backup` writes it.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) backup: Absent<String>,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

fn default_config_table_action() -> ConfigTableAction {
    ConfigTableAction::Read
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum LogsLevel {
    #[serde(rename = "TRACE")]
    Trace,
    #[serde(rename = "DEBUG")]
    Debug,
    #[serde(rename = "INFO")]
    Info,
    #[serde(rename = "WARN")]
    Warn,
    #[serde(rename = "ERROR")]
    Error,
}

impl LogsLevel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogsRequest {
    /// ApplicationLog, ScriptLog, CommunicationLog, ConfigurationLog or SecurityLog.
    pub(crate) log: String,
    /// How far back, ending now: a number and s, m, h or d. Not with from/to.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    #[schemars(extend("default" = "1h"))]
    pub(crate) since: Absent<String>,
    /// ISO-8601 with Z or an offset (2026-10-01T06:37:00Z); without one it is in the local time zone of the machine running twaco. Or `now`.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) from: Absent<String>,
    /// ISO-8601 or `now` (the default).
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) to: Absent<String>,
    /// This level and above.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) level: Absent<LogsLevel>,
    /// Entries whose message contains this text.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) grep: Absent<String>,
    /// A Java regex that must match the whole message. Not with grep.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) regex: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) user: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) thread: Absent<String>,
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) origin: Absent<String>,
    #[serde(default = "default_logs_limit")]
    #[schemars(range(min = 1))]
    pub(crate) limit: u64,
    #[serde(default)]
    pub(crate) oldest_first: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

fn default_logs_limit() -> u64 {
    200
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) enum LogLevelLevel {
    #[serde(rename = "TRACE")]
    Trace,
    #[serde(rename = "DEBUG")]
    Debug,
    #[serde(rename = "INFO")]
    Info,
    #[serde(rename = "WARN")]
    Warn,
    #[serde(rename = "ERROR")]
    Error,
}

impl LogLevelLevel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogLevelRequest {
    /// ApplicationLog, ScriptLog, CommunicationLog, ConfigurationLog or SecurityLog.
    pub(crate) log: String,
    /// Set this level. Omit, with reset false, to only read.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) level: Absent<LogLevelLevel>,
    /// A class or package within the log, such as com.thingworx.
    #[serde(default, skip_serializing_if = "Absent::is_absent")]
    pub(crate) sublogger: Absent<String>,
    /// Put the sublogger (or, without one, every sublogger) back to the log's level.
    #[serde(default)]
    pub(crate) reset: bool,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CallRequest {
    /// Collection/Name, or an entity of the solution by full or last-segment name (Manager), or a platform Thing's name.
    pub(crate) target: String,
    pub(crate) service: String,
    #[serde(default)]
    #[schemars(schema_with = "free_object")]
    pub(crate) parameters: Map<String, Value>,
    #[serde(default = "default_true")]
    pub(crate) dry_run: bool,
    #[serde(default = "default_call_timeout_seconds")]
    #[schemars(range(min = 1))]
    pub(crate) timeout_seconds: u64,
    /// Also return what the call wrote to ScriptLog and ApplicationLog, even when it fails. Waits up to 3 s for late entries. ScriptLog at WARN records no logger.info/debug; see log_level.
    #[serde(default)]
    pub(crate) with_logs: bool,
    /// Server profile name.
    #[serde(default = "default_profile")]
    pub(crate) profile: String,
    /// Return everything instead of the summary.
    #[serde(default)]
    pub(crate) detail: bool,
}

fn default_call_timeout_seconds() -> u64 {
    120
}
