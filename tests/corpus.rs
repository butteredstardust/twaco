//! Round-trip properties, asserted over real ThingWorx exports.
//!
//! This is the go/no-go for the whole design: if a no-op edit is not the identity function over
//! every entity file of a real repository, twaco's byte-splice approach does not work.
//!
//! **These tests prove round-tripping, not span correctness.** Tiling is close to tautological —
//! the scanner advances by its own chosen token end, so a construct it misreads still tiles
//! perfectly. The oracle for *where* a token begins and ends is the hand-computed unit tests in
//! `src/core/scan.rs`. What the corpus adds is scale and real data: hundreds of documents that
//! must all survive a rewrite unchanged.
//!
//! The corpus is whatever real ThingWorx repositories `TWACO_CORPUS` names, separated as `PATH`
//! is. Without it (CI, a fresh clone) these skip rather than fail, because a missing corpus is
//! not a defect in twaco. Set `TWACO_REQUIRE_CORPUS=1` to make a missing corpus a failure.

use std::path::{Path, PathBuf};

use twaco::core::scan::{self, Kind};
use twaco::core::splice::{self, Edit};

/// The real repositories `TWACO_CORPUS` names.
fn corpus_roots() -> Vec<PathBuf> {
    let Ok(joined) = std::env::var("TWACO_CORPUS") else { return Vec::new() };
    std::env::split_paths(&joined).filter(|p| p.is_dir()).collect()
}

/// Every XML file under a project root, skipping build output and foreign trees.
fn xml_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                // dist and distribution-backend are generated; .git and target are not ours.
                let skip = [".git", "target", "dist", "distribution-backend", "node_modules"];
                if skip.contains(&name.as_ref()) {
                    continue;
                }
                walk(&path, out);
            } else if name.ends_with(".xml") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

fn corpus() -> Vec<PathBuf> {
    corpus_roots().iter().flat_map(|r| xml_files(r)).collect()
}

/// Whether a corpus repository keeps its scripts indented to their `<code>` element, decided
/// from its own payloads exactly as `twaco init` decides it.
fn indented(root: &Path) -> bool {
    twaco::core::init::propose(root).toml.contains("indent_cdata_payload = true")
}

/// The compatibility layout of the corpus repository a file belongs to.
fn indent_cdata_payload(path: &Path) -> bool {
    corpus_roots().iter().find(|root| path.starts_with(root)).is_some_and(|root| indented(root))
}

/// True when there is no corpus and the environment has not demanded one.
fn skip_without_corpus(files: &[PathBuf], what: &str) -> bool {
    if !files.is_empty() {
        return false;
    }
    assert!(
        std::env::var("TWACO_REQUIRE_CORPUS").is_err(),
        "TWACO_REQUIRE_CORPUS is set but no corpus was found ({what})"
    );
    eprintln!("no corpus found; skipping {what}");
    true
}

#[test]
fn rename_plan_counts_on_the_corpus() {
    let Ok(case) = std::env::var("TWACO_RENAME_CASE") else {
        eprintln!("TWACO_RENAME_CASE is not set; skipping rename planner corpus test");
        return;
    };
    let Some((name, mode_name)) = case.rsplit_once('|') else {
        panic!("TWACO_RENAME_CASE must be <name>|<entity or prefix>");
    };
    let kind = match mode_name {
        "entity" => twaco::core::rename::Kind::Entity,
        "prefix" => twaco::core::rename::Kind::Prefix,
        _ => panic!("TWACO_RENAME_CASE mode must be entity or prefix"),
    };
    for root in corpus_roots() {
        let config = root.join("twaco.toml");
        if !config.is_file() {
            continue;
        }
        let repo = root.file_name().unwrap_or(root.as_os_str()).to_string_lossy();
        let solution = match twaco::core::config::Solution::load(&config) {
            Ok(solution) => solution,
            Err(error) => {
                eprintln!("rename-plan {repo} refused: {error}");
                continue;
            }
        };
        let spec = twaco::core::rename::Spec {
            kind,
            old: name.to_string(),
            new: "Twaco.Rename.Probe".to_string(),
            scope: None,
            service: None,
        };
        match twaco::core::rename::plan(&solution, &spec) {
            Ok(plan) => {
                let counts = plan.counts();
                let show = |count: twaco::core::rename::KindCounts| {
                    format!("{}/{}/{}/{}", count.files, count.exact, count.embedded, count.review)
                };
                eprintln!(
                    "rename-plan {repo} {name} {mode_name} moves={} entity={} sidecar={} config={} outside={} skipped={}",
                    plan.moves.len(),
                    show(counts.entity),
                    show(counts.sidecar),
                    show(counts.config),
                    show(counts.outside),
                    plan.skipped.len()
                );
            }
            Err(error) => eprintln!("rename-plan {repo} refused: {error}"),
        }
    }
}

/// Read a corpus file and tokenize it, recording a failure rather than skipping past it.
fn tokens_of(path: &Path, failures: &mut Vec<String>) -> Option<(Vec<u8>, Vec<scan::Token>)> {
    let src = match std::fs::read(path) {
        Ok(s) => s,
        Err(e) => {
            failures.push(format!("{}: unreadable: {e}", path.display()));
            return None;
        }
    };
    match scan::tokenize(&src) {
        Ok(tokens) => Some((src, tokens)),
        Err(e) => {
            failures.push(format!("{}: tokenize failed: {e}", path.display()));
            None
        }
    }
}

#[test]
fn tokens_tile_every_document() {
    let files = corpus();
    if skip_without_corpus(&files, "tiling") {
        return;
    }
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for path in &files {
        let Some((src, tokens)) = tokens_of(path, &mut failures) else { continue };
        let mut cursor = 0usize;
        let mut problem = None;
        for token in &tokens {
            if token.span.start != cursor {
                problem = Some(format!("gap or overlap at byte {cursor}"));
                break;
            }
            cursor = token.span.end;
        }
        if problem.is_none() && cursor != src.len() {
            problem = Some(format!("tokens end at {cursor} of {} bytes", src.len()));
        }
        match problem {
            Some(p) => failures.push(format!("{}: {p}", path.display())),
            None => checked += 1,
        }
    }
    assert!(failures.is_empty(), "{} file(s) failed:\n{}", failures.len(), failures.join("\n"));
    assert!(checked > 0, "corpus produced no readable files");
    eprintln!("tokens tile {checked} document(s)");
}

#[test]
fn a_no_op_edit_is_the_identity_function() {
    let files = corpus();
    if skip_without_corpus(&files, "identity") {
        return;
    }
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read(path) else { continue };
        match splice::splice(&src, &[]) {
            Ok(out) if out == src => checked += 1,
            Ok(_) => failures.push(format!("{}: empty splice changed the file", path.display())),
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(failures.is_empty(), "{} file(s) failed:\n{}", failures.len(), failures.join("\n"));
    assert!(checked > 0, "corpus produced no readable files");
    eprintln!("no-op splice is the identity over {checked} document(s)");
}

#[test]
fn rename_scan_counts_on_the_corpus() {
    let roots = corpus_roots();
    if roots.is_empty() {
        assert!(
            std::env::var("TWACO_REQUIRE_CORPUS").is_err(),
            "TWACO_REQUIRE_CORPUS is set but no corpus was found (rename scan)"
        );
        eprintln!("no corpus found; skipping rename scan");
        return;
    }

    if let Ok(case) = std::env::var("TWACO_RENAME_CASE") {
        let (name, mode_name) = case
            .split_once('|')
            .expect("TWACO_RENAME_CASE must be <name>|<entity or prefix>");
        let mode = match mode_name {
            "entity" => twaco::core::refs::Mode::Entity,
            "prefix" => twaco::core::refs::Mode::Prefix,
            _ => panic!("TWACO_RENAME_CASE mode must be entity or prefix"),
        };
        for root in &roots {
            let mut files = 0usize;
            let mut exact = 0usize;
            let mut embedded = 0usize;
            let mut review = 0usize;
            let mut failures = Vec::new();
            for path in xml_files(root) {
                let Ok(src) = std::fs::read(&path) else { continue };
                if twaco::core::entity::parse(&src).is_err() {
                    continue;
                }
                files += 1;
                match twaco::core::rename_scan::scan_xml(&src, name, mode, "Twaco.Rename.Probe") {
                    Ok(pass) => {
                        for finding in pass.findings {
                            match finding.tier {
                                twaco::core::refs::Tier::Exact => exact += 1,
                                twaco::core::refs::Tier::Embedded => embedded += 1,
                                twaco::core::refs::Tier::Review => review += 1,
                            }
                        }
                    }
                    Err(error) => failures.push(format!("{}: {error}", path.display())),
                }
            }
            println!(
                "rename-scan {} {} {} files={} exact={} embedded={} review={}",
                root.file_name().unwrap_or(root.as_os_str()).to_string_lossy(),
                name,
                mode_name,
                files,
                exact,
                embedded,
                review
            );
            assert!(
                failures.is_empty(),
                "{} rename scan failure(s):\n{}",
                failures.len(),
                failures.join("\n")
            );
        }
        return;
    }

    let mut checked = 0usize;
    let mut failures = Vec::new();
    for root in &roots {
        for path in xml_files(root) {
            let Ok(src) = std::fs::read(&path) else { continue };
            let Ok(info) = twaco::core::entity::parse(&src) else { continue };
            if info.name.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(&src);
            let mut new = format!("{}-TwacoRoundTrip", info.name);
            while text.contains(&new) {
                new.push('X');
            }
            let forward = match twaco::core::rename_scan::scan_xml(
                &src,
                &info.name,
                twaco::core::refs::Mode::Entity,
                &new,
            ) {
                Ok(pass) => pass,
                Err(error) => {
                    failures.push(format!("{}: forward scan: {error}", path.display()));
                    continue;
                }
            };
            let renamed = match splice::splice(&src, &forward.edits) {
                Ok(bytes) => bytes,
                Err(error) => {
                    failures.push(format!("{}: forward splice: {error}", path.display()));
                    continue;
                }
            };
            let reverse = match twaco::core::rename_scan::scan_xml(
                &renamed,
                &new,
                twaco::core::refs::Mode::Entity,
                &info.name,
            ) {
                Ok(pass) => pass,
                Err(error) => {
                    failures.push(format!("{}: reverse scan: {error}", path.display()));
                    continue;
                }
            };
            match splice::splice(&renamed, &reverse.edits) {
                Ok(restored) if restored == src => checked += 1,
                Ok(_) => failures.push(format!("{}: rename round trip changed bytes", path.display())),
                Err(error) => failures.push(format!("{}: reverse splice: {error}", path.display())),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} rename round-trip failure(s):\n{}",
        failures.len(),
        failures.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
    assert!(checked > 0, "corpus contained no entity document with a name");
    eprintln!("rename scan round-trips {checked} entity document(s) byte for byte");
}

#[test]
fn replacing_every_cdata_payload_with_itself_is_the_identity() {
    let files = corpus();
    if skip_without_corpus(&files, "CDATA identity") {
        return;
    }
    let mut checked = 0usize;
    let mut payloads = 0usize;
    let mut failures = Vec::new();
    for path in &files {
        let Some((src, tokens)) = tokens_of(path, &mut failures) else { continue };
        let edits: Vec<Edit> = tokens
            .iter()
            .filter(|t| t.kind == Kind::Cdata)
            .map(|t| Edit::new(t.inner, t.inner.of(&src).to_vec()))
            .collect();
        if edits.is_empty() {
            continue;
        }
        payloads += edits.len();
        match splice::splice(&src, &edits) {
            Ok(out) if out == src => checked += 1,
            Ok(_) => failures.push(format!("{}: rewriting CDATA with itself changed it", path.display())),
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(failures.is_empty(), "{} file(s) failed:\n{}", failures.len(), failures.join("\n"));
    assert!(payloads > 0, "corpus contained no CDATA at all, so this proved nothing");
    eprintln!("{payloads} CDATA payload(s) across {checked} file(s) round-trip unchanged");
}

#[test]
fn every_payload_survives_the_writer() {
    // Stronger than scanning payloads for `]]>`, which can never be found because `inner` ends
    // at the first one by construction. This pushes every real payload back out through
    // render_cdata and reads it again, which is the path a sync actually takes.
    let files = corpus();
    if skip_without_corpus(&files, "CDATA writer round-trip") {
        return;
    }
    let mut payloads = 0usize;
    let mut failures = Vec::new();
    for path in &files {
        let Some((src, tokens)) = tokens_of(path, &mut failures) else { continue };
        for token in tokens.iter().filter(|t| t.kind == Kind::Cdata) {
            let payload = token.inner.of(&src);
            let rendered = scan::render_cdata(payload);
            let reparsed = match scan::tokenize(&rendered) {
                Ok(t) => t,
                Err(e) => {
                    failures.push(format!("{}: rendered CDATA will not re-scan: {e}", path.display()));
                    continue;
                }
            };
            let joined: Vec<u8> = reparsed
                .iter()
                .filter(|t| t.kind == Kind::Cdata)
                .flat_map(|t| t.inner.of(&rendered).to_vec())
                .collect();
            if joined != payload {
                failures.push(format!("{}: payload changed through the writer", path.display()));
            }
            payloads += 1;
        }
    }
    assert!(failures.is_empty(), "{} failure(s):\n{}", failures.len(), failures.join("\n"));
    assert!(payloads > 0, "no payloads exercised");
    eprintln!("{payloads} payload(s) survive render_cdata and re-scan");
}

#[test]
fn an_attribute_value_is_never_reported_outside_its_tag() {
    // A span running past the tag would corrupt the document on the next write, so this asserts
    // the invariant over every attribute the corpus actually contains.
    let files = corpus();
    if skip_without_corpus(&files, "attribute bounds") {
        return;
    }
    let mut seen = 0usize;
    let mut failures = Vec::new();
    for path in &files {
        let Some((src, tokens)) = tokens_of(path, &mut failures) else { continue };
        for tag in tokens.iter().filter(|t| matches!(t.kind, Kind::Start | Kind::Empty)) {
            for key in ["name", "projectName", "baseType", "description"] {
                match scan::attribute(&src, tag, key) {
                    Ok(Some(span)) => {
                        if span.start < tag.span.start || span.end > tag.span.end {
                            failures.push(format!("{}: {key} value escapes its tag", path.display()));
                        }
                        seen += 1;
                    }
                    Ok(None) => {}
                    Err(e) => failures.push(format!("{}: {key}: {e}", path.display())),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} failure(s):\n{}", failures.len(), failures.join("\n"));
    assert!(seen > 0, "no attributes exercised");
    eprintln!("{seen} attribute value(s) stay inside their tag");
}

#[test]
fn project_attribution_is_possible_for_entity_documents() {
    // projectName is authoritative for attributing an entity to a project. The
    // sidecar `definition.xml` fragments are not entity documents and legitimately lack one, so
    // this measures the split rather than demanding it be zero.
    let files = corpus();
    if skip_without_corpus(&files, "projectName survey") {
        return;
    }
    let mut with = 0usize;
    let mut without: Vec<String> = Vec::new();
    let mut failures = Vec::new();
    for path in &files {
        let Some((src, tokens)) = tokens_of(path, &mut failures) else { continue };
        let found = tokens
            .iter()
            .filter(|t| matches!(t.kind, Kind::Start | Kind::Empty))
            .any(|t| matches!(scan::attribute(&src, t, "projectName"), Ok(Some(_))));
        if found {
            with += 1;
        } else {
            without.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    assert!(failures.is_empty(), "{} failure(s):\n{}", failures.len(), failures.join("\n"));
    assert!(with > 0, "no document carried a projectName");
    // Every file lacking one should be a fragment, not an entity export.
    let unexpected: Vec<&String> = without.iter().filter(|n| n.as_str() != "definition.xml").collect();
    eprintln!(
        "{with} document(s) carry projectName; {} do not ({} are definition.xml fragments)",
        without.len(),
        without.len() - unexpected.len()
    );
    if !unexpected.is_empty() {
        eprintln!("  not fragments: {:?}", unexpected.iter().take(8).collect::<Vec<_>>());
    }
}

#[test]
fn every_entity_document_attributes_to_exactly_one_project() {
    // An entity document must say which project it belongs to, and the
    // sidecar fragments must be distinguishable from entity documents without looking at the
    // folder they sit in. This asserts both over the real corpus.
    let files = corpus();
    if skip_without_corpus(&files, "entity attribution") {
        return;
    }
    let mut entities = 0usize;
    let mut fragments = 0usize;
    let mut undeclared: Vec<String> = Vec::new();
    let mut projects: std::collections::BTreeMap<String, usize> = Default::default();
    let mut failures = Vec::new();

    for path in &files {
        let Ok(src) = std::fs::read(path) else { continue };
        match twaco::core::entity::parse(&src) {
            Ok(info) => {
                entities += 1;
                if info.project.is_empty() {
                    undeclared.push(path.file_name().unwrap().to_string_lossy().into_owned());
                } else {
                    *projects.entry(info.project.clone()).or_default() += 1;
                }
                assert!(!info.collection.is_empty(), "{}: no collection", path.display());
                assert!(!info.name.is_empty(), "{}: no name", path.display());
            }
            Err(twaco::core::entity::ParseFailureKind::NotAnEntity) => fragments += 1,
            Err(twaco::core::entity::ParseFailureKind::Scan(e)) => {
                failures.push(format!("{}: {e}", path.display()))
            }
        }
    }

    assert!(failures.is_empty(), "{} failure(s):\n{}", failures.len(), failures.join("\n"));
    assert!(entities > 0, "no entity documents recognised");
    eprintln!("{entities} entity document(s), {fragments} fragment(s)");
    for (project, count) in &projects {
        eprintln!("  {project}: {count}");
    }
    if !undeclared.is_empty() {
        eprintln!("  no projectName: {} ({:?})", undeclared.len(), undeclared.iter().take(5).collect::<Vec<_>>());
    }
}

#[test]
fn every_entity_document_normalises_without_error() {
    let files = corpus();
    if skip_without_corpus(&files, "entity normalisation") {
        return;
    }
    let mut entities = 0usize;
    let mut failures = Vec::new();
    for path in &files {
        let src = match std::fs::read(path) {
            Ok(src) => src,
            Err(error) => {
                failures.push(format!("{}: unreadable: {error}", path.display()));
                continue;
            }
        };
        let tokens = match scan::tokenize(&src) {
            Ok(tokens) => tokens,
            Err(error) => {
                failures.push(format!("{}: tokenize failed: {error}", path.display()));
                continue;
            }
        };
        let mut depth = 0usize;
        let mut wrapper = false;
        let mut collections = 0usize;
        let mut entity_children = 0usize;
        for token in &tokens {
            match token.kind {
                Kind::Start => {
                    if depth == 0 {
                        wrapper = token.name.of(&src) == b"Entities";
                    } else if wrapper && depth == 1 {
                        collections += 1;
                    } else if wrapper && depth == 2 {
                        entity_children += 1;
                    }
                    depth += 1;
                }
                Kind::Empty => {
                    if wrapper && depth == 1 {
                        collections += 1;
                    } else if wrapper && depth == 2 {
                        entity_children += 1;
                    }
                }
                Kind::End => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if wrapper && collections == 1 && entity_children == 1 {
            entities += 1;
            if let Err(error) = twaco::core::normalise::normalise(&src) {
                failures.push(format!("{}: normalisation failed: {error}", path.display()));
            }
        }
    }
    assert!(failures.is_empty(), "{} failure(s):\n{}", failures.len(), failures.join("\n"));
    assert!(entities > 0, "no entity documents recognised");
    eprintln!("{entities} entity document(s) normalised without error");
}

/// Entity documents that have committed service sidecars, paired with the sidecar directory.
fn service_entities() -> Vec<(PathBuf, PathBuf, bool)> {
    let mut pairs = Vec::new();
    for root in corpus_roots() {
        let src_root = root.join("src");
        if !src_root.is_dir() {
            continue;
        }
        for path in xml_files(&root) {
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Ok(info) = twaco::core::entity::parse(&bytes) else { continue };
            let services = src_root.join(&info.name).join("services");
            if services.is_dir() {
                let indent = indent_cdata_payload(&path);
                pairs.push((path, services, indent));
            }
        }
    }
    pairs
}

#[test]
fn extraction_reproduces_the_committed_sidecars_byte_for_byte() {
    // Committed sidecars form the compatibility corpus; extraction must reproduce them exactly.
    let pairs = service_entities();
    if pairs.is_empty() {
        assert!(
            std::env::var("TWACO_REQUIRE_CORPUS").is_err(),
            "TWACO_REQUIRE_CORPUS is set but no service sidecars were found"
        );
        eprintln!("no service sidecars found; skipping");
        return;
    }

    let mut services = 0usize;
    let mut entities = 0usize;
    let mut stale: Vec<String> = Vec::new();
    let mut failures = Vec::new();
    for (entity_path, services_dir, _) in &pairs {
        let src = std::fs::read(entity_path).expect("entity is readable");
        let extracted = match twaco::core::sidecar::extract_services(&src) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: {e}", entity_path.display()));
                continue;
            }
        };
        entities += 1;
        for service in &extracted {
            let dir = services_dir.join(&service.name);
            let want_def = dir.join("definition.xml");
            let want_js = dir.join("script.js");
            if !want_def.is_file() || !want_js.is_file() {
                failures.push(format!("{}: no committed sidecar for {}", entity_path.display(), service.name));
                continue;
            }
            services += 1;
            let committed_def = std::fs::read_to_string(&want_def).unwrap().replace("\r\n", "\n");
            let committed_js = std::fs::read_to_string(&want_js).unwrap().replace("\r\n", "\n");
            if service.definition != committed_def {
                // Some committed definition sidecars end without their trailing newline. The
                // canonical extracted form includes one, so the file on disk is stale. Counted
                // and reported so it cannot quietly become a licence to differ.
                if service.definition == format!("{committed_def}
") {
                    stale.push(want_def.display().to_string());
                } else {
                    failures.push(format!("{}: definition differs", want_def.display()));
                }
            }
            // The committed script.js has no trailing newline; the extractor produces none.
            if service.script != committed_js {
                failures.push(format!(
                    "{}: script differs ({} vs {} bytes)",
                    want_js.display(),
                    service.script.len(),
                    committed_js.len()
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatch(es) out of {services} service(s):\n{}",
        failures.len(),
        failures.iter().take(12).cloned().collect::<Vec<_>>().join("\n")
    );
    assert!(services > 0, "no services compared");
    eprintln!("{services} service sidecar(s) across {entities} entity file(s) reproduced exactly");
    if !stale.is_empty() {
        eprintln!(
            "  {} committed file(s) are missing their trailing newline and are stale on disk:",
            stale.len()
        );
        for path in &stale {
            eprintln!("    {path}");
        }
    }
    // Pinned by name, not by count: any *other* file differing is twaco drifting, not a wart.
    const KNOWN_STALE: [&str; 2] = [
        "CleanAndReseedTestData",
        "GetDefaultConfigurationRows",
    ];
    for path in &stale {
        assert!(
            KNOWN_STALE.iter().any(|known| path.contains(known)),
            "{path} is missing its trailing newline and is not one of the two known stale files"
        );
    }
}

#[test]
fn extraction_handles_every_service_bearing_entity_in_the_corpus() {
    // Not only the ones that already have sidecars. a database Thing carries a
    // SQLCommand service and no sidecar tree, so the comparison test never reached it.
    let files = corpus();
    if skip_without_corpus(&files, "whole-corpus extraction") {
        return;
    }
    let mut entities = 0usize;
    let mut services = 0usize;
    let mut failures = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read(path) else { continue };
        if twaco::core::entity::parse(&src).is_err() {
            continue;
        }
        // Only entities that declare services at all.
        if !src.windows(19).any(|w| w == b"<ServiceDefinitions") {
            continue;
        }
        entities += 1;
        match twaco::core::sidecar::extract_services(&src) {
            Ok(list) => services += list.len(),
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} entity file(s) failed to extract:\n{}",
        failures.len(),
        failures.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
    eprintln!("{services} service(s) extracted from {entities} service-bearing entity file(s)");
}

/// Read the committed sidecars for one entity from disk.
///
/// Deliberately not re-extracted from the entity: that would only prove sync is the inverse of
/// extract, when what matters is that the files a person edits write back cleanly.
fn committed_sidecars(dir: &Path) -> std::collections::BTreeMap<String, twaco::core::sidecar::ServiceSidecar> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return out };
    for entry in entries.flatten() {
        let service = entry.path();
        if !service.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let (Ok(definition), Ok(script)) = (
            std::fs::read_to_string(service.join("definition.xml")),
            std::fs::read_to_string(service.join("script.js")),
        ) else {
            continue;
        };
        out.insert(
            name.clone(),
            twaco::core::sidecar::ServiceSidecar {
                name,
                definition: definition.replace("\r\n", "\n"),
                script: script.replace("\r\n", "\n"),
            },
        );
    }
    out
}

#[test]
fn syncing_the_committed_sidecars_back_changes_nothing() {
    // `sync --all --check` must report no change.
    // The sidecars come off disk, so this exercises the real round trip rather than proving
    // sync and extract agree with each other.
    let pairs = service_entities();
    if pairs.is_empty() {
        assert!(
            std::env::var("TWACO_REQUIRE_CORPUS").is_err(),
            "TWACO_REQUIRE_CORPUS is set but no managed entities were found"
        );
        eprintln!("no managed entities found; skipping");
        return;
    }
    let mut entities = 0usize;
    let mut services = 0usize;
    let mut failures = Vec::new();
    for (path, services_dir, indent_cdata_payload) in &pairs {
        let src = std::fs::read(path).expect("entity is readable");
        let sidecars = committed_sidecars(services_dir);
        if sidecars.is_empty() {
            failures.push(format!("{}: no sidecars read", services_dir.display()));
            continue;
        }
        // allow_structural_change: an entity can hold services with no sidecar, such as the two
        // committed ones that are stale, and this test is not about those.
        match twaco::core::sync::sync(&src, &sidecars, true, *indent_cdata_payload, false) {
            Ok((out, report)) => {
                entities += 1;
                services += report.unchanged.len() + report.changed.len();
                if out != src {
                    let at = out.iter().zip(src.iter()).position(|(a, b)| a != b).unwrap_or(0);
                    failures.push(format!(
                        "{}: writing the committed sidecars back changed byte {at} (changed: {:?})",
                        path.display(),
                        report.changed
                    ));
                }
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} failure(s):\n{}",
        failures.len(),
        failures.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
    );
    assert!(services > 0, "no services synced");
    eprintln!("{services} committed sidecar(s) across {entities} entity file(s) write back unchanged");
}

#[test]
fn syncing_extracted_sidecars_is_the_identity_in_each_corpus_mode() {
    // Layout is only applied to an edited script. With no content edit, both corpora must be
    // byte-identical under either setting, including CR-CR-LF payloads.
    let files = corpus();
    if skip_without_corpus(&files, "sync stability") {
        return;
    }
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read(path) else { continue };
        if twaco::core::entity::parse(&src).is_err() {
            continue;
        }
        let extraction = match twaco::core::sidecar::extract(&src) {
            Ok(e) => e,
            Err(e) => {
                failures.push(format!("{}: extract: {e}", path.display()));
                continue;
            }
        };
        if extraction.services.is_empty() {
            continue;
        }
        let sidecars: std::collections::BTreeMap<String, _> =
            extraction.services.iter().map(|s| (s.name.clone(), s.clone())).collect();
        for indent_cdata_payload in [false, true] {
            match twaco::core::sync::sync(&src, &sidecars, true, indent_cdata_payload, false) {
                Ok((once, first)) if once == src && first.changed.is_empty() => {
                    match twaco::core::sync::sync(
                        &once,
                        &sidecars,
                        true,
                        indent_cdata_payload,
                        false,
                    ) {
                        Ok((twice, second)) if twice == once && second.changed.is_empty() => {
                            checked += 1;
                        }
                        Ok((_, second)) => failures.push(format!(
                            "{}: second sync changed it in mode indent_cdata_payload={indent_cdata_payload} ({:?})",
                            path.display(),
                            second.changed
                        )),
                        Err(e) => failures.push(format!("{}: second sync: {e}", path.display())),
                    }
                }
                Ok((_, first)) => {
                    failures.push(format!(
                        "{}: unchanged content changed in mode indent_cdata_payload={indent_cdata_payload} ({:?})",
                        path.display(),
                        first.changed
                    ));
                }
                Err(e) => failures.push(format!("{}: first sync: {e}", path.display())),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} file(s) never settle:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(checked > 0, "no entities exercised");
    eprintln!("{checked} service-bearing entity/mode pairs round-trip byte for byte");
}

#[test]
fn an_indented_corpus_relayout_to_flush_rewrites_once_and_settles() {
    let Some(root) = corpus_roots().into_iter().find(|root| indented(root)) else {
        assert!(
            std::env::var("TWACO_REQUIRE_CORPUS").is_err(),
            "TWACO_REQUIRE_CORPUS is set but no corpus repository keeps indented scripts"
        );
        eprintln!("no indented corpus repository; skipping relayout corpus test");
        return;
    };

    let mut entities = 0usize;
    let mut services = 0usize;
    let mut rewritten = 0usize;
    let mut failures = Vec::new();
    for path in xml_files(&root) {
        let Ok(src) = std::fs::read(&path) else { continue };
        if twaco::core::entity::parse(&src).is_err() {
            continue;
        }
        let extraction = match twaco::core::sidecar::extract(&src) {
            Ok(extraction) if !extraction.services.is_empty() => extraction,
            Ok(_) => continue,
            Err(e) => {
                failures.push(format!("{}: extract: {e}", path.display()));
                continue;
            }
        };
        let sidecars: std::collections::BTreeMap<String, _> = extraction
            .services
            .iter()
            .map(|service| (service.name.clone(), service.clone()))
            .collect();
        let (once, first) = match twaco::core::sync::sync(&src, &sidecars, true, false, true) {
            Ok(result) => result,
            Err(e) => {
                failures.push(format!("{}: flush relayout: {e}", path.display()));
                continue;
            }
        };
        entities += 1;
        services += sidecars.len();
        rewritten += first.changed.len();

        let again = match twaco::core::sidecar::extract(&once) {
            Ok(extraction) => extraction,
            Err(e) => {
                failures.push(format!("{}: relayout output will not extract: {e}", path.display()));
                continue;
            }
        };
        let residecars: std::collections::BTreeMap<String, _> = again
            .services
            .iter()
            .map(|service| (service.name.clone(), service.clone()))
            .collect();
        match twaco::core::sync::sync(&once, &residecars, true, false, true) {
            Ok((twice, second)) if twice == once && second.changed.is_empty() => {}
            Ok((_, second)) => failures.push(format!(
                "{}: second flush relayout still changes {:?}",
                path.display(),
                second.changed
            )),
            Err(e) => failures.push(format!("{}: second flush relayout: {e}", path.display())),
        }
    }

    assert!(failures.is_empty(), "{} relayout failure(s):\n{}", failures.len(), failures.join("\n"));
    assert!(rewritten > 0, "the indented corpus had no indented payloads to migrate");
    eprintln!(
        "flush relayout rewrites {rewritten} of {services} payload(s) across {entities} entity file(s); the second pass changes none"
    );
}

#[test]
fn a_script_carrying_the_cdata_terminator_survives_a_real_entity() {
    // The corpus has no such payload, so this plants one in a real entity rather than a fixture.
    let pairs = service_entities();
    if pairs.is_empty() {
        return;
    }
    let (path, dir, indent_cdata_payload) = &pairs[0];
    let src = std::fs::read(path).expect("entity is readable");
    let mut sidecars = committed_sidecars(dir);
    let victim = sidecars.keys().next().expect("at least one sidecar").clone();
    sidecars.get_mut(&victim).unwrap().script = "var marker = \"]]>\";".to_string();

    let (out, _) = twaco::core::sync::sync(&src, &sidecars, true, *indent_cdata_payload, false)
        .expect("sync succeeds");
    let back = twaco::core::sidecar::extract(&out).expect("output re-extracts");
    let written = back.services.iter().find(|s| s.name == victim).expect("service survives");
    assert_eq!(written.script, "var marker = \"]]>\";", "the terminator must survive a round trip");
}

/// DataShape entity files paired with their committed `fields.json`.
fn datashape_pairs() -> Vec<(PathBuf, PathBuf)> {
    let mut pairs = Vec::new();
    for root in corpus_roots() {
        let src_root = root.join("src");
        if !src_root.is_dir() {
            continue;
        }
        for path in xml_files(&root) {
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Ok(info) = twaco::core::entity::parse(&bytes) else { continue };
            if info.collection != "DataShapes" {
                continue;
            }
            let sidecar = src_root.join(&info.name).join("fields.json");
            if sidecar.is_file() {
                pairs.push((path, sidecar));
            }
        }
    }
    pairs
}

#[test]
fn datashape_fields_reproduce_the_committed_sidecars_byte_for_byte() {
    let pairs = datashape_pairs();
    if pairs.is_empty() {
        assert!(
            std::env::var("TWACO_REQUIRE_CORPUS").is_err(),
            "TWACO_REQUIRE_CORPUS is set but no fields.json was found"
        );
        eprintln!("no DataShape sidecars found; skipping");
        return;
    }
    let mut checked = 0usize;
    let mut fields = 0usize;
    let mut failures = Vec::new();
    for (entity, sidecar) in &pairs {
        let src = std::fs::read(entity).expect("entity is readable");
        match twaco::core::datashape::extract(&src) {
            Ok(extracted) => {
                fields += extracted.len();
                let produced = twaco::core::datashape::to_sidecar(&extracted);
                let committed =
                    std::fs::read_to_string(sidecar).unwrap().replace("\r\n", "\n");
                if produced != committed {
                    failures.push(format!("{}: differs", sidecar.display()));
                } else {
                    checked += 1;
                }
            }
            Err(e) => failures.push(format!("{}: {e}", entity.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        pairs.len(),
        failures.iter().take(8).cloned().collect::<Vec<_>>().join("\n")
    );
    eprintln!("{checked} fields.json reproduced exactly, {fields} field(s) total");
}

#[test]
fn syncing_the_committed_datashape_fields_back_changes_nothing() {
    let pairs = datashape_pairs();
    if pairs.is_empty() {
        return;
    }
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for (entity, sidecar) in &pairs {
        let src = std::fs::read(entity).expect("entity is readable");
        let text = std::fs::read_to_string(sidecar).unwrap().replace("\r\n", "\n");
        let desired = match twaco::core::datashape::from_sidecar(&text) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{}: {e}", sidecar.display()));
                continue;
            }
        };
        match twaco::core::datashape::sync(&src, &desired, false) {
            Ok((out, changes)) => {
                if out != src {
                    let at = out.iter().zip(src.iter()).position(|(a, b)| a != b).unwrap_or(0);
                    failures.push(format!("{}: changed at byte {at} ({changes:?})", entity.display()));
                } else {
                    checked += 1;
                }
            }
            Err(e) => failures.push(format!("{}: {e}", entity.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} failure(s):\n{}",
        failures.len(),
        failures.iter().take(8).cloned().collect::<Vec<_>>().join("\n")
    );
    eprintln!("{checked} DataShape(s) survive a no-op field sync unchanged");
}

/// Mashup entity files paired with their committed sidecar directory.
fn mashup_pairs() -> Vec<(PathBuf, PathBuf)> {
    let mut pairs = Vec::new();
    for root in corpus_roots() {
        let src_root = root.join("src");
        if !src_root.is_dir() {
            continue;
        }
        for path in xml_files(&root) {
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Ok(info) = twaco::core::entity::parse(&bytes) else { continue };
            if info.collection != "Mashups" {
                continue;
            }
            let dir = src_root.join(&info.name).join("mashup");
            if dir.join("content.json").is_file() {
                pairs.push((path, dir));
            }
        }
    }
    pairs
}

#[test]
fn mashup_assets_reproduce_the_committed_sidecars_byte_for_byte() {
    let pairs = mashup_pairs();
    if pairs.is_empty() {
        assert!(
            std::env::var("TWACO_REQUIRE_CORPUS").is_err(),
            "TWACO_REQUIRE_CORPUS is set but no mashup sidecars were found"
        );
        eprintln!("no mashup sidecars found; skipping");
        return;
    }
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for (entity, dir) in &pairs {
        let src = std::fs::read(entity).expect("entity is readable");
        match twaco::core::mashup::extract(&src) {
            Ok(assets) => {
                let content = std::fs::read_to_string(dir.join("content.json"))
                    .unwrap()
                    .replace("\r\n", "\n");
                let css = std::fs::read_to_string(dir.join("custom.css"))
                    .unwrap_or_default()
                    .replace("\r\n", "\n");
                if assets.content != content {
                    let at = assets
                        .content
                        .bytes()
                        .zip(content.bytes())
                        .position(|(a, b)| a != b)
                        .unwrap_or(content.len().min(assets.content.len()));
                    failures.push(format!(
                        "{}: content.json differs at byte {at}: {:?} vs {:?}",
                        dir.display(),
                        &assets.content[at.saturating_sub(40)..(at + 40).min(assets.content.len())],
                        &content[at.saturating_sub(40)..(at + 40).min(content.len())]
                    ));
                } else if assets.css != css {
                    failures.push(format!("{}: custom.css differs", dir.display()));
                } else {
                    checked += 1;
                }
            }
            Err(e) => failures.push(format!("{}: {e}", entity.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        pairs.len(),
        failures.iter().take(3).cloned().collect::<Vec<_>>().join("\n")
    );
    eprintln!("{checked} mashup sidecar pair(s) reproduced exactly");
}

#[test]
fn syncing_the_committed_mashup_assets_back_changes_nothing() {
    let pairs = mashup_pairs();
    if pairs.is_empty() {
        return;
    }
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for (entity, dir) in &pairs {
        let src = std::fs::read(entity).expect("entity is readable");
        let assets = twaco::core::mashup::Assets {
            content: std::fs::read_to_string(dir.join("content.json")).unwrap().replace("\r\n", "\n"),
            css: std::fs::read_to_string(dir.join("custom.css")).unwrap_or_default().replace("\r\n", "\n"),
        };
        match twaco::core::mashup::sync(&src, &assets) {
            Ok((out, changes)) => {
                if out != src {
                    failures.push(format!("{}: changed ({changes:?})", entity.display()));
                } else {
                    checked += 1;
                }
            }
            Err(e) => failures.push(format!("{}: {e}", entity.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} failure(s):\n{}",
        failures.len(),
        failures.iter().take(5).cloned().collect::<Vec<_>>().join("\n")
    );
    eprintln!("{checked} mashup(s) survive a no-op sync unchanged");
}

/// Every DataTable Thing in the corpus.
///
/// There is no committed `datatable.json` anywhere, so unlike the other sidecar kinds this has
/// no byte-for-byte oracle. What it can assert is the property that matters: a configuration
/// that goes out to a sidecar and straight back in leaves the document untouched.
fn data_table_files() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for root in corpus_roots() {
        for path in xml_files(&root) {
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if twaco::core::datatable::is_data_table(&bytes) {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn a_data_table_round_trips_through_its_sidecar_unchanged() {
    let files = data_table_files();
    if files.is_empty() {
        assert!(
            std::env::var("TWACO_REQUIRE_CORPUS").is_err(),
            "TWACO_REQUIRE_CORPUS is set but no DataTable entities were found"
        );
        eprintln!("no DataTable entities found; skipping");
        return;
    }
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for path in &files {
        let src = std::fs::read(path).expect("entity is readable");
        let configuration = match twaco::core::datatable::extract(&src) {
            Ok(configuration) => configuration,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let text = match twaco::core::datatable::to_sidecar(&configuration) {
            Ok(text) => text,
            Err(e) => {
                failures.push(format!("{}: will not become a sidecar: {e}", path.display()));
                continue;
            }
        };
        let back = match twaco::core::datatable::from_sidecar(&text) {
            Ok(back) => back,
            Err(e) => {
                failures.push(format!("{}: sidecar will not read back: {e}", path.display()));
                continue;
            }
        };
        match twaco::core::datatable::sync(&src, &back) {
            Ok((out, changes)) => {
                if out != src {
                    failures.push(format!("{}: changed ({changes:?})", path.display()));
                } else {
                    checked += 1;
                }
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} failed:\n{}",
        failures.len(),
        files.len(),
        failures.iter().take(5).cloned().collect::<Vec<_>>().join("\n")
    );
    eprintln!("{checked} DataTable(s) round-trip through a sidecar unchanged");
}
