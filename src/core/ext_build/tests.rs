use super::*;
use std::io::Write as _;

const METADATA: &str = r#"<Entities>
    <ExtensionPackages>
        <ExtensionPackage name="Acme" packageVersion="1.0.0" vendor="v"/>
    </ExtensionPackages>
    <Widgets>
        <Widget name="gauge">
            <UIResources>
                <FileResource type="JS" file="gauge.ide.js" isDevelopment="true" isRuntime="false"/>
                <FileResource type="JS" file="gauge.runtime.js" isDevelopment="false" isRuntime="true"/>
            </UIResources>
        </Widget>
    </Widgets>
    <Things>
        <Thing name="Acme.Settings" thingTemplate="GenericThing"/>
    </Things>
</Entities>
"#;

/// A project with no build: metadata.xml, the widget's files, one entity and one jar.
fn plain() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for (path, text) in [
        ("metadata.xml", METADATA),
        ("ui/gauge/gauge.ide.js", "ide"),
        ("ui/gauge/gauge.runtime.js", "runtime"),
        ("ui/gauge/.gitkeep", ""),
        ("Entities/Things/Acme.Settings.xml", "<Entities/>"),
        ("lib/helper.jar", "jar"),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn names(zip: &[u8]) -> Vec<String> {
    let archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
    let mut names: Vec<String> = archive
        .file_names()
        .filter_map(|name| name.ok().map(std::borrow::Cow::into_owned))
        .collect();
    names.sort();
    names
}

#[test]
fn a_project_with_no_build_is_packed_as_the_import_expects() {
    let dir = plain();
    let built = build(dir.path(), "acme").unwrap();
    assert_eq!(built.build, ["packed by twaco"]);
    assert_eq!(built.zip, "acme/build/distributions/Acme.zip");
    assert_eq!(built.widgets, ["gauge"]);
    assert_eq!(built.entities, ["Thing Acme.Settings"]);
    assert!(built.problems.is_empty(), "{:?}", built.problems);
    let zip = std::fs::read(dir.path().join("build/distributions/Acme.zip")).unwrap();
    assert_eq!(
        names(&zip),
        [
            "Entities/Things/Acme.Settings.xml",
            "lib/common/helper.jar",
            "metadata.xml",
            "ui/gauge/gauge.ide.js",
            "ui/gauge/gauge.runtime.js"
        ]
    );
}

#[test]
fn java_sources_without_a_build_are_refused_rather_than_packed() {
    let dir = plain();
    let java = dir.path().join("src/main/java/acme/A.java");
    std::fs::create_dir_all(java.parent().unwrap()).unwrap();
    std::fs::write(java, "class A {}").unwrap();
    let error = build(dir.path(), "acme").unwrap_err();
    assert!(
        error.contains("Java sources but no Gradle or Ant build"),
        "{error}"
    );
}

#[test]
fn files_metadata_names_and_the_zip_lacks_are_problems() {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("metadata.xml", options).unwrap();
        zip.write_all(
            METADATA
                .replace(
                    "</Things>",
                    "</Things><JarResources><FileResource type=\"JAR\" file=\"acme.jar\"/></JarResources>",
                )
                .as_bytes(),
        )
        .unwrap();
        zip.start_file("ui/gauge/gauge.ide.js", options).unwrap();
        zip.finish().unwrap();
    }
    let contents = contents(buffer.get_ref()).unwrap();
    assert_eq!(
        contents.problems,
        [
            "widget gauge names gauge.runtime.js, which is not at ui/gauge/gauge.runtime.js",
            "JarResources names acme.jar, which is not at lib/common/acme.jar"
        ]
    );
    assert_eq!(contents.declared, ["gauge", "Acme.Settings"]);
}

#[test]
fn node_refusing_md4_is_seen_however_much_output_follows() {
    let mut output = b"start ".to_vec();
    output.extend_from_slice(OPENSSL_REFUSED.as_bytes());
    output.extend(std::iter::repeat_n(b'x', 200 * 1024));
    let (text, refused) =
        tail_of(std::io::Cursor::new(output)).text(std::time::Duration::from_secs(5));
    assert!(refused);
    assert!(
        !text.contains(OPENSSL_REFUSED),
        "only the last 64 KiB are kept"
    );
    assert_eq!(text.len(), 64 * 1024);
}

#[test]
fn a_credential_in_build_output_is_hidden() {
    assert_eq!(
        masked("npm ERR! //registry.example/:_authToken=abc123"),
        "npm ERR! //registry.example/:_authToken=<hidden>"
    );
    assert_eq!(masked("password: hunter2"), "password:<hidden>");
    assert_eq!(masked("BUILD FAILED"), "BUILD FAILED");
    assert_eq!(
        masked("Authorization: Bearer abc.def"),
        "Authorization:<hidden>"
    );
    assert_eq!(
        masked("fetch https://bob:s3cret@registry.example/pkg failed"),
        "fetch https://<hidden>@registry.example/pkg failed"
    );
    assert_eq!(
        masked("see https://registry.example/pkg"),
        "see https://registry.example/pkg"
    );
}

#[test]
fn a_package_name_that_is_a_path_is_not_packed() {
    let dir = plain();
    let metadata = dir.path().join("metadata.xml");
    let text = std::fs::read_to_string(&metadata).unwrap();
    std::fs::write(&metadata, text.replace("name=\"Acme\"", "name=\"../../x\"")).unwrap();
    let error = build(dir.path(), "acme").unwrap_err();
    assert!(error.contains("cannot name a zip file"), "{error}");
}

#[test]
fn a_widget_project_takes_its_zip_from_zip_before_build() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("package.json"),
        r#"{"packageName": "Acme", "version": "1.0.0", "scripts": {"build": "x"}}"#,
    )
    .unwrap();
    let zip_with = |path: &Path| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        zip.start_file("metadata.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(METADATA.as_bytes()).unwrap();
        zip.finish().unwrap();
    };
    zip_with(&root.join("zip/Acme-prod-v1.0.0.zip"));
    zip_with(&root.join("build/distributions/Acme.zip"));
    let found = package_zip(root, "acme", "Acme", "1.0.0").unwrap();
    assert!(found.ends_with("zip/Acme-prod-v1.0.0.zip"), "{found:?}");
}

#[test]
fn a_widget_project_is_identified_by_its_package_json() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{"packageName": "AcmeDial", "version": "2.0.0", "scripts": {"build": "node build.mjs"}}"#,
    )
    .unwrap();
    let identity = identity(dir.path()).unwrap();
    assert_eq!(identity.source, Source::PackageJson);
    assert_eq!(
        (identity.package.as_str(), identity.version.as_str()),
        ("AcmeDial", "2.0.0")
    );
    assert_eq!(identity.file(), "package.json");
}
