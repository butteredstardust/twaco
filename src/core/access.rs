//! `twaco access explain`: can a user call a service of an entity, and why.
//!
//! The run-time verdict is the server's own: `CheckPermissionForUser` with `ServiceInvoke`, and
//! `GetPermissionsForUser` for the permission it found. Visibility is worked out from the
//! entity's visibility grants and the user's organizations and groups: a member of
//! Administrators sees everything, and anyone else sees an entity whose visibility names an
//! organization they belong to. Visibility an entity takes from its template is not read, so an
//! entity with no visibility of its own is "unknown", never "visible". Nothing is invoked: the
//! target service is not called, and every call made is a read.

use super::entity::key::ServiceTarget;
use super::server::ServerError;
use super::types::platform::Remote;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Allowed,
    Denied,
    Unknown,
}

impl Verdict {
    pub fn word(self) -> &'static str {
        match self {
            Verdict::Allowed => "allowed",
            Verdict::Denied => "denied",
            Verdict::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Explanation {
    pub user: String,
    pub entity: String,
    pub service: String,
    /// The whole answer: allowed only when the run-time check and visibility both allow.
    pub verdict: Verdict,
    /// `CheckPermissionForUser(ServiceInvoke)`, the server's run-time permission verdict.
    pub run_time: Verdict,
    pub visibility: Verdict,
    pub administrator: bool,
    pub organizations: Vec<String>,
    pub groups: Vec<String>,
    /// The organizations the entity's own visibility grants allow.
    pub visible_to: Vec<String>,
    /// Why, in order, one line each.
    pub reasons: Vec<String>,
}

/// Explain one user and one service. Read-only.
pub fn explain(
    remote: &dyn Remote,
    user: &str,
    collection: &str,
    name: &str,
    service: &str,
) -> Result<Explanation, String> {
    let entity = format!("{collection}/{name}");
    let target = ServiceTarget::entity(collection, name).map_err(|e| e.to_string())?;
    let person = ServiceTarget::entity("Users", user).map_err(|e| e.to_string())?;
    let mut reasons = Vec::new();

    // The permission check answers for any name, so a misspelt service would read as allowed.
    match remote.call(&target, "GetServiceDefinition", &json!({ "name": service })) {
        Err(ServerError::Http {
            status: 404, body, ..
        }) if body.contains("Service definition") => {
            return Err(format!("{entity} has no service {service}"));
        }
        Err(error) if error.is_not_found() => {
            return Err(format!("no entity {entity} on the server"));
        }
        _ => {}
    }

    let check = remote
        .call(
            &target,
            "CheckPermissionForUser",
            &json!({ "user": user, "type": "ServiceInvoke", "name": service }),
        )
        .map_err(|error| failed(&entity, "CheckPermissionForUser", &error))?;
    let run_time = match first(&check).and_then(|row| row["result"].as_bool()) {
        Some(true) => {
            reasons.push(format!("run time: {user} may invoke {service} on {entity}"));
            Verdict::Allowed
        }
        Some(false) => {
            reasons.push(format!(
                "run time: {user} may not invoke {service} on {entity}: no grant allows it, or one denies it"
            ));
            Verdict::Denied
        }
        None => {
            reasons.push("run time: CheckPermissionForUser gave no answer".to_string());
            Verdict::Unknown
        }
    };
    // What the server says it found for the service, to show beside the verdict.
    if let Ok(found) = remote.call(
        &target,
        "GetPermissionsForUser",
        &json!({ "user": user, "permissionName": "ServiceInvoke", "name": service }),
    ) {
        for row in rows(&found) {
            if let (Some(feature), Some(permitted)) =
                (row["name"].as_str(), row["isPermitted"].as_bool())
            {
                if feature != service {
                    reasons.push(format!(
                        "run time: the permission that applies is the one for {feature} ({})",
                        if permitted { "allow" } else { "deny" }
                    ));
                }
            }
        }
    }

    let names = |service: &str| -> Result<Vec<String>, String> {
        let reply = remote.call(&person, service, &json!({})).map_err(|error| {
            if error.is_not_found() {
                format!("no user {user} on the server")
            } else {
                failed(&format!("Users/{user}"), service, &error)
            }
        })?;
        let mut out: Vec<String> = rows(&reply)
            .filter_map(|row| row["name"].as_str().map(str::to_string))
            .collect();
        out.sort();
        Ok(out)
    };
    let organizations = names("GetOrganizations")?;
    let groups = names("GetGroups")?;
    let administrator = groups.iter().any(|group| group == "Administrators");

    let visibility_reply = remote
        .call(&target, "GetVisibilityPermissionsAsJSON", &json!({}))
        .map_err(|error| failed(&entity, "GetVisibilityPermissionsAsJSON", &error))?;
    let grants: Vec<&Value> = visibility_reply
        .as_ref()
        .and_then(|value| value["Visibility"].as_array())
        .map(|list| list.iter().collect())
        .unwrap_or_default();
    let visible_to: Vec<String> = grants
        .iter()
        .filter(|grant| grant["isPermitted"].as_bool() == Some(true))
        .filter(|grant| grant["type"].as_str() == Some("Organization"))
        .filter_map(|grant| grant["name"].as_str().map(str::to_string))
        .collect();
    let shared: Vec<&String> = organizations
        .iter()
        .filter(|organization| visible_to.contains(organization))
        .collect();
    // A deny wins over an allow, so denies are judged first; a grant to something other than a
    // whole organization (an organizational unit) cannot be resolved to its members here, so it
    // leaves the answer open rather than letting an allow stand.
    let denied_by: Option<&str> = grants
        .iter()
        .filter(|grant| grant["isPermitted"].as_bool() == Some(false))
        .filter(|grant| grant["type"].as_str() == Some("Organization"))
        .filter_map(|grant| grant["name"].as_str())
        .find(|name| {
            organizations
                .iter()
                .any(|organization| organization == name)
        });
    let unresolved = grants
        .iter()
        .any(|grant| grant["type"].as_str() != Some("Organization"));
    // The owner of an entity sees it, whatever its visibility says.
    let owner = remote
        .call(&target, "GetOwner", &json!({}))
        .ok()
        .and_then(|reply| {
            first(&reply)
                .and_then(|row| row["result"].as_str())
                .map(str::to_string)
        });
    let visibility = if administrator {
        reasons.push(format!(
            "visibility: {user} is in Administrators, who see every entity"
        ));
        Verdict::Allowed
    } else if owner.as_deref() == Some(user) {
        reasons.push(format!(
            "visibility: {user} owns {entity}, and an owner sees it"
        ));
        Verdict::Allowed
    } else if let Some(organization) = denied_by {
        reasons.push(format!(
            "visibility: {entity} denies visibility to {organization}, which {user} belongs to"
        ));
        Verdict::Denied
    } else if unresolved {
        reasons.push(format!(
            "visibility: {entity} has grants for organizational units, which twaco does not resolve to members"
        ));
        Verdict::Unknown
    } else if let Some(organization) = shared.first() {
        reasons.push(format!(
            "visibility: {entity} is visible to {organization}, which {user} belongs to"
        ));
        Verdict::Allowed
    } else if grants.is_empty() {
        reasons.push(format!(
            "visibility: {entity} grants none of its own; what its template grants its instances is not read here"
        ));
        Verdict::Unknown
    } else if owner.is_none() {
        reasons.push(format!(
            "visibility: none of {entity}'s organizations has {user}, but its owner could not be read, and an owner sees it"
        ));
        Verdict::Unknown
    } else {
        reasons.push(format!(
            "visibility: {entity} is visible to {}, none of which {user} belongs to{}",
            visible_to.join(", "),
            if organizations.is_empty() {
                String::new()
            } else {
                format!(" ({user} is in {})", organizations.join(", "))
            }
        ));
        Verdict::Denied
    };

    let verdict = match (run_time, visibility) {
        (Verdict::Denied, _) | (_, Verdict::Denied) => Verdict::Denied,
        (Verdict::Allowed, Verdict::Allowed) => Verdict::Allowed,
        _ => Verdict::Unknown,
    };
    Ok(Explanation {
        user: user.to_string(),
        entity,
        service: service.to_string(),
        verdict,
        run_time,
        visibility,
        administrator,
        organizations,
        groups,
        visible_to,
        reasons,
    })
}

fn failed(entity: &str, service: &str, error: &ServerError) -> String {
    format!("{entity}.{service}: {error}")
}

fn rows(reply: &Option<Value>) -> impl Iterator<Item = &Value> {
    reply
        .as_ref()
        .and_then(|value| value["rows"].as_array())
        .into_iter()
        .flatten()
}

fn first(reply: &Option<Value>) -> Option<&Value> {
    rows(reply).next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Replies by `<target>.<service>`.
    struct Fake(BTreeMap<String, Value>);

    impl Remote for Fake {
        fn call(
            &self,
            target: &ServiceTarget,
            service: &str,
            _: &Value,
        ) -> Result<Option<Value>, ServerError> {
            Ok(self.0.get(&format!("{target}.{service}")).cloned())
        }
    }

    fn table(rows: Value) -> Value {
        json!({ "rows": rows })
    }

    fn server(check: bool, organizations: &[&str], groups: &[&str], visible: Value) -> Fake {
        let names = |list: &[&str]| {
            table(Value::Array(
                list.iter().map(|n| json!({ "name": n })).collect(),
            ))
        };
        Fake(BTreeMap::from([
            (
                "Things/Acme.T.CheckPermissionForUser".to_string(),
                table(json!([{ "result": check }])),
            ),
            (
                "Things/Acme.T.GetPermissionsForUser".to_string(),
                table(json!([{ "name": "Run", "isPermitted": check }])),
            ),
            (
                "Users/ann.GetOrganizations".to_string(),
                names(organizations),
            ),
            ("Users/ann.GetGroups".to_string(), names(groups)),
            (
                "Things/Acme.T.GetVisibilityPermissionsAsJSON".to_string(),
                json!({ "Visibility": visible }),
            ),
            (
                "Things/Acme.T.GetOwner".to_string(),
                table(json!([{ "result": "bob" }])),
            ),
        ]))
    }

    #[test]
    fn a_missing_service_or_user_is_named_rather_than_judged() {
        struct Missing(&'static str, &'static str);
        impl Remote for Missing {
            fn call(
                &self,
                target: &ServiceTarget,
                service: &str,
                _: &Value,
            ) -> Result<Option<Value>, ServerError> {
                if format!("{target}.{service}") == self.0 {
                    return Err(ServerError::Http {
                        method: crate::core::server::Method::Post,
                        status: 404,
                        url: "http://server".to_string(),
                        body: self.1.to_string(),
                    });
                }
                Ok(Some(json!({ "rows": [] })))
            }
        }
        let service = Missing(
            "Things/Acme.T.GetServiceDefinition",
            "Service definition [Run] does not exist",
        );
        assert_eq!(
            explain(&service, "ann", "Things", "Acme.T", "Run").unwrap_err(),
            "Things/Acme.T has no service Run"
        );
        let user = Missing("Users/ann.GetOrganizations", "Entity Not Found : [ann]");
        assert_eq!(
            explain(&user, "ann", "Things", "Acme.T", "Run").unwrap_err(),
            "no user ann on the server"
        );
    }

    fn explained(fake: &Fake) -> Explanation {
        explain(fake, "ann", "Things", "Acme.T", "Run").unwrap()
    }

    #[test]
    fn allowed_needs_the_run_time_grant_and_visibility() {
        let visible = json!([{ "name": "Acme.Org", "type": "Organization", "isPermitted": true }]);
        let both = explained(&server(true, &["Acme.Org"], &[], visible.clone()));
        assert_eq!(
            (both.verdict, both.run_time, both.visibility),
            (Verdict::Allowed, Verdict::Allowed, Verdict::Allowed)
        );
        assert!(both
            .reasons
            .iter()
            .any(|r| r.contains("visible to Acme.Org, which ann belongs to")));

        let unseen = explained(&server(true, &["Other.Org"], &[], visible.clone()));
        assert_eq!(
            (unseen.verdict, unseen.visibility),
            (Verdict::Denied, Verdict::Denied)
        );

        let no_grant = explained(&server(false, &["Acme.Org"], &[], visible));
        assert_eq!(
            (no_grant.verdict, no_grant.run_time),
            (Verdict::Denied, Verdict::Denied)
        );
    }

    #[test]
    fn a_deny_wins_units_leave_it_open_and_an_owner_sees_it() {
        let allowed = json!({ "name": "Acme.Org", "type": "Organization", "isPermitted": true });
        let denied = json!({ "name": "Acme.Org", "type": "Organization", "isPermitted": false });
        let both = explained(&server(
            true,
            &["Acme.Org"],
            &[],
            json!([allowed.clone(), denied]),
        ));
        assert_eq!(both.visibility, Verdict::Denied, "{:?}", both.reasons);

        let unit =
            json!({ "name": "Acme.Org:Floor", "type": "OrganizationalUnit", "isPermitted": false });
        let open = explained(&server(true, &["Acme.Org"], &[], json!([allowed, unit])));
        assert_eq!(
            (open.verdict, open.visibility),
            (Verdict::Unknown, Verdict::Unknown)
        );

        let mut owned = server(
            true,
            &[],
            &[],
            json!([{ "name": "Other.Org", "type": "Organization", "isPermitted": true }]),
        );
        owned.0.insert(
            "Things/Acme.T.GetOwner".to_string(),
            table(json!([{ "result": "ann" }])),
        );
        let owner = explained(&owned);
        assert_eq!(
            (owner.verdict, owner.visibility),
            (Verdict::Allowed, Verdict::Allowed)
        );
    }

    #[test]
    fn administrators_see_everything_and_missing_visibility_is_unknown() {
        let admin = explained(&server(true, &[], &["Administrators"], json!([])));
        assert_eq!(
            (admin.verdict, admin.administrator),
            (Verdict::Allowed, true)
        );

        let inherited = explained(&server(true, &["Acme.Org"], &[], json!([])));
        assert_eq!(
            (inherited.verdict, inherited.visibility),
            (Verdict::Unknown, Verdict::Unknown)
        );
    }
}
