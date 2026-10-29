use super::*;
use crate::core::commands::{self, requirements as command, Mode};
use std::path::Path;

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn solution(extra: &str) -> (tempfile::TempDir, Solution) {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "twaco.toml",
        &format!("[[project]]\nname = \"Acme.App\"\n{extra}"),
    );
    let solution = Solution::load(&dir.path().join("twaco.toml")).unwrap();
    (dir, solution)
}

fn entity(root: &Path, collection: &str, tag: &str, name: &str, attrs: &str, body: &str) {
    write(
        root,
        &format!("{collection}/{name}.xml"),
        &format!(
            r#"<Entities majorVersion="10" minorVersion="1"><{collection}><{tag} name="{name}" projectName="Acme.App" {attrs}>{body}</{tag}></{collection}></Entities>"#
        ),
    );
}

fn service(script: &str) -> String {
    format!(
        r#"<ServiceImplementations><ServiceImplementation name="Run" handlerName="Script"><ConfigurationTables><ConfigurationTable name="Script"><Rows><Row><code><![CDATA[{script}]]></code></Row></Rows></ConfigurationTable></ConfigurationTables></ServiceImplementation></ServiceImplementations>"#
    )
}

#[test]
fn every_structural_source_is_collected_with_referrers_and_xml_decoding() {
    let (_dir, solution) = solution("");
    let root = &solution.root;
    let sources = [
        ("thingTemplate", "ThingTemplates"),
        ("baseThingTemplate", "ThingTemplates"),
        ("aspect.thingTemplate", "ThingTemplates"),
        ("aspect.thingShape", "ThingShapes"),
        ("baseDataShape", "DataShapes"),
        ("aspect.dataShape", "DataShapes"),
        ("dataShape", "DataShapes"),
    ];
    for (i, (attr, collection)) in sources.iter().enumerate() {
        entity(
            root,
            "Things",
            "Thing",
            &format!("Acme.Ref{i}"),
            &format!(r#"{attr}="Acme.External&amp;{i}""#),
            "",
        );
        let collected = collect(&solution).unwrap();
        assert_eq!(
            collected.references[*collection][&format!("Acme.External&{i}")],
            BTreeSet::from([format!("Things/Acme.Ref{i}")])
        );
    }
    entity(
        root,
        "Things",
        "Thing",
        "Acme.More",
        r#"valueStream="Acme.Values""#,
        r#"<ImplementedShapes><ImplementedShape name="Acme.Shape"/></ImplementedShapes><ConfigurationTables><ConfigurationTable name="Rows" dataShapeName="Acme.Row"/></ConfigurationTables><Permissions><Principal type="Group" name="Acme.Group"/><Principal type="Organization" name="Acme.Org"/><Principal type="OrganizationalUnit" name="Acme.Other:Unit:Child"/></Permissions>"#,
    );
    let collected = collect(&solution).unwrap();
    for (collection, name) in [
        ("Things", "Acme.Values"),
        ("ThingShapes", "Acme.Shape"),
        ("DataShapes", "Acme.Row"),
        ("Groups", "Acme.Group"),
        ("Organizations", "Acme.Org"),
        ("Organizations", "Acme.Other"),
    ] {
        assert_eq!(
            collected.references[collection][name],
            BTreeSet::from(["Things/Acme.More".into()])
        );
    }
}

#[test]
fn script_facts_cover_all_four_collections_without_mentions_or_platform_builtins() {
    let (_dir, solution) = solution("");
    entity(
        &solution.root,
        "Things",
        "Thing",
        "Acme.Ref",
        "",
        &service(
            r#"Things["Acme.One"].Run(); Things.AcmeTwo; ThingTemplates["Acme.Template"]; ThingTemplates.AcmeTemplate; ThingShapes["Acme.Shape"]; ThingShapes.AcmeShape; DataShapes["Acme.Row"]; DataShapes.AcmeRow; Resources["Acme.Resource"].Run(); Subsystems.AcmeSubsystem; var mention = "Acme.Mention"; Things[dynamic]; // Things.Acme.Comment
"#,
        ),
    );
    let collected = collect(&solution).unwrap();
    for (collection, names) in [
        ("Things", ["Acme.One", "AcmeTwo"]),
        ("ThingTemplates", ["Acme.Template", "AcmeTemplate"]),
        ("ThingShapes", ["Acme.Shape", "AcmeShape"]),
        ("DataShapes", ["Acme.Row", "AcmeRow"]),
    ] {
        assert_eq!(
            collected.references[collection]
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(names)
        );
    }
    assert_eq!(collected.references.len(), 4);
}

#[test]
fn defined_names_empty_values_owners_users_and_scratch_trees_are_not_dependencies() {
    let (_dir, solution) = solution("");
    entity(
        &solution.root,
        "ThingTemplates",
        "ThingTemplate",
        "Acme.Own",
        "",
        "",
    );
    entity(
        &solution.root,
        "Things",
        "Thing",
        "Acme.Ref",
        r#"thingTemplate="Acme.Own" valueStream="""#,
        r#"<Owner name="Acme.Owner" type="Organization"/><Principal name="Acme.User" type="User"/><Field aspect.dataShape=""/>"#,
    );
    for folder in ["dist/Things", ".twaco/Things", "backups/Things"] {
        write(
            &solution.root,
            &format!("{folder}/Acme.Scratch.xml"),
            r#"<Entities><Things><Thing name="Acme.Scratch" thingTemplate="Acme.ScratchTemplate"/></Things></Entities>"#,
        );
    }
    assert!(collect(&solution).unwrap().references.is_empty());
}

#[test]
fn mashup_bindings_include_external_targets_and_only_unknown_widgets() {
    let (_dir, solution) = solution("");
    let content = json!({"UI": {"Properties":{"Type":"mashup"}, "Widgets":[{"Properties":{"Type":"ptcsbutton"}},{"Properties":{"Type":"gauge"}},{"Properties":{"Type":"shape"},"Widgets":[{"Properties":{"Type":"logoutfunction"}}]},{"Properties":{"Type":"Acme.Widget"}}]}, "Data": [{"EntityType":"DynamicThingShapes", "EntityName":"Acme.Shape", "Services":[{"Target":"Run"}]}, {"EntityType":"ThingTemplate", "EntityName":"Acme.Template"}, {"EntityName":"Acme.Thing"}, {"EntityType":"Resources", "EntityName":"Acme.Resource"}], "Note":"Acme.Mention"});
    entity(
        &solution.root,
        "Mashups",
        "Mashup",
        "Acme.View",
        "",
        &format!("<mashupContent><![CDATA[{content}]]></mashupContent>"),
    );
    let collected = collect(&solution).unwrap();
    for (collection, name) in [
        ("ThingShapes", "Acme.Shape"),
        ("ThingTemplates", "Acme.Template"),
        ("Things", "Acme.Thing"),
        ("Widgets", "Acme.Widget"),
    ] {
        assert!(
            collected.references[collection].contains_key(name),
            "{collection}/{name}"
        );
    }
    assert_eq!(collected.references["Widgets"].len(), 1);
    assert_eq!(collected.references.len(), 4);
}

#[test]
fn depends_on_missing_empty_and_escaped_json_and_highest_versions() {
    let (_dir, solution) = solution("");
    for (name, depends) in [
        ("Acme.Empty", ""),
        ("Acme.Missing", "{}"),
        ("Acme.Blank", r#"{"extensions":"","projects":""}"#),
        (
            "Acme.One",
            r#"{"extensions":"Acme.Base:10.0.261811144,Acme.Other:1.0","projects":"Acme.External:0.0.0,Acme.Empty:0.0.0"}"#,
        ),
        ("Acme.Two", r#"{"extensions":"Acme.Base:10.1.0-b47"}"#),
    ] {
        entity(
            &solution.root,
            "Projects",
            "Project",
            name,
            &format!("dependsOn=\"{}\"", scan::escape_attribute(depends).unwrap()),
            "",
        );
    }
    let collected = collect(&solution).unwrap();
    assert_eq!(collected.extensions["Acme.Base"].0, "10.1.0-b47");
    assert_eq!(collected.extensions["Acme.Base"].1.len(), 2);
    assert_eq!(collected.extensions.len(), 2);
    assert_eq!(
        collected.projects.keys().collect::<Vec<_>>(),
        [&"Acme.External"]
    );
}

#[test]
fn platform_source_precedence_and_highest_project_or_export_version() {
    let (_dir, solution) = solution("[package]\nminimum_thingworx = \"10.2\"\n");
    entity(&solution.root, "Things", "Thing", "Acme.Thing", "", "");
    assert_eq!(collect(&solution).unwrap().platform.unwrap().0, "10.2");
    entity(
        &solution.root,
        "Projects",
        "Project",
        "Acme.One",
        r#"minPlatformVersion="9.1""#,
        "",
    );
    entity(
        &solution.root,
        "Projects",
        "Project",
        "Acme.Two",
        r#"minPlatformVersion="9.2""#,
        "",
    );
    let result = collect(&solution).unwrap().platform.unwrap();
    assert_eq!(result.0, "9.2");
    assert!(result.1.contains("Acme.Two minPlatformVersion"));
    let (_dir2, exports) = self::solution("");
    entity(&exports.root, "Things", "Thing", "Acme.Thing", "", "");
    entity(&exports.root, "Things", "Thing", "Acme.Other", "", "");
    let path = exports.root.join("Things/Acme.Other.xml");
    let xml = std::fs::read_to_string(&path)
        .unwrap()
        .replace("minorVersion=\"1\"", "minorVersion=\"3\"");
    std::fs::write(path, xml).unwrap();
    let result = collect(&exports).unwrap().platform.unwrap();
    assert_eq!(result.0, "10.3");
    assert!(result.1.contains("entity exports"));
}

#[test]
fn numeric_versions_pad_zeroes_ignore_build_and_refuse_invalid_parts() {
    assert_eq!(compare("10.1.0-b47", "10.1"), Ok(Ordering::Equal));
    assert_eq!(compare("10.0.261811144", "10.1.0"), Ok(Ordering::Less));
    assert_eq!(compare("10.10", "10.2"), Ok(Ordering::Greater));
    for value in ["10.x", "", "10..1", "10.+1", "999999999999999999999999999"] {
        let error = parse(&format!("[platform]\nminimum = {value:?}"))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(FILE) && error.contains("platform.minimum"),
            "{error}"
        );
    }
    let error = parse("[[extension]]\nname = 'Acme.Base'\nminimum = '10.x'")
        .unwrap_err()
        .to_string();
    assert!(error.contains("extension.Acme.Base.minimum"));
}

#[test]
fn strict_contract_parsing_names_unknown_keys_collections_and_invalid_names() {
    assert!(parse("").is_ok());
    assert!(parse("[entities]\nThings = []").is_ok());
    for (text, key) in [
        ("[platfrom]\nminimum = '10.1'", "platfrom"),
        ("[platform]\nminimum = '10'\nminimun = '9'", "minimun"),
        ("[entities]\nThingz = ['Acme.X']", "entities.Thingz"),
        ("[[project]]\nname = '../bad'", "project.name"),
        (
            "[[extension]]\nname='Acme.Base'\nminimum='1'\nunknown=1",
            "unknown",
        ),
    ] {
        let error = parse(text).unwrap_err().to_string();
        assert!(
            error.contains(FILE) && error.contains(key) && error.contains("edit"),
            "{error}"
        );
    }
}

#[test]
fn init_is_byte_stable_plans_offline_applies_and_refuses_overwrite() {
    let (_dir, solution) = solution("");
    entity(
        &solution.root,
        "Things",
        "Thing",
        "Acme.Z",
        r#"thingTemplate="Acme.External""#,
        "",
    );
    entity(
        &solution.root,
        "Things",
        "Thing",
        "Acme.A",
        r#"thingTemplate="Acme.External""#,
        "",
    );
    let init = |mode| {
        command::execute_init(
            &solution,
            &command::InitRequest {
                mode,
                lock_label: "test requirements",
            },
            &mut commands::Notices::default(),
        )
    };
    let first = init(Mode::Plan).unwrap();
    let second = init(Mode::Plan).unwrap();
    assert_eq!(first.text.as_bytes(), second.text.as_bytes());
    assert!(first.text.contains("2 referrers: Things/Acme.A"));
    assert!(first.text.contains("# from entity exports"));
    assert!(!first.written && !solution.root.join(FILE).exists());
    let applied = init(Mode::Apply).unwrap();
    assert!(applied.written);
    assert_eq!(
        std::fs::read(solution.root.join(FILE)).unwrap(),
        first.text.as_bytes()
    );
    assert!(!applied.report.blocks());
    for mode in [Mode::Plan, Mode::Apply] {
        let error = init(mode).err().unwrap().to_string();
        assert!(error.contains(FILE) && error.contains("requirements check"));
    }
    assert!(!solution.root.join(".twaco/profiles/default.toml").exists());
}

#[test]
fn offline_reports_undeclared_lower_and_stale_and_new_draft_is_clean() {
    let (_dir, solution) = solution("");
    entity(
        &solution.root,
        "Things",
        "Thing",
        "Acme.Ref",
        r#"thingTemplate="Acme.External""#,
        "",
    );
    entity(
        &solution.root,
        "Projects",
        "Project",
        "Acme.App",
        &format!(
            "dependsOn=\"{}\"",
            scan::escape_attribute(
                r#"{"extensions":"Acme.Base:2.0,Acme.Other:1","projects":"Acme.ExternalProject:0"}"#
            )
            .unwrap()
        ),
        "",
    );
    let collected = collect(&solution).unwrap();
    let contract =
        parse("[[extension]]\nname='Acme.Base'\nminimum='1'\n[entities]\nThings=['Acme.Stale']")
            .unwrap();
    let report = offline(&contract, &collected);
    assert!(report.blocks());
    for (rule, requirement) in [
        ("undeclared", "ThingTemplates/Acme.External"),
        ("undeclared", "Projects/Acme.ExternalProject"),
        ("declared_lower", "extension/Acme.Base"),
        ("declared_lower", "extension/Acme.Other"),
        ("stale", "Things/Acme.Stale"),
        ("undeclared", "platform"),
    ] {
        assert!(
            report
                .findings
                .iter()
                .any(|item| item.rule == rule && item.requirement == requirement),
            "{rule} {requirement}"
        );
    }
    let fresh = parse(&draft(&collected)).unwrap();
    assert!(offline(&fresh, &collected).findings.is_empty());
    let mut stale_only = fresh;
    stale_only.extension.push(Extension {
        name: "Acme.Unused".into(),
        minimum: "1".into(),
    });
    stale_only.project.push(Project {
        name: "Acme.UnusedProject".into(),
    });
    let report = offline(&stale_only, &collected);
    assert_eq!(report.findings.len(), 2);
    assert!(!report.blocks());
}

#[test]
fn check_requires_contract_and_never_opens_a_profile_offline() {
    let (_dir, solution) = solution("");
    let check = || {
        command::execute_check::<Fake, _>(
            &solution,
            &command::CheckRequest { server: None },
            |_| panic!("offline check opened a profile"),
            &progress::NONE,
        )
    };
    assert!(check()
        .unwrap_err()
        .to_string()
        .contains("twaco requirements init"));
    write(&solution.root, FILE, "");
    assert!(!check().unwrap().blocks());
}

struct Fake {
    platform: Result<String, String>,
    extensions: Result<BTreeMap<String, String>, String>,
    exists: Result<bool, String>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            platform: Ok("10.1.0-b47".into()),
            extensions: Ok(BTreeMap::from([("Acme.Base".into(), "2.0".into())])),
            exists: Ok(true),
        }
    }
}

impl Remote for Fake {
    fn platform_version(&self) -> Result<String, String> {
        self.platform.clone()
    }
    fn extensions(&self) -> Result<BTreeMap<String, String>, String> {
        self.extensions.clone()
    }
    fn entity_exists(&self, collection: &str, name: &str) -> Result<bool, String> {
        assert!(matches!(
            (collection, name),
            ("Widgets", "Acme.Widget") | ("Projects", "Acme.External")
        ));
        self.exists.clone()
    }
}

fn checked(remote: Fake) -> Report {
    let contract = parse("[platform]\nminimum='10.1'\n[[extension]]\nname='Acme.Base'\nminimum='2'\n[[project]]\nname='Acme.External'\n[entities]\nWidgets=['Acme.Widget']").unwrap();
    let collected = Collected::default();
    let mut report = Report {
        entities: 1,
        collections: BTreeMap::new(),
        extensions: 1,
        projects: 1,
        platform: Some("10.1".into()),
        server_checked: false,
        findings: Vec::new(),
    };
    server_check(&contract, &collected, &remote, &mut report, &progress::NONE);
    assert!(report.server_checked);
    report
}

#[test]
fn server_all_met_passes() {
    assert!(!checked(Fake::default()).blocks());
}

#[test]
fn server_platform_too_low_includes_installed_version() {
    let report = checked(Fake {
        platform: Ok("10.0.261811144".into()),
        ..Fake::default()
    });
    assert!(report.blocks());
    assert_eq!(report.findings[0].requirement, "platform");
    assert_eq!(
        report.findings[0].installed.as_deref(),
        Some("10.0.261811144")
    );
}

#[test]
fn server_missing_extension_blocks() {
    let report = checked(Fake {
        extensions: Ok(BTreeMap::new()),
        ..Fake::default()
    });
    assert!(report.blocks());
    assert_eq!(report.findings[0].requirement, "extension/Acme.Base");
    assert!(report.findings[0].message.contains("not installed"));
}

#[test]
fn server_old_extension_reports_installed_version() {
    let report = checked(Fake {
        extensions: Ok(BTreeMap::from([("Acme.Base".into(), "1.9".into())])),
        ..Fake::default()
    });
    assert!(report.blocks());
    assert_eq!(report.findings[0].installed.as_deref(), Some("1.9"));
}

#[test]
fn server_missing_project_and_widget_block_by_name() {
    let report = checked(Fake {
        exists: Ok(false),
        ..Fake::default()
    });
    assert!(report.blocks());
    assert_eq!(report.findings.len(), 2);
    assert!(report
        .findings
        .iter()
        .any(|f| f.requirement == "Widgets/Acme.Widget"));
    assert!(report
        .findings
        .iter()
        .any(|f| f.requirement == "Projects/Acme.External"));
}

#[test]
fn every_server_read_error_fails_closed_for_each_requirement() {
    let report = checked(Fake {
        platform: Err("platform unavailable".into()),
        extensions: Err("extension list unavailable".into()),
        exists: Err("entity unavailable".into()),
    });
    assert!(report.blocks());
    assert_eq!(report.findings.len(), 4);
    assert!(report
        .findings
        .iter()
        .all(|f| f.rule == "unmet" && f.blocking));
}

#[test]
fn malformed_server_versions_are_findings_without_panics() {
    let report = checked(Fake {
        platform: Ok("unknown".into()),
        extensions: Ok(BTreeMap::from([("Acme.Base".into(), "2.x".into())])),
        ..Fake::default()
    });
    assert_eq!(report.findings.len(), 2);
    assert!(report.blocks());
    assert!(report
        .findings
        .iter()
        .all(|f| f.message.contains("non-numeric")));
}

#[test]
fn compact_json_keeps_counts_and_detail_has_all_findings_and_referrers() {
    let mut report = checked(Fake::default());
    for i in 0..12 {
        report.finding(
            "undeclared",
            format!("Things/Acme.T{i}"),
            "declare it".into(),
            BTreeSet::from(["Things/Acme.Ref".into()]),
        );
    }
    let compact = report.json(false);
    assert_eq!(compact["total_findings"], 12);
    assert_eq!(compact["findings"].as_array().unwrap().len(), 8);
    assert!(compact["findings"][0].get("referrers").is_none());
    let detail = report.json(true);
    assert_eq!(detail["findings"].as_array().unwrap().len(), 12);
    assert_eq!(detail["findings"][0]["referrers"][0], "Things/Acme.Ref");
}

#[test]
fn server_executor_loads_the_requested_profile_and_reports_progress() {
    let (_dir, solution) = solution("");
    write(
        &solution.root,
        ".twaco/profiles/acme-test.toml",
        "url='http://127.0.0.1:9/Thingworx/'\nusername='test-user'\npassword='test-password'\n",
    );
    write(&solution.root, FILE, "[platform]\nminimum='10.1'\n[[extension]]\nname='Acme.Base'\nminimum='2'\n[[project]]\nname='Acme.External'\n[entities]\nWidgets=['Acme.Widget']");
    let progress = progress::Recorder::default();
    let opened = std::cell::Cell::new(false);
    let report = command::execute_check(
        &solution,
        &command::CheckRequest {
            server: Some("acme-test".into()),
        },
        |_| {
            opened.set(true);
            Fake::default()
        },
        &progress,
    )
    .unwrap();
    assert!(opened.get());
    assert!(report.server_checked);
    assert!(!report.blocks());
    assert_eq!(
        progress.phases(),
        [("checking requirements".into(), Some(2))]
    );
    assert_eq!(progress.advanced(), 2);
    assert!(matches!(
        progress.events().last(),
        Some(progress::Event::Finish)
    ));
}

#[test]
fn unreadable_entities_and_malformed_project_declarations_refuse_partial_drafts() {
    let (_dir, solution) = solution("");
    write(
        &solution.root,
        "Things/Acme.Broken.xml",
        "<Entities><Things><Thing",
    );
    let error = collect(&solution).unwrap_err().to_string();
    assert!(error.contains("Acme.Broken.xml"));
    std::fs::remove_file(solution.root.join("Things/Acme.Broken.xml")).unwrap();
    for depends in [
        "{broken",
        r#"{"extensions":"Acme.Base"}"#,
        r#"{"extensions":"Acme.Base:1.x"}"#,
    ] {
        entity(
            &solution.root,
            "Projects",
            "Project",
            "Acme.App",
            &format!("dependsOn=\"{}\"", scan::escape_attribute(depends).unwrap()),
            "",
        );
        let error = collect(&solution).unwrap_err().to_string();
        assert!(
            error.contains("Projects/Acme.App") && error.contains("dependsOn"),
            "{error}"
        );
    }
}

#[test]
fn duplicate_contract_entries_are_refused() {
    for text in ["[entities]\nThings=['Acme.T','Acme.T']", "[[project]]\nname='Acme.P'\n[[project]]\nname='Acme.P'", "[[extension]]\nname='Acme.Base'\nminimum='1'\n[[extension]]\nname='Acme.Base'\nminimum='2'"] {
        assert!(parse(text).unwrap_err().to_string().contains("duplicate"));
    }
}

#[test]
fn optional_entities_are_declared_offline_and_missing_on_server_is_a_note() {
    let contract = parse("[optional.entities]\nWidgets=['Acme.Widget']").unwrap();
    let collected = Collected {
        references: BTreeMap::from([(
            "Widgets".into(),
            BTreeMap::from([(
                "Acme.Widget".into(),
                BTreeSet::from(["Things/Acme.Ref".into()]),
            )]),
        )]),
        ..Default::default()
    };
    assert!(!offline(&contract, &collected).blocks());
    let mut report = offline(&contract, &collected);
    server_check(
        &contract,
        &collected,
        &Fake {
            exists: Ok(false),
            ..Default::default()
        },
        &mut report,
        &progress::NONE,
    );
    assert!(!report.blocks());
    assert!(report.findings.iter().any(|finding| {
        finding.rule == "optional_missing" && finding.requirement == "Widgets/Acme.Widget"
    }));
    assert!(parse(
        "[entities]\nWidgets=['Acme.Widget']\n[optional.entities]\nWidgets=['Acme.Widget']"
    )
    .unwrap_err()
    .to_string()
    .contains("one table"));
}

#[test]
fn unparsed_script_has_no_resolved_requirements() {
    let (_dir, solution) = solution("");
    entity(
        &solution.root,
        "Things",
        "Thing",
        "Acme.Ref",
        "",
        &service(r#"Things["Acme.External"].Run(); var broken = ;"#),
    );
    assert!(collect(&solution).unwrap().references.is_empty());
}
