use super::super::server::Client;
use super::model::DbError;
use super::remote::Remote;
use serde::Serialize;
use serde_json::Value;

/// What sweeping stale temporary Things needs from a server.
pub trait Sweeper {
    fn thing_names(&self) -> Result<Vec<String>, String>;
    fn template_of(&self, name: &str) -> Result<String, String>;
    fn delete_thing(&self, name: &str) -> Result<(), String>;
    fn thing_exists(&self, name: &str) -> Result<bool, String>;
}

impl Sweeper for Client {
    fn thing_names(&self) -> Result<Vec<String>, String> {
        self.list_entity_names("Things")
            .map_err(|error| error.to_string())
    }

    fn template_of(&self, name: &str) -> Result<String, String> {
        let thing = Remote::thing(self, name)?;
        Ok(thing
            .get("thingTemplate")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    fn delete_thing(&self, name: &str) -> Result<(), String> {
        Remote::delete_thing(self, name)
    }

    fn thing_exists(&self, name: &str) -> Result<bool, String> {
        Remote::thing_exists(self, name)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SweepStatus {
    /// Found, and a plan leaves it.
    Stale,
    Deleted,
    /// Matches the name but is not a `Database` Thing: left alone.
    Skipped,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct Swept {
    pub name: String,
    pub status: SweepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// Whether a name is one `temporary_name` makes: `ZZ.Twaco.Sql.` and eight lowercase hex digits.
pub fn is_temporary_name(name: &str) -> bool {
    name.strip_prefix("ZZ.Twaco.Sql.").is_some_and(|rest| {
        rest.len() == 8
            && rest
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Find the temporary Things an interrupted run left, and with `apply` delete them, confirming
/// each is gone. Only a name twaco generates, on a `Database` Thing, is ever touched.
pub fn sweep(remote: &dyn Sweeper, apply: bool) -> Result<Vec<Swept>, DbError> {
    let mut names: Vec<String> = remote
        .thing_names()
        .map_err(|why| DbError(format!("cannot list the server's Things: {why}")))?
        .into_iter()
        .filter(|name| is_temporary_name(name))
        .collect();
    names.sort();
    let mut swept = Vec::new();
    for name in names {
        let template = remote
            .template_of(&name)
            .map_err(|why| DbError(format!("cannot read {name}: {why}")))?;
        if template != "Database" {
            swept.push(Swept {
                name,
                status: SweepStatus::Skipped,
                why: Some(format!("its template is {template:?}, not Database")),
            });
            continue;
        }
        if !apply {
            swept.push(Swept {
                name,
                status: SweepStatus::Stale,
                why: None,
            });
            continue;
        }
        let outcome = remote
            .delete_thing(&name)
            .and_then(|()| match remote.thing_exists(&name) {
                Ok(false) => Ok(()),
                Ok(true) => Err("it still exists after the delete".to_string()),
                Err(why) => Err(format!("cannot confirm the delete: {why}")),
            });
        swept.push(match outcome {
            Ok(()) => Swept {
                name,
                status: SweepStatus::Deleted,
                why: None,
            },
            Err(why) => Swept {
                name,
                status: SweepStatus::Failed,
                why: Some(why),
            },
        });
    }
    Ok(swept)
}
