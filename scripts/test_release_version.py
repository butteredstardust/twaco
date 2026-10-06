"""Tests for release_version.py. Run: python3 -m unittest discover -s scripts"""
import unittest

from release_version import ReleaseError, choose, write

CARGO_TOML = """[package]
name = "twaco"
version = "0.1.0"

[dependencies]
serde = { version = "1" }

[workspace.package]
version = "9.9.9"
"""

CARGO_LOCK = """[[package]]
name = "serde"
version = "1.0.0"

[[package]]
name = "twaco"
version = "0.1.0"
"""

CHANGELOG = """# Changelog

The [Keep a Changelog] format.

## [Unreleased]

### Added

- `update`.

## [0.1.0] - 2026-01-01

- First.

[Keep a Changelog]: https://keepachangelog.com
[0.1.0]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.0
"""

def NO_TAGS(_version):
    return False


class Choose(unittest.TestCase):
    def test_a_released_version_bumps_the_patch(self):
        self.assertEqual(choose(CARGO_TOML, CHANGELOG, NO_TAGS), "0.1.1")

    def test_an_unreleased_version_is_released_as_it_is(self):
        toml = CARGO_TOML.replace('version = "0.1.0"', 'version = "0.2.0"')
        self.assertEqual(choose(toml, CHANGELOG, NO_TAGS), "0.2.0")

    def test_a_tag_counts_as_a_release(self):
        toml = CARGO_TOML.replace('version = "0.1.0"', 'version = "0.2.0"')
        self.assertEqual(choose(toml, CHANGELOG, lambda v: v == "0.2.0"), "0.2.1")

    def test_a_released_bump_fails(self):
        with self.assertRaisesRegex(ReleaseError, "v0.1.1 already"):
            choose(CARGO_TOML, CHANGELOG, lambda v: v == "0.1.1")

    def test_only_the_package_table_counts(self):
        toml = '[workspace.package]\nversion = "9.9.9"\n\n[package]\nname = "twaco"\nversion = "0.3.0"\n'
        self.assertEqual(choose(toml, CHANGELOG, NO_TAGS), "0.3.0")

    def test_no_package_version_fails(self):
        with self.assertRaisesRegex(ReleaseError, "under \\[package\\]"):
            choose('[package]\nname = "twaco"\n\n[x]\nversion = "1.0.0"\n', CHANGELOG, NO_TAGS)


class Write(unittest.TestCase):
    def test_writes_all_three_files(self):
        toml, lock, changelog = write(CARGO_TOML, CARGO_LOCK, CHANGELOG, "0.1.1", "2026-10-06")
        self.assertIn('[package]\nname = "twaco"\nversion = "0.1.1"\n', toml)
        self.assertIn('[workspace.package]\nversion = "9.9.9"', toml)
        self.assertIn('name = "twaco"\nversion = "0.1.1"', lock)
        self.assertIn('name = "serde"\nversion = "1.0.0"', lock)
        self.assertIn("## [Unreleased]\n\n## [0.1.1] - 2026-10-06\n\n### Added\n", changelog)
        self.assertTrue(
            changelog.endswith(
                "[Keep a Changelog]: https://keepachangelog.com\n"
                "[0.1.1]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.1\n"
                "[0.1.0]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.0\n"
            )
        )

    def test_a_link_in_the_text_is_not_the_block(self):
        text = CHANGELOG.replace("- First.", "[0.0.9]: https://example.com\n\n- First.")
        changelog = write(CARGO_TOML, CARGO_LOCK, text, "0.1.1", "2026-10-06")[2]
        self.assertIn("[0.0.9]: https://example.com\n\n- First.", changelog)
        self.assertIn("keepachangelog.com\n[0.1.1]: ", changelog)

    def test_a_duplicate_link_fails(self):
        text = CHANGELOG + "[0.1.1]: https://example.com\n"
        with self.assertRaisesRegex(ReleaseError, "already has a \\[0.1.1\\] link"):
            write(CARGO_TOML, CARGO_LOCK, text, "0.1.1", "2026-10-06")

    def test_no_unreleased_heading_fails(self):
        with self.assertRaisesRegex(ReleaseError, "Unreleased"):
            write(CARGO_TOML, CARGO_LOCK, CHANGELOG.replace("[Unreleased]", "[Next]"), "0.1.1", "x")

    def test_no_link_block_fails(self):
        text = CHANGELOG.split("[Keep a Changelog]:")[0]
        with self.assertRaisesRegex(ReleaseError, "no link definitions"):
            write(CARGO_TOML, CARGO_LOCK, text, "0.1.1", "x")

    def test_no_lock_entry_fails(self):
        with self.assertRaisesRegex(ReleaseError, "Cargo.lock"):
            write(CARGO_TOML, CARGO_LOCK.replace('"twaco"', '"other"'), CHANGELOG, "0.1.1", "x")


if __name__ == "__main__":
    unittest.main()
