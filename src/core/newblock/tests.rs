use super::*;
use crate::core::normalise;
use crate::core::scan::{self, Kind};

const GOLDEN: [(&str, &str); 3] = [
    ("standard", include_str!("golden/standard.xml")),
    ("abstract", include_str!("golden/abstract.xml")),
    ("implementation", include_str!("golden/implementation.xml")),
];

struct Fixture {
    root: PathBuf,
    solution: Solution,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A solution with one project that depends on a `PTC.Base` extension, as a real one does.
fn fixture() -> Fixture {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = format!("{}-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let root = std::env::temp_dir().join(format!("twaco-newblock-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(root.join("Projects")).unwrap();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"Acme.Existing\"\nroot = \".\"\n").unwrap();
    std::fs::write(
        root.join("Projects/Acme.Existing.xml"),
        "<Entities><Projects><Project dependsOn=\"{&quot;extensions&quot;:&quot;PTC.Base:10.1.0,PTC.Other:1.0.0&quot;,&quot;projects&quot;:&quot;PTC.Base:0.0.0&quot;}\" name=\"Acme.Existing\" projectName=\"Acme.Existing\"></Project></Projects></Entities>\n",
    )
    .unwrap();
    let solution = Solution::load(&root.join("twaco.toml")).unwrap();
    Fixture { root, solution }
}

#[cfg(unix)]
fn directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn directory_link(target: &Path, link: &Path) {
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[cfg(unix)]
fn remove_directory_link(link: &Path) {
    std::fs::remove_file(link).unwrap();
}

#[cfg(windows)]
fn remove_directory_link(link: &Path) {
    std::fs::remove_dir(link).unwrap();
}

fn standard(name: &str) -> Request {
    Request {
        name: name.into(),
        kind: BlockType::Standard,
        display_name: Some("Acme Block".into()),
        description: "A block".into(),
        parent: None,
        model_logic: true,
        management_shape: true,
        root: None,
        base_extension: None,
    }
}

/// The text between two markers removed, wherever it occurs.
fn without(mut text: String, open: &str, close: &str) -> String {
    let mut from = 0;
    while let Some(found) = text[from..].find(open) {
        let start = from + found;
        let Some(end) = text[start + open.len()..].find(close) else { break };
        text.replace_range(start..start + open.len() + end + close.len(), "");
        from = start;
    }
    text
}

/// What a comparison with the server's export cannot hold exact: generated timestamps, the order
/// the server lists an organization's connections in, and the project's resolved dependencies.
fn masked(text: &str) -> String {
    let text = without(text.to_string(), "<Timestamp>", "</Timestamp>");
    let text = without(text, "<Connections>", "</Connections>");
    without(text, " dependsOn=\"", "\"")
}

/// The entity elements of an Exporter document, by name.
fn golden_entities(document: &str) -> BTreeMap<String, String> {
    let bytes = document.as_bytes();
    let tokens = scan::tokenize(bytes).unwrap();
    let mut found = BTreeMap::new();
    let mut depth = 0;
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index].kind {
            Kind::Start | Kind::Empty => {
                depth += 1;
                if depth == 3 {
                    let name = scan::attribute(bytes, &tokens[index], "name").unwrap().map(|span| String::from_utf8_lossy(span.of(bytes)).into_owned());
                    let span = scan::element_span(&tokens, index).unwrap();
                    if let Some(name) = name {
                        found.insert(name, String::from_utf8_lossy(span.of(bytes)).into_owned());
                    }
                    let end = scan::element_end(&tokens, index).unwrap();
                    index = end;
                    depth -= 1;
                    if tokens[end].kind == Kind::End {
                        depth -= 0;
                    }
                }
                if tokens[index].kind == Kind::Empty {
                    depth -= 1;
                }
            }
            Kind::End => depth -= 1,
            _ => {}
        }
        index += 1;
    }
    found
}

fn request_for(kind: &str) -> Request {
    match kind {
        "standard" => Request { name: "Acme.Block".into(), ..standard("Acme.Block") },
        "abstract" => Request { name: "Acme.Base".into(), kind: BlockType::Abstract, display_name: Some("Acme Base".into()), description: "An abstract block".into(), model_logic: false, ..standard("Acme.Base") },
        _ => Request {
            name: "Acme.Impl".into(),
            kind: BlockType::Implementation,
            display_name: Some("Acme Impl".into()),
            description: "An implementation".into(),
            parent: Some("Acme.Base".into()),
            ..standard("Acme.Impl")
        },
    }
}

#[test]
fn what_is_generated_is_what_the_framework_produced_on_a_server() {
    for (kind, document) in GOLDEN {
        let fixture = fixture();
        let planned = plan(&fixture.solution, &request_for(kind)).unwrap();
        let golden = golden_entities(document);
        assert!(golden.len() >= 8, "{kind}: the fixture holds the block's entities");
        let mut compared = 0;
        for file in &planned.files {
            let name = file.path.file_stem().unwrap().to_string_lossy().into_owned();
            let expected = golden.get(&name).unwrap_or_else(|| panic!("{kind}: the server made no {name}"));
            let (generated, exported) = (masked(&file.text), masked(expected));
            // Hash the entity elements the way a deploy compares them.
            let a = normalise::hash(generated.as_bytes()).unwrap_or_else(|error| panic!("{kind} {name}: {error}"));
            let b = normalise::hash(exported.as_bytes()).unwrap();
            assert_eq!(a, b, "{kind}: {name} is not what the server produced");
            compared += 1;
        }
        // And nothing the server made is missing, except what this does not create.
        let missing: Vec<&String> = golden.keys().filter(|name| !planned.files.iter().any(|file| file.path.file_stem().unwrap().to_string_lossy() == name.as_str())).collect();
        assert!(missing.is_empty(), "{kind}: the server also made {missing:?}");
        assert_eq!(compared, golden.len());
    }
}

#[test]
fn the_project_depends_on_what_the_solution_already_does_and_the_connections_are_right() {
    let fixture = fixture();
    let planned = plan(&fixture.solution, &standard("Acme.Block")).unwrap();
    let project = &planned.files.iter().find(|file| file.path.ends_with("Projects/Acme.Block.xml")).unwrap().text;
    // The extension is the one another project declares (not its project dependency PTC.Base:0.0.0).
    assert!(project.contains("dependsOn=\"{&quot;extensions&quot;:&quot;PTC.Base:10.1.0&quot;,&quot;projects&quot;:&quot;PTC.Base:0.0.0&quot;}\""), "{project}");
    let organization = &planned.files.iter().find(|file| file.path.ends_with("Organizations/Acme.Block.Default_OR.xml")).unwrap().text;
    for pair in [("", "Root"), ("Root", "Acme.Block.Admin_UG"), ("Root", "Acme.Block.Default_UG")] {
        let flat = organization.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains(&format!("from=\"{}\" to=\"{}\"", pair.0, pair.1)), "{pair:?} in {flat}");
    }
    // An implementation depends on its parent; a flag overrides what is found.
    let implementation = plan(&fixture.solution, &request_for("implementation")).unwrap();
    let text = &implementation.files[0].text;
    assert!(text.contains("&quot;projects&quot;:&quot;Acme.Base:1.0.0&quot;"), "{text}");
    let flagged = plan(&fixture.solution, &Request { base_extension: Some("PTC.Base:11.2.0".into()), ..standard("Acme.Block") }).unwrap();
    assert!(flagged.files[0].text.contains("PTC.Base:11.2.0"));
    // The twaco.toml entry is appended, with the parent as a dependency when it is a project here.
    assert_eq!(planned.config_addition, "\n[[project]]\nname = \"Acme.Block\"\nroot = \"Acme.Block\"\n");
}

#[test]
fn applying_writes_the_entities_and_registers_the_project_and_nothing_else_changes() {
    let fixture = fixture();
    let before_toml = std::fs::read_to_string(fixture.root.join("twaco.toml")).unwrap();
    let planned = plan(&fixture.solution, &standard("Acme.Block")).unwrap();
    assert!(!fixture.root.join("Acme.Block").exists(), "a plan writes nothing");
    let created = apply(&fixture.solution, &planned).unwrap();
    assert_eq!(created.len(), planned.files.len());
    let toml = std::fs::read_to_string(fixture.root.join("twaco.toml")).unwrap();
    assert!(toml.starts_with(&before_toml) && toml.ends_with("name = \"Acme.Block\"\nroot = \"Acme.Block\"\n"));
    // The solution reads again, and every new entity belongs to the new project.
    let reloaded = Solution::load(&fixture.root.join("twaco.toml")).unwrap();
    assert!(reloaded.project("Acme.Block").is_some());
    let discovered = workspace::discover(&reloaded);
    assert!(discovered.unreadable.is_empty(), "{:?}", discovered.unreadable);
    let mine: Vec<_> = discovered.entities.iter().filter(|entity| entity.found_under == "Acme.Block").collect();
    assert_eq!(mine.len(), planned.files.len(), "{:?}", mine.iter().map(|entity| &entity.info.name).collect::<Vec<_>>());
    assert!(mine.iter().all(|entity| entity.info.project == "Acme.Block" || entity.info.collection == "Projects"));
    // A second block with the same name is refused, and so is one that would reuse an entity.
    assert!(plan(&reloaded, &standard("Acme.Block")).is_err());
}

#[test]
fn an_abstract_block_has_no_manager_and_an_implementation_may_skip_its_management_shape() {
    let fixture = fixture();
    let names = |planned: &Plan| planned.files.iter().map(|file| file.path.file_stem().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
    let abstract_block = plan(&fixture.solution, &request_for("abstract")).unwrap();
    assert!(!names(&abstract_block).iter().any(|name| name.ends_with(".Manager")));
    assert!(abstract_block.files.iter().find(|file| file.path.ends_with("Things/Acme.Base.EntryPoint.xml")).unwrap().text.contains("Abstract"));
    let lean = plan(&fixture.solution, &Request { management_shape: false, model_logic: false, ..request_for("implementation") }).unwrap();
    assert!(!names(&lean).iter().any(|name| name.ends_with("_TS")));
    let manager_template = &lean.files.iter().find(|file| file.path.ends_with("Acme.Impl.Manager_TT.xml")).unwrap().text;
    assert!(manager_template.contains("<ImplementedShapes></ImplementedShapes>") && !manager_template.contains("Acme.Impl.Management_TS"), "{manager_template}");
    assert!(manager_template.contains("baseThingTemplate=\"Acme.Base.Manager_TT\""));
}

#[test]
fn nonsense_and_collisions_are_refused_before_anything_is_planned() {
    let fixture = fixture();
    let refused = |request: Request| plan(&fixture.solution, &request).unwrap_err().to_string();
    assert!(refused(standard("Orders")).contains("namespace"));
    assert!(refused(standard("Acme..Orders")).contains("consecutive"));
    assert!(refused(Request { kind: BlockType::Implementation, parent: None, ..standard("Acme.Orders") }).contains("--parent"));
    assert!(refused(Request { parent: Some("Acme.Base".into()), ..standard("Acme.Orders") }).contains("only for an implementation"));
    assert!(refused(Request { management_shape: false, ..standard("Acme.Orders") }).contains("only an implementation"));
    assert!(refused(Request { description: "two\nlines".into(), ..standard("Acme.Orders") }).contains("one line"));
    assert!(refused(Request { description: "x]]>y".into(), ..standard("Acme.Orders") }).contains("]]>"));
    assert!(refused(Request { root: Some("../out".into()), ..standard("Acme.Orders") }).contains("inside the solution"));
    assert!(refused(Request { base_extension: Some("Other:1".into()), ..standard("Acme.Orders") }).contains("PTC.Base:10.1.0"));
    assert!(refused(Request { name: "Acme.Existing".into(), ..standard("Acme.Existing") }).contains("already exists"));
    // No PTC.Base to depend on anywhere: say what to pass.
    std::fs::remove_file(fixture.root.join("Projects/Acme.Existing.xml")).unwrap();
    assert!(refused(standard("Acme.Orders")).contains("--base-extension"));
    // An entity of the same name elsewhere in the solution blocks it.
    std::fs::create_dir_all(fixture.root.join("Things")).unwrap();
    std::fs::write(fixture.root.join("Things/Acme.Orders.EntryPoint.xml"), "<Entities><Things><Thing name=\"Acme.Orders.EntryPoint\" projectName=\"Acme.Existing\"></Thing></Things></Entities>\n").unwrap();
    let with_extension = Request { base_extension: Some("PTC.Base:10.1.0".into()), ..standard("Acme.Orders") };
    assert!(refused(with_extension).contains("Acme.Orders.EntryPoint"));
}

#[test]
fn a_link_or_junction_cannot_redirect_a_planned_block_out_of_its_target() {
    let fixture = fixture();
    let request = Request { root: Some("blocks/Acme.Block".into()), ..standard("Acme.Block") };
    let planned = plan(&fixture.solution, &request).unwrap();
    let redirected = fixture.root.join("redirected");
    let link = fixture.root.join("blocks");
    std::fs::create_dir(&redirected).unwrap();
    directory_link(&redirected, &link);

    let error = apply(&fixture.solution, &planned).unwrap_err().to_string();
    assert!(error.contains(&link.display().to_string()), "{error}");
    assert!(error.contains("symlink or junction"), "{error}");
    assert!(!redirected.join("Acme.Block").exists());
    assert!(plan(&fixture.solution, &request).unwrap_err().to_string().contains(&link.display().to_string()));
    remove_directory_link(&link);
}

#[test]
fn project_roots_cannot_inject_toml() {
    let fixture = fixture();
    for root in ["folder\"quote", "folder\\escape", "folder\nnewline", "folder\tcontrol"] {
        let error = plan(&fixture.solution, &Request { root: Some(root.into()), ..standard("Acme.Block") }).unwrap_err().to_string();
        assert!(error.contains("project folder"), "{root:?}: {error}");
    }
    assert!(plan(&fixture.solution, &Request { root: Some("nested/block".into()), ..standard("Acme.Block") }).is_ok());
}

#[test]
fn a_normalized_project_root_cannot_be_registered_twice() {
    let fixture = fixture();
    std::fs::write(fixture.root.join("twaco.toml"), "[[project]]\nname = \"Acme.Existing\"\nroot = \"./shared/\"\n").unwrap();
    let solution = Solution::load(&fixture.root.join("twaco.toml")).unwrap();
    let requested = if cfg!(windows) { "SHARED/" } else { "shared/" };
    let error = plan(&solution, &Request { root: Some(requested.into()), ..standard("Acme.Block") }).unwrap_err().to_string();
    assert!(error.contains("Acme.Existing"), "{error}");
    assert!(error.contains("already used"), "{error}");
}

#[test]
fn unreadable_files_that_might_hide_a_collision_are_refused() {
    {
        let fixture = fixture();
        let under_target = fixture.root.join("Things/nested/broken.xml");
        std::fs::create_dir_all(under_target.parent().unwrap()).unwrap();
        std::fs::write(&under_target, [0xff]).unwrap();
        let error = plan(&fixture.solution, &Request { root: Some("Things/nested".into()), ..standard("Acme.Block") }).unwrap_err().to_string();
        assert!(error.contains("broken.xml"), "{error}");
    }
    {
        let fixture = fixture();
        let named_like_entity = fixture.root.join("Things/Acme.Block.EntryPoint.xml");
        std::fs::create_dir_all(named_like_entity.parent().unwrap()).unwrap();
        std::fs::write(&named_like_entity, [0xff]).unwrap();
        let error = plan(&fixture.solution, &Request { root: Some("fresh".into()), ..standard("Acme.Block") }).unwrap_err().to_string();
        assert!(error.contains("Acme.Block.EntryPoint.xml"), "{error}");
    }
}

#[test]
fn generated_file_names_may_not_exceed_two_hundred_characters() {
    let fixture = fixture();
    let at_limit = format!("Acme.{}", "A".repeat(177));
    assert!(plan(&fixture.solution, &standard(&at_limit)).is_ok());
    let too_long = format!("Acme.{}", "A".repeat(178));
    let error = plan(&fixture.solution, &standard(&too_long)).unwrap_err().to_string();
    assert!(error.contains("201 characters"), "{error}");
    assert!(error.contains("200"), "{error}");
    assert!(error.contains(".xml"), "{error}");
}

#[test]
fn a_failed_write_removes_what_was_created_and_restores_the_configuration() {
    let fixture = fixture();
    let planned = plan(&fixture.solution, &standard("Acme.Block")).unwrap();
    let toml = std::fs::read_to_string(fixture.root.join("twaco.toml")).unwrap();
    // A file where a folder must go: the writes stop part-way.
    std::fs::create_dir_all(fixture.root.join("Acme.Block")).unwrap();
    std::fs::write(fixture.root.join("Acme.Block/ThingTemplates"), "in the way").unwrap();
    assert!(apply(&fixture.solution, &planned).is_err());
    assert_eq!(std::fs::read_to_string(fixture.root.join("twaco.toml")).unwrap(), toml);
    let left: Vec<PathBuf> = walk(&fixture.root.join("Acme.Block"));
    assert_eq!(left, [fixture.root.join("Acme.Block/ThingTemplates")], "only the blocker is left: {left:?}");
}

#[test]
fn a_file_created_after_the_plan_is_not_replaced_and_earlier_writes_are_rolled_back() {
    let fixture = fixture();
    let planned = plan(&fixture.solution, &standard("Acme.Block")).unwrap();
    let blocker = planned.files[3].path.clone();
    std::fs::create_dir_all(blocker.parent().unwrap()).unwrap();
    std::fs::write(&blocker, "someone else's file").unwrap();

    assert!(apply(&fixture.solution, &planned).is_err());
    assert_eq!(std::fs::read_to_string(&blocker).unwrap(), "someone else's file");
    let left: Vec<PathBuf> = walk(&fixture.root.join("Acme.Block"));
    assert_eq!(left, [blocker], "only the concurrent file is left: {left:?}");
}

#[test]
fn a_twaco_toml_edit_after_the_plan_is_kept_and_created_entities_are_rolled_back() {
    let fixture = fixture();
    let planned = plan(&fixture.solution, &standard("Acme.Block")).unwrap();
    let toml = std::fs::read_to_string(fixture.root.join("twaco.toml")).unwrap();
    let edited = format!("{toml}# edited\n");
    std::fs::write(fixture.root.join("twaco.toml"), &edited).unwrap();

    let error = apply(&fixture.solution, &planned).unwrap_err().to_string();
    assert_eq!(error, "twaco.toml changed since the plan; run again");
    assert_eq!(std::fs::read_to_string(fixture.root.join("twaco.toml")).unwrap(), edited);
    assert!(!fixture.root.join("Acme.Block").exists());
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}
