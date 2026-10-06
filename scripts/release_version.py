"""Choose the version the release workflow publishes, and write it into the repository.

WARNING: rewrites Cargo.toml, Cargo.lock and CHANGELOG.md in place. Run it in a clean checkout.

    python3 scripts/release_version.py            # write the files, print the version
    python3 scripts/release_version.py --dry-run  # print the version, write nothing

The version in Cargo.toml is released as it is until it has a release: a tag, or a dated heading
in CHANGELOG.md. After that, each release bumps the patch number. To release a minor or major
version, set it in Cargo.toml and Cargo.lock by hand; the next release uses it.

The `## [Unreleased]` section of CHANGELOG.md becomes the section of the new version, and an
empty `## [Unreleased]` stays above it. scripts/test_release_version.py tests this file.
"""
import datetime
import pathlib
import re
import subprocess
import sys
from typing import NoReturn

ROOT = pathlib.Path(__file__).resolve().parent.parent
REPOSITORY = "https://github.com/butteredstardust/twaco"
LINK = re.compile(r"^\[([^\]]+)\]: \S+$")


class ReleaseError(Exception):
    pass


def fail(message) -> NoReturn:
    print(f"release_version.py: {message}", file=sys.stderr)
    sys.exit(1)


def package_version(cargo_toml):
    """Find the `version = "X.Y.Z"` line of the [package] table: its match, or an error."""
    table = re.search(r"^\[package\]\n(.*?)(?=^\[|\Z)", cargo_toml, re.MULTILINE | re.DOTALL)
    if not table:
        raise ReleaseError("Cargo.toml has no [package] table")
    line = re.compile(r'^version = "(\d+)\.(\d+)\.(\d+)"$', re.MULTILINE)
    found = line.search(cargo_toml, table.start(1), table.end(1))
    if not found:
        raise ReleaseError('Cargo.toml has no `version = "X.Y.Z"` line under [package]')
    return found


def link_block(changelog):
    """The link definitions at the end of CHANGELOG.md: a list of (offset, name)."""
    lines = changelog.rstrip("\n").split("\n")
    offsets = [0]
    for line in lines:
        offsets.append(offsets[-1] + len(line) + 1)
    links = []
    for index in reversed(range(len(lines))):
        found = LINK.match(lines[index])
        if not found:
            break
        links.insert(0, (offsets[index], found.group(1)))
    if not links:
        raise ReleaseError("CHANGELOG.md has no link definitions at its end")
    return links


def choose(cargo_toml, changelog, tagged):
    """The version to release. `tagged(version)` tells whether the tag v<version> exists."""
    major, minor, patch = (int(part) for part in package_version(cargo_toml).groups())

    def released(version):
        dated = re.search(rf"^## \[{re.escape(version)}\] - ", changelog, re.MULTILINE)
        return tagged(version) or bool(dated)

    version = f"{major}.{minor}.{patch}"
    if released(version):
        version = f"{major}.{minor}.{patch + 1}"
    if released(version):
        raise ReleaseError(
            f"v{version} already has a tag or a dated CHANGELOG.md heading; set a newer version in Cargo.toml"
        )
    return version


def write(cargo_toml, cargo_lock, changelog, version, today):
    """The three files with the version written in: (Cargo.toml, Cargo.lock, CHANGELOG.md)."""
    if changelog.count("\n## [Unreleased]\n") != 1:
        raise ReleaseError("CHANGELOG.md needs exactly one `## [Unreleased]` heading")
    links = link_block(changelog)
    if any(name == version for _, name in links):
        raise ReleaseError(f"CHANGELOG.md already has a [{version}] link")
    # Version links run newest first, after any other link.
    versions = [offset for offset, name in links if re.fullmatch(r"\d+\.\d+\.\d+", name)]
    start = versions[0] if versions else len(changelog.rstrip("\n")) + 1

    current = package_version(cargo_toml)
    cargo_toml = cargo_toml[: current.start()] + f'version = "{version}"' + cargo_toml[current.end() :]
    cargo_lock, count = re.subn(
        r'(\[\[package\]\]\nname = "twaco"\nversion = )"[^"]*"', rf'\g<1>"{version}"', cargo_lock
    )
    if count != 1:
        raise ReleaseError('Cargo.lock needs exactly one `name = "twaco"` package entry')
    link = f"[{version}]: {REPOSITORY}/releases/tag/v{version}\n"
    changelog = changelog[:start] + link + changelog[start:]
    changelog = changelog.replace(
        "\n## [Unreleased]\n", f"\n## [Unreleased]\n\n## [{version}] - {today}\n", 1
    )
    return cargo_toml, cargo_lock, changelog


def git_tagged(version):
    tags = subprocess.run(
        ["git", "tag", "--list", f"v{version}"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout
    return bool(tags.split())


def main():
    dry_run = sys.argv[1:] == ["--dry-run"]
    if sys.argv[1:] and not dry_run:
        fail("the only argument is --dry-run")

    files = [ROOT / name for name in ("Cargo.toml", "Cargo.lock", "CHANGELOG.md")]
    cargo_toml, cargo_lock, changelog = (file.read_text() for file in files)
    today = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
    try:
        version = choose(cargo_toml, changelog, git_tagged)
        written = write(cargo_toml, cargo_lock, changelog, version, today)
    except ReleaseError as error:
        fail(error)

    if not dry_run:
        for file, text in zip(files, written):
            file.write_text(text)
    print(version)


if __name__ == "__main__":
    main()
