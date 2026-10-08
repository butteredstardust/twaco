use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Run,
    Query,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub mode: Mode,
    pub thing: Option<String>,
    pub apply: bool,
    pub no_transaction: bool,
    pub max_rows: u64,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub applied: bool,
    pub mode: Mode,
    pub thing: String,
    pub jdbc_url: String,
    pub user: String,
    pub bytes: usize,
    pub sql: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temporary_thing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DbError(pub(crate) String);
