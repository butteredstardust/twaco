"""Regenerate documentation/COMMANDS.md from twaco's own usage text, grouped by task.

    cargo build && python scripts/commands_doc.py

Fails when a command in the usage text belongs to no group, so a new command is placed by a
person rather than dropped.
The usage text itself is held to every flag the parser accepts by a test in src/main.rs.
"""
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
binary = ROOT / "target" / "debug" / ("twaco.exe" if sys.platform == "win32" else "twaco")
usage = subprocess.run([str(binary)], capture_output=True, text=True).stderr

blocks = []  # [key, lines]
for line in usage.splitlines()[2:]:
    if not line.strip():
        continue
    if line.startswith("  ") and not line.startswith("    "):
        words = line.split()
        key = words[0] if words[0] != "entity" else f"entity {words[1]}"
        blocks.append([key, [line[2:]]])
    elif line.startswith("exit:"):
        blocks.append(["exit", [line]])
    else:
        blocks[-1][1].append(line[2:])

GROUPS = [
    ("Set up", "Describe a solution, check the environment, serve agents, update twaco.",
     ["init", "doctor", "projects", "mcp", "update"]),
    ("Work on the repository", "Offline: sidecars, gates, types, the service catalog, what a change reaches, a designer's export, renames.",
     ["extract", "sync", "fmt", "check", "types", "catalog", "impact", "unused", "docs", "adopt", "rename", "move", "copy", "retemplate", "new"]),
    ("Deploy and compare", "Against a server: what differs, what would be imported, and doing it.",
     ["entity status", "entity get", "entity push", "entity delete", "entity carry", "entity restore", "datatable", "bundle", "deploy", "config-table", "db"]),
    ("Run and observe", "Call services and read what the server says.",
     ["call", "logs", "settings"]),
    ("Server content", "File repositories, extension packages, Composer-style exports and imports.",
     ["repo", "ext", "export", "import"]),
    ("Release", "Package the repository, offline.", ["package"]),
    ("Knowledge", "Workflow, platform quirks, project documents, the help center and the Java API.",
     ["guide", "help", "javadoc"]),
    ("Shared", "Flags described once for the commands that list them, and exit codes.", ["--project", "--version", "exit"]),
]

by_key = {}
for key, lines in blocks:
    by_key.setdefault(key, []).extend(lines)
placed = {k for _, _, keys in GROUPS for k in keys}
missing = [k for k in by_key if k not in placed]
if missing:
    sys.exit(f"usage commands in no group: {missing}; add them to GROUPS")

out = ["# Commands", "",
       "Every twaco command and flag, from `twaco` run with no arguments. Commands that change a server",
       "print a plan unless given `--apply`; `call` is the exception, because twaco cannot tell whether a",
       "service writes. A server command takes `--profile <name>` (default: `default`); see",
       "[Configuration](CONFIGURATION.md#server-profiles).", ""]
for title, intro, keys in GROUPS:
    out += [f"## {title}", "", intro, "", "```text"]
    for key in keys:
        out += by_key.get(key, [])
    out += ["```", ""]
(ROOT / "documentation" / "COMMANDS.md").write_text("\n".join(out), encoding="utf-8", newline="\n")
print(f"documentation/COMMANDS.md: {len(by_key)} commands")
