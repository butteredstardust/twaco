"""Build a small app on a real ThingWorx server with twaco, check every step, and delete it.

WARNING: writes to the server the environment names. It creates, changes and deletes only
entities named ZZ.Twaco.Smoke.* and refuses to run when the server already has any. Use a
disposable test server, never a shared one.

    TWACO_URL=http://localhost:8080/Thingworx TWACO_USERNAME=... TWACO_PASSWORD=... \
        python3 scripts/live_smoke.py [--twaco path/to/twaco] [--keep]

Credentials come only from the environment, as every twaco command reads them; nothing is
written to disk but a temporary solution folder, deleted at the end. After each step the script
asserts what the server holds: the deploy read back what it sent, `entity status` says in sync,
and a service call returns what its script builds. The entities are deleted at the end even
when a step fails, unless --keep is given.
"""
import argparse
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

PREFIX = "ZZ.Twaco.Smoke"
PROJECT = PREFIX
SHAPE = f"{PREFIX}.Rows_DS"
THING = f"{PREFIX}.Store_T"
RENAMED = f"{PREFIX}.Shop_T"
MASHUP = f"{PREFIX}.Orders_MU"


class SmokeError(Exception):
    pass


class Smoke:
    def __init__(self, twaco: str, root: pathlib.Path):
        self.twaco = twaco
        self.root = root

    def run(self, *args: str, ok: bool = True) -> subprocess.CompletedProcess:
        result = subprocess.run(
            [self.twaco, *args],
            cwd=self.root,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        print(f"$ twaco {' '.join(args)}  -> exit {result.returncode}")
        if ok and result.returncode != 0:
            raise SmokeError(
                f"twaco {' '.join(args)} failed ({result.returncode})\n{result.stdout}\n{result.stderr}"
            )
        return result

    def in_sync(self) -> None:
        """Every entity of the solution is as the server has it."""
        out = self.run("entity", "status", "--all").stdout
        counts = dict(re.findall(r"^\s+([a-z-]+)\s+(\d+)$", out, re.M))
        total = sum(int(n) for n in counts.values())
        if total == 0 or int(counts.get("in-sync", 0)) != total:
            raise SmokeError(f"not every entity is in sync:\n{out}")

    def deploy(self) -> None:
        out = self.run("deploy", "--apply").stdout
        match = re.search(r"import read-back: (\d+) matching, (\d+) not kept", out)
        if not match or match.group(2) != "0":
            raise SmokeError(f"the deploy was not read back as sent:\n{out}")
        self.in_sync()

    def call_names(self, thing: str) -> list:
        out = self.run("call", thing, "GetRows", '{"limit": 2}', "--json").stdout
        return re.findall(r'"name":\s*"([^"]*)"', out)


def server_has_smoke_entities(smoke: Smoke) -> bool:
    """Whether the server holds a ZZ.Twaco.Smoke.* entity; a search that fails is an error, never
    an empty answer, so the script cannot deploy over (and then delete) entities it did not make."""
    result = smoke.run("search", PREFIX, ok=False)
    if result.returncode != 0:
        raise SmokeError(f"cannot search the server for {PREFIX}.*:\n{result.stderr}")
    return PREFIX in result.stdout


def build(smoke: Smoke) -> None:
    run = smoke.run
    run("new", "project", PROJECT, "--apply")
    run("new", "data-shape", SHAPE, "--field", "id:STRING:primary", "--field", "name:STRING", "--apply")
    run("new", "thing", THING, "--template", "GenericThing", "--apply")
    script = smoke.root / "rows.js"
    script.write_text(
        f'const rows = Resources["InfoTableFunctions"].CreateInfoTableFromDataShape('
        f'{{ infoTableName: "Rows", dataShapeName: "{SHAPE}" }});\n'
        'for (let i = 1; i <= limit; i++) {\n'
        '    rows.AddRow({ id: "R" + i, name: "Row " + i });\n'
        '}\n'
        'result = rows;\n',
        encoding="utf-8",
    )
    run("thing", "service", "add", THING, "GetRows", "--param", "limit:INTEGER",
        "--returns", f"INFOTABLE:{SHAPE}", "--script", str(script), "--apply")
    run("mashup", "new", MASHUP, "--apply")
    run("mashup", "add", MASHUP, "grid", "--apply")
    run("mashup", "bind", MASHUP, "--from", f"data:{THING}.GetRows", "--to", "ui:ptcsgrid-3.Data", "--apply")
    run("mashup", "on", MASHUP, "--event", "mashup:Loaded", "--invoke", f"data:{THING}.GetRows", "--apply")
    run("mashup", "recipe", "columns", MASHUP, "ptcsgrid-3", "--column", "id:ID", "--column", "name:Name", "--apply")
    run("check")


def exercise(smoke: Smoke) -> None:
    smoke.deploy()
    names = smoke.call_names(THING)
    if names[:2] != ["Row 1", "Row 2"]:
        raise SmokeError(f"GetRows returned {names}")

    # A script edit pushed alone.
    script = smoke.root / "src" / THING / "services" / "GetRows" / "script.js"
    script.write_text(script.read_text(encoding="utf-8").replace('"Row "', '"Item "'), encoding="utf-8")
    smoke.run("sync", "--all")
    smoke.run("entity", "push", f"Things/{THING}", "--apply")
    smoke.in_sync()
    names = smoke.call_names(THING)
    if names[:1] != ["Item 1"]:
        raise SmokeError(f"after the push GetRows returned {names}")

    # A rename that ripples into the mashup, deployed, and the old Thing deleted.
    smoke.run("rename", "entity", THING, RENAMED, "--text", "--apply")
    smoke.run("check")
    smoke.deploy()
    smoke.run("entity", "delete", "--renamed", "--apply")
    names = smoke.call_names(RENAMED)
    if names[:1] != ["Item 1"]:
        raise SmokeError(f"after the rename GetRows returned {names}")
    smoke.in_sync()


def clean(smoke: Smoke) -> None:
    entities = [
        f"Mashups/{MASHUP}",
        f"Things/{RENAMED}",
        f"Things/{THING}",
        f"DataShapes/{SHAPE}",
        f"Projects/{PROJECT}",
    ]
    for entity in entities:
        # One at a time: an entity a failed step never created is simply absent.
        smoke.run("entity", "delete", entity, "--allow-repository-defined", "--apply", ok=False)
    if server_has_smoke_entities(smoke):
        raise SmokeError(f"the server still has {PREFIX}.* entities; delete them by hand")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--twaco", default=shutil.which("twaco") or "twaco")
    parser.add_argument("--keep", action="store_true", help="leave the entities on the server")
    args = parser.parse_args()
    missing = [name for name in ("TWACO_URL", "TWACO_USERNAME", "TWACO_PASSWORD") if not os.environ.get(name)]
    if missing:
        print(f"set {', '.join(missing)}: the server to test against", file=sys.stderr)
        return 2
    root = pathlib.Path(tempfile.mkdtemp(prefix="twaco-smoke-"))
    (root / "twaco.toml").write_text(f'[[project]]\nname = "{PROJECT}"\n', encoding="utf-8")
    smoke = Smoke(args.twaco, root)
    try:
        existing = server_has_smoke_entities(smoke)
    except SmokeError as error:
        print(f"FAILED before any change: {error}", file=sys.stderr)
        shutil.rmtree(root, ignore_errors=True)
        return 2
    if existing:
        print(f"the server already has {PREFIX}.* entities; delete them first", file=sys.stderr)
        shutil.rmtree(root, ignore_errors=True)
        return 2
    failure = None
    try:
        build(smoke)
        exercise(smoke)
    except SmokeError as error:
        failure = error
    finally:
        if not args.keep:
            try:
                clean(smoke)
            except SmokeError as error:
                failure = failure or error
        shutil.rmtree(root, ignore_errors=True)
    if failure:
        print(f"FAILED: {failure}", file=sys.stderr)
        return 1
    print("smoke test passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
