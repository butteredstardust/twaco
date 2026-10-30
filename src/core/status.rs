//! Where each entity stands between the working copy, the server and the baseline.
//!
//! Shared by `twaco entity status` and the MCP `status` tool, so the two cannot disagree about
//! what "in sync" means.

use super::baseline::Baseline;
use super::entity::key::EntityKey;
use super::normalise;
use super::parallel;
use super::progress::{self, Progress};
use super::push::Remote;
use super::workspace::EntityFile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    InSync,
    /// Both sides changed since the baseline, but now contain the same normalised content.
    Converged,
    LocalChanged,
    ServerChanged,
    BothChanged,
    NotOnServer,
    NoBaselineSame,
    NoBaselineDiffers,
}

impl Verdict {
    pub(crate) const ALL: [Verdict; 8] = [
        Verdict::InSync,
        Verdict::Converged,
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
            Verdict::Converged => "converged",
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
        !matches!(
            self,
            Verdict::InSync | Verdict::Converged | Verdict::NoBaselineSame
        )
    }
}

/// Each side against its own recorded state (the two-sided baseline).
pub(crate) fn verdict(
    working: &str,
    server: Option<&str>,
    baseline: Option<(&str, &str)>,
) -> Verdict {
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
        (true, true) if working == server => Verdict::Converged,
        (true, true) => Verdict::BothChanged,
    }
}

#[derive(Clone, Debug)]
pub struct EntityStatus {
    pub server_owned: Vec<String>,
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
///
/// It reports one step per entity. Messages hold entity names only.
pub(crate) fn compute_with_progress(
    remote: &(dyn Remote + Sync),
    baseline: &Baseline,
    entities: &[EntityFile],
    rules: &[super::server_owned::Rule],
    patches: &[super::config::Patch],
    progress: &dyn Progress,
) -> (Vec<EntityStatus>, Vec<String>) {
    let _phase = progress::phase(progress, "comparing entities", Some(entities.len() as u64));
    let results = parallel::map_progress(entities, progress, |entity| {
        progress.message(&entity.info.name);
        let parts = super::server_owned::parts(rules, &entity.info.collection, &entity.info.name);
        let working = std::fs::read(&entity.path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                super::patch::apply(&bytes, &entity.info.collection, &entity.info.name, patches)
            })
            .and_then(|bytes| {
                normalise::hash_with_parts(&bytes, &parts).map_err(|error| error.to_string())
            })
            .map_err(|error| format!("{}: {error}", entity.path.display()))?;
        let server = match EntityKey::address(&entity.info.collection, &entity.info.name)
            .and_then(|key| remote.fetch(&key))
        {
            Ok(Some(bytes)) => Some(
                normalise::hash_with_parts(&bytes, &parts)
                    .map_err(|error| format!("server export for {}: {error}", entity.info.name))?,
            ),
            Ok(None) => None,
            Err(error) => return Err(format!("{}: {error}", entity.info.name)),
        };
        let ancestor = baseline.get_for(&entity.info.collection, &entity.info.name, &working);
        Ok(EntityStatus {
            server_owned: super::server_owned::labels(&parts),
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

/// A stand-in baseline from an export document: each entity it holds is the ancestor of both
/// sides, hashed under today's normalisation and the solution's server-owned parts. For a
/// workspace whose baseline is missing or from an older hash version, so status can still say
/// which side moved since that export. Nothing is written.
pub(crate) fn ancestor_from_export(
    bytes: &[u8],
    rules: &[super::server_owned::Rule],
) -> Result<(Baseline, usize), String> {
    use super::scan::{self, Kind};
    let tokens = scan::tokenize(bytes).map_err(|error| format!("not XML: {error}"))?;
    let mut baseline = Baseline::default();
    let mut found = 0;
    let mut depth = 0usize;
    let mut collection = String::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            Kind::Start | Kind::Empty => {
                // ThingWorx writes exports without namespace prefixes; a prefixed one would hash
                // and name its entities unlike the workspace and the server, so it is refused.
                if depth <= 2 && token.name.of(bytes).contains(&b':') {
                    return Err(format!(
                        "<{}> has a namespace prefix: export it from ThingWorx as it is",
                        String::from_utf8_lossy(token.name.of(bytes))
                    ));
                }
                if depth == 1 {
                    collection = String::from_utf8_lossy(token.name.of(bytes)).into_owned();
                } else if depth == 2 {
                    let name = scan::attribute(bytes, token, "name")
                        .map_err(|error| format!("not XML: {error}"))?
                        .map(|span| {
                            scan::decode_entities(&String::from_utf8_lossy(span.of(bytes)))
                        });
                    if let Some(name) = name {
                        let end = scan::element_end_in(&tokens, bytes, index)
                            .ok_or_else(|| format!("{collection}/{name} is not closed"))?;
                        let element = &bytes[token.span.start..tokens[end].span.end];
                        let parts = super::server_owned::parts(rules, &collection, &name);
                        let hash = normalise::hash_with_parts(element, &parts)
                            .map_err(|error| format!("{collection}/{name}: {error}"))?;
                        baseline.set(&collection, &name, hash.clone(), hash);
                        found += 1;
                    }
                }
                if token.kind == Kind::Start {
                    depth += 1;
                }
            }
            Kind::End => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    if found == 0 {
        return Err(
            "it holds no entities: an export is <Entities><Things><Thing name=...>".to_string(),
        );
    }
    Ok((baseline, found))
}

/// Record a baseline for every entity whose working copy and server agree, and never for one
/// that differs. Returns how many were recorded.
pub(crate) fn record_matching(baseline: &mut Baseline, statuses: &mut [EntityStatus]) -> usize {
    let mut recorded = 0;
    for status in statuses {
        if status.server.as_deref() == Some(status.working.as_str()) {
            baseline.set(
                &status.collection,
                &status.name,
                status.working.clone(),
                status.working.clone(),
            );
            status.local_baseline = Some(status.working.clone());
            status.server_baseline = Some(status.working.clone());
            status.verdict = Verdict::InSync;
            recorded += 1;
        }
    }
    recorded
}

/// How many entities have each verdict, in `Verdict::ALL` order.
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
        assert_eq!(
            verdict("b", Some("b"), Some(("a", "a"))),
            Verdict::Converged
        );
        assert_eq!(verdict("a", None, Some(("a", "a"))), Verdict::NotOnServer);
        assert_eq!(verdict("a", Some("a"), None), Verdict::NoBaselineSame);
        assert_eq!(verdict("a", Some("b"), None), Verdict::NoBaselineDiffers);
        assert!(!Verdict::NoBaselineSame.is_drift());
        assert!(!Verdict::Converged.is_drift());
        assert!(Verdict::NoBaselineDiffers.is_drift());
    }

    #[test]
    fn an_export_stands_in_for_the_baseline_with_each_entity_hashed_as_a_file_would_be() {
        let thing = |description: &str| {
            format!(
                "<Entities><Things><Thing name=\"P.T\" description=\"{description}\"></Thing></Things></Entities>"
            )
        };
        let export = b"<Entities build=\"b\">\n  <Things>\n    <Thing name=\"P.T\" description=\"old\"></Thing>\n  </Things>\n  <Mashups>\n    <Mashup name=\"P.M\"/>\n  </Mashups>\n</Entities>";

        let (ancestor, count) = ancestor_from_export(export, &[]).unwrap();

        assert_eq!(count, 2);
        let entry = ancestor.get("Things", "P.T").unwrap();
        let old = normalise::hash(thing("old").as_bytes()).unwrap();
        assert_eq!(
            (entry.local.as_str(), entry.server.as_str()),
            (old.as_str(), old.as_str())
        );
        assert!(ancestor.get("Mashups", "P.M").is_some());
        assert!(ancestor_from_export(b"<Entities><Things/></Entities>", &[])
            .unwrap_err()
            .contains("holds no entities"));
    }

    #[test]
    fn record_selects_only_matching_working_and_server_hashes() {
        fn status(name: &str, working: &str, server: Option<&str>) -> EntityStatus {
            EntityStatus {
                server_owned: Vec::new(),
                collection: "Things".to_string(),
                name: name.to_string(),
                working: working.to_string(),
                server: server.map(str::to_string),
                local_baseline: None,
                server_baseline: None,
                verdict: verdict(working, server, None),
            }
        }

        let mut statuses = [
            status("Acme.Same", "v10:same", Some("v10:same")),
            status("Acme.Different", "v10:working", Some("v10:server")),
            status("Acme.Missing", "v10:working", None),
        ];
        let mut baseline = Baseline::default();
        statuses[0].verdict = Verdict::BothChanged;
        assert_eq!(record_matching(&mut baseline, &mut statuses), 1);
        assert_eq!(statuses[0].verdict, Verdict::InSync);
        assert_eq!(statuses[0].local_baseline.as_deref(), Some("v10:same"));
        assert_eq!(statuses[0].server_baseline.as_deref(), Some("v10:same"));
        assert_eq!(statuses[1].verdict, Verdict::NoBaselineDiffers);
        assert_eq!(
            baseline
                .get("Things", "Acme.Same")
                .map(|entry| entry.local.as_str()),
            Some("v10:same")
        );
        assert_eq!(
            baseline
                .get("Things", "Acme.Same")
                .map(|entry| entry.server.as_str()),
            Some("v10:same")
        );
        assert_eq!(baseline.get("Things", "Acme.Different"), None);
        assert_eq!(baseline.get("Things", "Acme.Missing"), None);
    }
}
