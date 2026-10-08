//! An independent XML parser as a test oracle.
//!
//! twaco reads and writes entity XML with its own scanner and byte splices. A test that re-reads
//! a writer's output with that same scanner shares any blind spot it has, so every `.xml` file a
//! twaco writer produces during a unit test is also parsed by roxmltree, a strict parser that
//! shares no code with twaco. Release builds never see this.

/// Panic unless `bytes`, about to be written to `path`, are a well-formed XML document. Files
/// whose name does not end in `.xml` are not XML and are not checked.
pub(crate) fn check_write(path: &std::path::Path, bytes: &[u8]) {
    let is_xml = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"));
    if is_xml {
        assert_well_formed(bytes, &path.display().to_string());
    }
}

/// Panic unless `bytes` are a well-formed XML document; `what` names it in the message.
pub(crate) fn assert_well_formed(bytes: &[u8], what: &str) {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => panic!("{what}: twaco wrote XML that is not UTF-8: {e}"),
    };
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    if let Err(e) = roxmltree::Document::parse_with_options(text, options) {
        panic!("{what}: twaco wrote XML that a strict parser refuses: {e}");
    }
}

mod tests {
    #[test]
    #[should_panic(expected = "a strict parser refuses")]
    fn a_malformed_entity_file_cannot_be_written_in_a_test() {
        let dir = tempfile::Builder::new()
            .prefix("twaco-oracle-")
            .tempdir()
            .unwrap();
        let path = dir.path().join("entity.xml");
        // An unescaped ampersand: twaco's lenient reading would take it as text.
        let _ = crate::core::workspace::atomic_replace(&path, b"<Thing name=\"A&B\"/>");
    }

    #[test]
    fn only_xml_files_are_checked() {
        let dir = tempfile::Builder::new()
            .prefix("twaco-oracle-")
            .tempdir()
            .unwrap();
        let path = dir.path().join("entity.js");
        crate::core::workspace::atomic_replace(&path, b"<not xml").unwrap();
    }
}
