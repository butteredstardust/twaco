//! Where each entity stands between the working copy, the server and the baseline.
//!
//! Shared by `twaco entity status` and the MCP `status` tool, so the two cannot disagree about
//! what "in sync" means.

use super::baseline::Baseline;
use super::entity_key::EntityKey;
use super::normalise;
use super::parallel;
use super::push::Remote;
use super::workspace::EntityFile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    InSync,
    LocalChanged,
    ServerChanged,
    BothChanged,
    NotOnServer,
    NoBaselineSame,
    NoBaselineDiffers,
}

impl Verdict {
    pub const ALL: [Verdict; 7] = [
        Verdict::InSync,
        Verdict::LocalChanged,
        Verdict::ServerChanged,
        Verdict::BothChanged,
        Verdict::NotOnServer,
        Verdict::NoBaselineSame,
        Verdict::NoBaselineDiffers,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Verdict::InSync => "in-sync",
            Verdict::LocalChanged => "local-changed",
            Verdict::ServerChanged => "server-changed",
            Verdict::BothChanged => "both-changed",
            Verdict::NotOnServer => "not-on-server",
            Verdict::NoBaselineSame => "no-baseline-same",
            Verdict::NoBaselineDiffers => "no-baseline-differs",
        }
    }

    /// Whether this verdict means the two sides need attention.
    pub fn is_drift(self) -> bool {
        !matches!(self, Verdict::InSync | Verdict::NoBaselineSame)
    }
}

/// Each side against its own recorded state (the two-sided baseline).
pub fn verdict(working: &str, server: Option<&str>, baseline: Option<(&str, &str)>) -> Verdict {
    let Some(server) = server else {
        return Verdict::NotOnServer;
    };
    let Some((local_baseline, server_baseline)) = baseline else {
        return if working == server {
            Verdict::NoBaselineSame
        } else {
            Verdict::NoBaselineDiffers
        };
    };
    match (working != local_baseline, server != server_baseline) {
        (false, false) => Verdict::InSync,
        (true, false) => Verdict::LocalChanged,
        (false, true) => Verdict::ServerChanged,
        (true, true) => Verdict::BothChanged,
    }
}

#[derive(Clone, Debug)]
pub struct EntityStatus {
    pub collection: String,
    pub name: String,
    pub working: String,
    pub server: Option<String>,
    pub local_baseline: Option<String>,
    pub server_baseline: Option<String>,
    pub verdict: Verdict,
}

/// Compare every entity with the server, in parallel. Per-entity failures are returned beside the
/// statuses rather than aborting the rest, in input order.
pub fn compute(
    remote: &(dyn Remote + Sync),
    baseline: &Baseline,
    entities: &[EntityFile],
) -> (Vec<EntityStatus>, Vec<String>) {
    let results = parallel::map(entities, |entity| {
        let working = std::fs::read(&entity.path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| normalise::hash(&bytes).map_err(|error| error.to_string()))
            .map_err(|error| format!("{}: {error}", entity.path.display()))?;
        let server = match EntityKey::address(&entity.info.collection, &entity.info.name)
            .and_then(|key| remote.fetch(&key))
        {
            Ok(Some(bytes)) => Some(
                normalise::hash(&bytes)
                    .map_err(|error| format!("server export for {}: {error}", entity.info.name))?,
            ),
            Ok(None) => None,
            Err(error) => return Err(format!("{}: {error}", entity.info.name)),
        };
        let ancestor = baseline.get(&entity.info.collection, &entity.info.name);
        Ok(EntityStatus {
            collection: entity.info.collection.clone(),
            name: entity.info.name.clone(),
            verdict: verdict(
                &working,
                server.as_deref(),
                ancestor.map(|entry| (entry.local.as_str(), entry.server.as_str())),
            ),
            working,
            server,
            local_baseline: ancestor.map(|entry| entry.local.clone()),
            server_baseline: ancestor.map(|entry| entry.server.clone()),
        })
    });
    let mut statuses = Vec::new();
    let mut failures = Vec::new();
    for result in results {
        match result {
            Ok(status) => statuses.push(status),
            Err(error) => failures.push(error),
        }
    }
    (statuses, failures)
}

/// Record a baseline for every entity whose working copy and server agree, and never for one
/// that differs. Returns how many were recorded.
pub fn record_matching(baseline: &mut Baseline, statuses: &[EntityStatus]) -> usize {
    let mut recorded = 0;
    for status in statuses {
        if status.server.as_deref() == Some(status.working.as_str()) {
            baseline.set(
                &status.collection,
                &status.name,
                status.working.clone(),
                status.working.clone(),
            );
            recorded += 1;
        }
    }
    recorded
}

/// How many entities have each verdict, in [`Verdict::ALL`] order.
pub fn counts(statuses: &[EntityStatus]) -> Vec<(Verdict, usize)> {
    Verdict::ALL
        .into_iter()
        .map(|verdict| {
            (
                verdict,
                statuses.iter().filter(|s| s.verdict == verdict).count(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_attributes_changes_against_the_baseline() {
        assert_eq!(verdict("a", Some("a"), Some(("a", "a"))), Verdict::InSync);
        assert_eq!(
            verdict("local", Some("server"), Some(("local", "server"))),
            Verdict::InSync
        );
        assert_eq!(
            verdict("b", Some("a"), Some(("a", "a"))),
            Verdict::LocalChanged
        );
        assert_eq!(
            verdict("a", Some("b"), Some(("a", "a"))),
            Verdict::ServerChanged
        );
        assert_eq!(
            verdict("b", Some("c"), Some(("a", "a"))),
            Verdict::BothChanged
        );
        assert_eq!(verdict("a", None, Some(("a", "a"))), Verdict::NotOnServer);
        assert_eq!(verdict("a", Some("a"), None), Verdict::NoBaselineSame);
        assert_eq!(verdict("a", Some("b"), None), Verdict::NoBaselineDiffers);
        assert!(!Verdict::NoBaselineSame.is_drift());
        assert!(Verdict::NoBaselineDiffers.is_drift());
    }

    #[test]
    fn record_selects_only_matching_working_and_server_hashes() {
        fn status(name: &str, working: &str, server: Option<&str>) -> EntityStatus {
            EntityStatus {
                collection: "Things".to_string(),
                name: name.to_string(),
                working: working.to_string(),
                server: server.map(str::to_string),
                local_baseline: None,
                server_baseline: None,
                verdict: verdict(working, server, None),
            }
        }

        let statuses = [
            status("Same", "v5:same", Some("v5:same")),
            status("Different", "v5:working", Some("v5:server")),
            status("Missing", "v5:working", None),
        ];
        let mut baseline = Baseline::default();
        assert_eq!(record_matching(&mut baseline, &statuses), 1);
        assert_eq!(
            baseline
                .get("Things", "Same")
                .map(|entry| entry.local.as_str()),
            Some("v5:same")
        );
        assert_eq!(
            baseline
                .get("Things", "Same")
                .map(|entry| entry.server.as_str()),
            Some("v5:same")
        );
        assert_eq!(baseline.get("Things", "Different"), None);
        assert_eq!(baseline.get("Things", "Missing"), None);
    }
}
