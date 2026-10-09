use super::*;

const FLAT_DEFAULT: &[u8] = include_bytes!(
    "../../../tests/fixtures/localization/flat/Acme.App_LocalizationTable_Default.xml"
);
const FLAT_DE: &[u8] =
    include_bytes!("../../../tests/fixtures/localization/flat/Acme.App_LocalizationTable_DE.xml");
const BLOCK_DEFAULT: &[u8] =
    include_bytes!("../../../tests/fixtures/localization/block/Acme.Other/LocalizationTable.xml");
const BLOCK_FR: &[u8] = include_bytes!(
    "../../../tests/fixtures/localization/block/Acme.Other/LocalizationTable_fr.xml"
);
const THING: &[u8] =
    include_bytes!("../../../tests/fixtures/localization/notatable/Acme.App.Thing.xml");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/localization")
}

fn token(name: &str, value: &str) -> Token {
    Token {
        name: name.to_string(),
        value: value.to_string(),
        usage: "label".to_string(),
        context: String::new(),
    }
}

fn labelled(name: &str, value: &str) -> Token {
    Token {
        context: "Acme label.".to_string(),
        ..token(name, value)
    }
}

fn names(file: &TableFile) -> Vec<&str> {
    file.tokens.iter().map(|t| t.name.as_str()).collect()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn one(path: &str, src: &[u8]) -> TableFile {
    read(Path::new(path), src).unwrap().unwrap()
}

/// A row as the hand-curated files lay it out, without its trailing newline.
fn curated_row(name: &str, value: &str) -> String {
    let cell = |field: &str, v: &str| {
        format!(
            "                            <{field}>\n                                <![CDATA[{v}]]>\n                            </{field}>\n"
        )
    };
    format!(
        "                        <Row>\n{}{}{}{}                        </Row>",
        cell("context", "Acme label."),
        cell("name", name),
        cell("usage", "label"),
        cell("value", value)
    )
}

fn project(name: &str) -> Project {
    let solution: Solution = toml::from_str(&format!("[[project]]\nname = \"{name}\"\n")).unwrap();
    solution.projects.into_iter().next().unwrap()
}

fn file(path: &str, table: &str, tokens: Vec<Token>) -> TableFile {
    TableFile {
        path: PathBuf::from(path),
        table: table.to_string(),
        header: Header::default(),
        tokens,
    }
}

#[test]
fn every_fixture_reads_with_trimmed_values_and_its_header() {
    let default = one("d.xml", FLAT_DEFAULT);
    assert_eq!(default.table, DEFAULT_TABLE);
    assert_eq!(
        names(&default),
        [
            "Acme.App.Save",
            "Acme.App.Open",
            "Acme.App.Delete",
            "Acme.App.Close",
            "Acme.App.Cancel",
            "Acme.App.Help"
        ]
    );
    assert_eq!(default.tokens[0], labelled("Acme.App.Save", "Save"));
    assert_eq!(
        default.header.description.as_deref(),
        Some("Default localization table")
    );
    assert_eq!(default.header.language_common, None);

    let de = one("de.xml", FLAT_DE);
    assert_eq!(de.table, "de");
    assert_eq!(de.tokens[1].value, "Öffnen");

    let block = one("b.xml", BLOCK_DEFAULT);
    assert_eq!(block.header.language_common.as_deref(), Some("English"));
    assert_eq!(block.tokens[0], token("Acme.Other.Start", "Start"));
    assert_eq!(block.tokens.len(), 4, "the duplicate is read, not merged");

    let fr = one("fr.xml", BLOCK_FR);
    assert_eq!(fr.table, "fr");
    assert_eq!(fr.header.language_native.as_deref(), Some("Français"));
    assert_eq!(fr.tokens[0].value, "Démarrer");

    assert!(read(Path::new("t.xml"), THING).unwrap().is_none());
    assert!(read(Path::new("n.xml"), b"not xml at all <<")
        .unwrap()
        .is_none());
}

#[test]
fn a_broken_table_export_is_refused_and_discovery_lists_it_as_unreadable() {
    let no_name = b"<Entities><LocalizationTables><LocalizationTable><ConfigurationTables>\
<ConfigurationTable name=\"LocalizationTokens\"><Rows></Rows></ConfigurationTable>\
</ConfigurationTables></LocalizationTable></LocalizationTables></Entities>";
    let error = read(Path::new("broken.xml"), no_name).unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidData);
    assert!(error.to_string().contains("broken.xml"), "{error}");

    let two = b"<Entities><LocalizationTables><LocalizationTable name=\"a\"/>\
<LocalizationTable name=\"b\"/></LocalizationTables></Entities>";
    assert!(read(Path::new("two.xml"), two).is_err());

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("x")).unwrap();
    std::fs::write(dir.path().join("x/broken.XML"), no_name).unwrap();
    std::fs::write(dir.path().join("x/ok.xml"), FLAT_DE).unwrap();
    std::fs::write(dir.path().join("notes.txt"), b"<Entities>").unwrap();
    let found = discover(dir.path()).unwrap();
    assert_eq!(found.files.len(), 1);
    assert_eq!(found.unreadable.len(), 1);
    assert!(found.unreadable[0].0.ends_with("broken.XML"));
}

#[test]
fn discovery_reads_both_layouts_and_ignores_other_xml() {
    let found = discover(&fixtures()).unwrap();
    assert!(found.unreadable.is_empty(), "{:?}", found.unreadable);
    let tables: Vec<(&str, usize)> = found
        .files
        .iter()
        .map(|f| (f.table.as_str(), f.tokens.len()))
        .collect();
    // Sorted by path: block/ before flat/.
    assert_eq!(
        tables,
        [("Default", 4), ("fr", 2), ("de", 6), ("Default", 6)]
    );
    let missing = discover(&fixtures().join("absent")).unwrap();
    assert!(missing.files.is_empty() && missing.unreadable.is_empty());
}

#[test]
fn edits_that_change_nothing_return_every_fixture_byte_for_byte() {
    for (path, src) in [
        ("d.xml", FLAT_DEFAULT),
        ("de.xml", FLAT_DE),
        ("b.xml", BLOCK_DEFAULT),
        ("fr.xml", BLOCK_FR),
    ] {
        let table = one(path, src);
        let mut seen = BTreeSet::new();
        let mut edits: Vec<Edit> = table
            .tokens
            .iter()
            .filter(|t| table.tokens.iter().filter(|o| o.name == t.name).count() == 1)
            .filter(|t| seen.insert(t.name.clone()))
            .map(|t| Edit::Set(t.clone()))
            .collect();
        edits.push(Edit::Remove("Acme.Absent".to_string()));
        assert_eq!(edit(Path::new(path), src, &edits).unwrap(), src, "{path}");
        assert_eq!(edit(Path::new(path), src, &[]).unwrap(), src, "{path}");
    }
}

#[test]
fn a_changed_three_line_cell_keeps_its_lines() {
    let out = edit(
        Path::new("de.xml"),
        FLAT_DE,
        &[Edit::Set(labelled("Acme.App.Save", "Sichern"))],
    )
    .unwrap();
    assert_eq!(
        text(&out),
        text(FLAT_DE).replace("<![CDATA[Speichern]]>", "<![CDATA[Sichern]]>")
    );
}

#[test]
fn a_changed_cdata_with_whitespace_inside_keeps_the_whitespace_around_it() {
    let out = edit(
        Path::new("b.xml"),
        BLOCK_DEFAULT,
        &[Edit::Set(token("Acme.Other.Start", "Begin"))],
    )
    .unwrap();
    let old = "<![CDATA[\n                            Start\n                            ]]>";
    assert_eq!(text(BLOCK_DEFAULT).matches(old).count(), 1);
    assert_eq!(
        text(&out),
        text(BLOCK_DEFAULT).replace(old, "<![CDATA[Begin]]>")
    );
}

#[test]
fn empty_and_self_closing_cells_take_a_cdata() {
    let changed = Token {
        context: "c".to_string(),
        ..token("Acme.Other.Start", "Start")
    };
    let out = edit(
        Path::new("b.xml"),
        BLOCK_DEFAULT,
        &[Edit::Set(changed.clone())],
    )
    .unwrap();
    assert_eq!(
        text(&out),
        text(BLOCK_DEFAULT).replacen("<context></context>", "<context><![CDATA[c]]></context>", 1)
    );

    let closed = text(BLOCK_DEFAULT).replacen("<context></context>", "<context/>", 1);
    let out = edit(Path::new("b.xml"), closed.as_bytes(), &[Edit::Set(changed)]).unwrap();
    assert_eq!(
        text(&out),
        closed.replacen("<context/>", "<context><![CDATA[c]]></context>", 1)
    );
}

#[test]
fn a_new_row_follows_the_last_row_in_its_layout_before_a_trailing_comment() {
    let out = edit(
        Path::new("de.xml"),
        FLAT_DE,
        &[
            Edit::Set(labelled("Acme.App.Print", "Drucken")),
            Edit::Set(labelled("Acme.App.Quit", "Beenden")),
        ],
    )
    .unwrap();
    let last = curated_row("Acme.App.Help", "Hilfe");
    let expected = text(FLAT_DE).replace(
        &format!("{last}\n                        <!-- END -->"),
        &format!(
            "{last}\n{}\n{}\n                        <!-- END -->",
            curated_row("Acme.App.Print", "Drucken"),
            curated_row("Acme.App.Quit", "Beenden")
        ),
    );
    assert_ne!(expected, text(FLAT_DE), "the fixture holds the last row");
    assert_eq!(text(&out), expected);
}

#[test]
fn a_new_row_in_a_crlf_file_uses_crlf() {
    let out = edit(
        Path::new("fr.xml"),
        BLOCK_FR,
        &[Edit::Set(token("Acme.Other.Reset", "Réinitialiser"))],
    )
    .unwrap();
    assert!(
        !out.windows(2).any(|w| w[1] == b'\n' && w[0] != b'\r'),
        "a bare LF crept in"
    );
    let fr = one("fr.xml", &out);
    assert_eq!(fr.tokens[2], token("Acme.Other.Reset", "Réinitialiser"));
    assert!(out.starts_with(&BLOCK_FR[..BLOCK_FR.len() - 300]));
}

#[test]
fn rows_are_added_to_an_empty_or_self_closing_rows_in_the_rendered_layout() {
    let header = Header::default();
    let empty = render("de", &header, &[]);
    let wanted = render("de", &header, &[labelled("Acme.App.A", "a")]);
    let set = [Edit::Set(labelled("Acme.App.A", "a"))];
    assert_eq!(
        text(&edit(Path::new("e.xml"), &empty, &set).unwrap()),
        text(&wanted)
    );
    let closed = text(&empty).replace("<Rows>\n                    </Rows>", "<Rows/>");
    assert_ne!(closed, text(&empty));
    assert_eq!(
        text(&edit(Path::new("e.xml"), closed.as_bytes(), &set).unwrap()),
        text(&wanted)
    );
    let no_rows = text(&empty).replace("<Rows>\n                    </Rows>", "");
    let error = edit(Path::new("e.xml"), no_rows.as_bytes(), &set).unwrap_err();
    assert!(error.to_string().contains("Rows"), "{error}");
}

#[test]
fn a_removed_row_leaves_no_blank_line() {
    let out = edit(
        Path::new("de.xml"),
        FLAT_DE,
        &[Edit::Remove("Acme.App.Open".to_string())],
    )
    .unwrap();
    let row = format!("\n{}", curated_row("Acme.App.Open", "Öffnen"));
    assert_eq!(text(FLAT_DE).matches(&row).count(), 1);
    assert_eq!(text(&out), text(FLAT_DE).replace(&row, ""));
    // The first row of a section goes too, and its section comment stays.
    let out = edit(
        Path::new("de.xml"),
        FLAT_DE,
        &[Edit::Remove("Acme.App.Cancel".to_string())],
    )
    .unwrap();
    let row = format!("\n{}", curated_row("Acme.App.Cancel", "Abbrechen"));
    assert_eq!(text(&out), text(FLAT_DE).replace(&row, ""));
    assert!(text(&out).contains("<!-- Messages -->\n                        <Row>"));
}

#[test]
fn markup_in_a_cell_duplicates_and_repeated_edits_are_refused() {
    let commented =
        text(FLAT_DE).replace("<![CDATA[Speichern]]>", "<!-- old --><![CDATA[Speichern]]>");
    let error = edit(
        Path::new("de.xml"),
        commented.as_bytes(),
        &[Edit::Set(labelled("Acme.App.Save", "Sichern"))],
    )
    .unwrap_err();
    assert!(error.to_string().contains("Acme.App.Save"), "{error}");
    assert!(error.to_string().contains("<value>"), "{error}");
    assert_eq!(error.code(), ErrorCode::InvalidData);

    let error = edit(
        Path::new("b.xml"),
        BLOCK_DEFAULT,
        &[Edit::Set(token("Acme.Other.Stop", "Halt"))],
    )
    .unwrap_err();
    assert!(error.to_string().contains("more than once"), "{error}");
    // Another token of the same file is still editable.
    edit(
        Path::new("b.xml"),
        BLOCK_DEFAULT,
        &[Edit::Set(token("Acme.Other.Reset", "Clear"))],
    )
    .unwrap();

    let error = edit(
        Path::new("de.xml"),
        FLAT_DE,
        &[
            Edit::Set(labelled("Acme.App.Save", "a")),
            Edit::Remove("Acme.App.Save".to_string()),
        ],
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidArguments);
}

#[test]
fn a_cell_the_row_lacks_is_added_after_its_last_cell() {
    let rendered = text(&render(
        "de",
        &Header::default(),
        &[labelled("Acme.App.A", "a")],
    ));
    let usage = "                            <usage>\n                                <![CDATA[label]]>\n                            </usage>\n";
    let without = rendered.replace(usage, "");
    assert_ne!(without, rendered);
    let out = edit(
        Path::new("e.xml"),
        without.as_bytes(),
        &[Edit::Set(labelled("Acme.App.A", "a"))],
    )
    .unwrap();
    assert_eq!(
        text(&out),
        without.replace(
            "                            </value>",
            "                            </value>\n                            <usage><![CDATA[label]]></usage>"
        )
    );
}

#[test]
fn render_writes_the_documented_layout() {
    let header = Header {
        description: None,
        language_common: Some("German".to_string()),
        language_native: Some("Deutsch".to_string()),
    };
    let out = render(
        "de",
        &header,
        &[
            labelled("Acme.App.Z", "Zett"),
            labelled("Acme.App.A", "A & <b>"),
        ],
    );
    let row = |name: &str, value: &str| {
        format!(
            "                        <Row>\n                            <context>\n                                <![CDATA[Acme label.]]>\n                            </context>\n                            <name>\n                                <![CDATA[{name}]]>\n                            </name>\n                            <usage>\n                                <![CDATA[label]]>\n                            </usage>\n                            <value>\n                                <![CDATA[{value}]]>\n                            </value>\n                        </Row>\n"
        )
    };
    let field = |friendly: &str, name: &str, ordinal: &str| {
        format!(
            "                            <FieldDefinition\n                                    aspect.friendlyName=\"{friendly}\"\n                                    baseType=\"STRING\"\n                                    description=\"{friendly}\"\n                                    name=\"{name}\"\n                                    ordinal=\"{ordinal}\"></FieldDefinition>\n"
        )
    };
    let expected = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Entities>\n    <LocalizationTables>\n        <LocalizationTable\n                description=\"de localization table\"\n                aspect.isEditableExtensionObject=\"false\"\n                documentationContent=\"\"\n                homeMashup=\"\"\n                languageCommon=\"German\"\n                languageNative=\"Deutsch\"\n                name=\"de\"\n                tags=\"\">\n            <avatar></avatar>\n            <DesignTimePermissions>\n                <Create></Create>\n                <Read></Read>\n                <Update></Update>\n                <Delete></Delete>\n                <Metadata></Metadata>\n            </DesignTimePermissions>\n            <RunTimePermissions></RunTimePermissions>\n            <VisibilityPermissions>\n                <Visibility></Visibility>\n            </VisibilityPermissions>\n            <ConfigurationTableDefinitions></ConfigurationTableDefinitions>\n            <ConfigurationTables>\n                <ConfigurationTable\n                        dataShapeName=\"\"\n                        description=\"Localization tokens and usage\"\n                        isMultiRow=\"true\"\n                        name=\"LocalizationTokens\"\n                        ordinal=\"0\">\n                    <DataShape>\n                        <FieldDefinitions>\n{}{}{}{}                        </FieldDefinitions>\n                    </DataShape>\n                    <Rows>\n{}{}                    </Rows>\n                </ConfigurationTable>\n            </ConfigurationTables>\n        </LocalizationTable>\n    </LocalizationTables>\n</Entities>\n",
        field("Translation context", "context", "3"),
        field("Token name", "name", "0"),
        field("Token usage", "usage", "2"),
        field("Localized value", "value", "1"),
        row("Acme.App.A", "A & <b>"),
        row("Acme.App.Z", "Zett"),
    );
    assert_eq!(text(&out), expected);
    assert!(!expected.contains("projectName"));

    let back = one("r.xml", &out);
    assert_eq!(
        back.header,
        Header {
            description: Some("de localization table".to_string()),
            ..header
        }
    );
    assert_eq!(
        back.tokens,
        [
            labelled("Acme.App.A", "A & <b>"),
            labelled("Acme.App.Z", "Zett")
        ]
    );
    let plain = render(DEFAULT_TABLE, &Header::default(), &[]);
    assert!(text(&plain).contains("description=\"Default localization table\""));
    assert!(!text(&plain).contains("languageCommon"));
    assert!(text(&plain).contains("<Rows>\n                    </Rows>"));
}

#[test]
fn a_cdata_terminator_in_a_value_round_trips() {
    for value in ["a ]]> b", "]]>", "x]]>]]>y"] {
        let out = render(
            DEFAULT_TABLE,
            &Header::default(),
            &[token("Acme.App.A", value)],
        );
        assert_eq!(one("x.xml", &out).tokens[0].value, value);
        let changed = edit(
            Path::new("x.xml"),
            &render(
                DEFAULT_TABLE,
                &Header::default(),
                &[token("Acme.App.A", "plain")],
            ),
            &[Edit::Set(token("Acme.App.A", value))],
        )
        .unwrap();
        assert_eq!(one("x.xml", &changed).tokens[0].value, value);
    }
}

#[test]
fn target_prefers_the_file_holding_the_project_then_its_folder_then_the_only_one() {
    let root = Path::new("loc");
    let app = project("Acme.App");
    let prefixes = vec!["Acme.App.".to_string()];
    let files = vec![
        file("loc/flat_de.xml", "de", vec![token("Acme.App.A", "a")]),
        file("loc/other_de.xml", "de", vec![token("Acme.Other.A", "a")]),
        file("loc/Acme.App/LocalizationTable_fr.xml", "fr", vec![]),
        file("loc/flat_it.xml", "it", vec![token("Acme.Other.A", "a")]),
    ];
    assert_eq!(
        target(&files, root, &app, &prefixes, "de", false),
        PathBuf::from("loc/flat_de.xml")
    );
    assert_eq!(
        target(&files, root, &app, &prefixes, "fr", false),
        PathBuf::from("loc/Acme.App/LocalizationTable_fr.xml")
    );
    assert_eq!(
        target(&files, root, &app, &prefixes, "it", true),
        PathBuf::from("loc/flat_it.xml")
    );
    assert_eq!(
        target(&files, root, &app, &prefixes, "it", false),
        root.join("Acme.App").join("LocalizationTable_it.xml")
    );
    assert_eq!(
        target(&files, root, &app, &prefixes, DEFAULT_TABLE, true),
        root.join("Acme.App").join("LocalizationTable.xml")
    );
}

#[test]
fn compare_reports_all_four_states_within_the_prefixes() {
    let files = vec![
        file(
            "a/LocalizationTable.xml",
            DEFAULT_TABLE,
            vec![
                token("Acme.App.Same", "x"),
                token("Acme.App.Changed", "new"),
                token("Acme.App.Local", "l"),
                token("Typo.App.Kept", "k"),
            ],
        ),
        file(
            "a/LocalizationTable_de.xml",
            "de",
            vec![token("Acme.App.Same", "x")],
        ),
        // A second file of a table: the first by path wins for a name in both.
        file(
            "b/LocalizationTable.xml",
            DEFAULT_TABLE,
            vec![token("Acme.App.Same", "other")],
        ),
    ];
    let mut server = BTreeMap::new();
    server.insert(
        DEFAULT_TABLE.to_string(),
        vec![
            token("Acme.App.Same", "  x \n"),
            token("Acme.App.Changed", "old"),
            token("Acme.App.Server", "s"),
            token("Typo.App.Kept", "k"),
            token("Someone.Else", "e"),
        ],
    );
    server.insert("fr".to_string(), vec![token("Acme.App.Same", "x")]);
    let compared = compare(&files, &server, &["Acme.App.".to_string()]);
    let states: Vec<(&str, &str, State)> = compared
        .iter()
        .map(|c| (c.table.as_str(), c.name.as_str(), c.state))
        .collect();
    assert_eq!(
        states,
        [
            ("Default", "Acme.App.Changed", State::Differs),
            ("Default", "Acme.App.Local", State::LocalOnly),
            ("Default", "Acme.App.Same", State::Same),
            ("Default", "Acme.App.Server", State::ServerOnly),
            ("Default", "Typo.App.Kept", State::Same),
            // The server has no de table: everything in it is local only.
            ("de", "Acme.App.Same", State::LocalOnly),
        ]
    );
    assert_eq!(
        compared[2].file.as_deref(),
        Some(Path::new("a/LocalizationTable.xml"))
    );
    assert_eq!(compared[3].server.as_ref().unwrap().value, "s");
    assert_eq!(compared[0].local.as_ref().unwrap().value, "new");
}

#[test]
fn problems_name_duplicates_tokens_missing_from_default_and_untranslated_ones() {
    let files = vec![
        file(
            "p/LocalizationTable.xml",
            DEFAULT_TABLE,
            vec![
                token("Acme.App.A", "a"),
                token("Acme.App.B", "b"),
                token("Acme.App.A", "again"),
                token("Acme.Other.X", "x"),
            ],
        ),
        file(
            "p/LocalizationTable_de.xml",
            "de",
            vec![token("Acme.App.A", "a")],
        ),
        file(
            "q/LocalizationTable_de.xml",
            "de",
            vec![token("Acme.App.A", "a"), token("Acme.App.Z", "z")],
        ),
        // fr uses no prefix of this solution: nothing is untranslated there.
        file(
            "q/LocalizationTable_fr.xml",
            "fr",
            vec![token("Acme.Other.X", "x")],
        ),
    ];
    let prefixes = vec!["Acme.App.".to_string()];
    assert_eq!(
        problems(&files, &prefixes),
        [
            Problem::Duplicate {
                table: "Default".to_string(),
                name: "Acme.App.A".to_string(),
                files: vec![PathBuf::from("p/LocalizationTable.xml")],
            },
            Problem::Duplicate {
                table: "de".to_string(),
                name: "Acme.App.A".to_string(),
                files: vec![
                    PathBuf::from("p/LocalizationTable_de.xml"),
                    PathBuf::from("q/LocalizationTable_de.xml")
                ],
            },
            Problem::Untranslated {
                table: "de".to_string(),
                name: "Acme.App.B".to_string(),
            },
            Problem::NotInDefault {
                table: "de".to_string(),
                name: "Acme.App.Z".to_string(),
                file: PathBuf::from("q/LocalizationTable_de.xml"),
            },
        ]
    );
}

#[test]
fn prefixes_default_to_the_project_name_and_root_to_localization() {
    let solution: Solution = toml::from_str(
        "[[project]]\nname = \"Acme.App\"\n\n[[project]]\nname = \"Acme.Other\"\n[project.localization]\nprefixes = [\"Acme.O.\", \"Acme.Other.\"]\n",
    )
    .unwrap();
    assert_eq!(prefixes(&solution.projects[0]), ["Acme.App."]);
    assert_eq!(prefixes(&solution.projects[1]), ["Acme.O.", "Acme.Other."]);
    assert_eq!(root(&solution), solution.root.join("localization"));
    assert_eq!(file_name(DEFAULT_TABLE), "LocalizationTable.xml");
    assert_eq!(file_name("pt-BR"), "LocalizationTable_pt-BR.xml");
}
