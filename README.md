<div align="center">

<img src="assets/icon/twaco-512.png" alt="" width="128" height="128">

# twaco

**Keep a ThingWorx solution in Git, and work on it from a terminal or an AI agent.**

[![Release](https://img.shields.io/github/v/release/butteredstardust/twaco?style=for-the-badge&logo=github&color=181717)](https://github.com/butteredstardust/twaco/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/butteredstardust/twaco/ci.yml?branch=main&style=for-the-badge&logo=github&label=CI)](https://github.com/butteredstardust/twaco/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue?style=for-the-badge)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-lightgrey?style=for-the-badge)](https://github.com/butteredstardust/twaco/releases/latest)

<br>

![twaco guide quirks lists the verified platform quirks](documentation/images/guide-quirks.png)

</div>

---

ThingWorx keeps a solution's code inside entity XML, with every service script in a CDATA
section. That is hard to edit, review, diff or hand to an AI agent. twaco makes your repository
the source of truth: edit scripts as plain files, check them offline, and deploy without
overwriting anyone else's work. It is one binary for Windows, Linux and macOS, with no runtime
to install.

## What you can do

- **Edit scripts as files.** `extract` writes service scripts, DataShape fields, mashups and
  DataTable settings to files under `src/`; `sync` puts them back, changing nothing else.
- **Start from what you have.** `init` sets up a repository from a folder of exports, or with
  `--from-server` from projects on a server; `entity pull` takes in later server changes.
- **Build without Composer.** `new` creates Things, templates, shapes, DataShapes, DataTables,
  timers, styles, menus and more; `thing` edits services, properties and subscriptions;
  `mashup` builds mashups from templates, with widgets, bindings and events.
- **Refactor safely.** `rename`, `move` and `copy` follow every reference through entities,
  scripts and mashups, and show the plan first.
- **Understand before you change.** `catalog` shows a service's signature and script, `impact`
  what a change reaches, `trace` how callers use a service, `unused` what nothing uses, and
  `docs` writes the solution up as Markdown.
- **Catch problems before the server does.** `check` runs every gate in one pass: sidecars in
  sync, formatting, script traps, mashup bindings, localization tokens, structure against
  ThingWorx's export schema, and your own hooks. `types --check` type-checks scripts.
- **Deploy without clobbering.** `deploy` has the server parse every script, refuses when
  someone changed an entity since your last deploy, and reads back what it imported.
- **Take in a designer's work.** `adopt` compares a Composer export with the repository and
  shows only what really changed.
- **Use the server from the terminal.** Call services (as a test user, with their logs), read
  logs, run SQL, copy DataTable rows, and manage permissions, localization, users, application
  keys and file repositories.
- **Build extensions and building blocks.** `ext` creates, builds and deploys Java and widget
  extensions; `block` and `new building-block` work with the Solution Framework.
- **Look things up offline.** `guide` lists platform quirks verified on a live server; `help`
  and `javadoc` search the ThingWorx help center and Java API.

## Install

Download the installer for your platform from the
[latest release](https://github.com/butteredstardust/twaco/releases/latest):

| Platform | File |
| --- | --- |
| Windows x64 | `twaco-<version>-x64-setup.exe` (no administrator needed) |
| macOS (Apple Silicon) | `twaco-<version>-aarch64-apple-darwin.pkg` |
| Linux x64 (Debian, Ubuntu) | `twaco_<version>_amd64.deb` |
| Linux x64 (other) | `twaco-<version>-x86_64.AppImage` |

- The macOS package is unsigned: right-click it and choose Open.
- Linux builds need glibc 2.35 or later (Ubuntu 22.04).
- Each release also has a plain archive per platform. Or build from source:
  `cargo install --git https://github.com/butteredstardust/twaco`.

`twaco update --apply` installs a newer release (not for a `.deb` or AppImage). `types --check`
also needs TypeScript (`npm install -g typescript`).

## Quick start

In a repository of ThingWorx entity exports (`Things/`, `ThingShapes/`, `Mashups/`, ...):

```sh
twaco init --write        # write twaco.toml, plus AGENTS.md and CLAUDE.md for agents
twaco extract --all       # scripts, DataShapes and mashups out to files under src/
twaco check               # every gate, one exit code
```

To reach a server, create `.twaco/profiles/default.toml` (git-ignored) and run `twaco doctor`:

```toml
url = "http://localhost:8080/Thingworx"
username = "Administrator"
password = "..."
```

Then the everyday loop, after editing `src/<Entity>/services/<Service>/script.js`:

```sh
twaco sync <Entity>                     # write the edit back into the entity XML
twaco deploy --only <Entity>            # the plan: what it imports, and any conflict
twaco deploy --only <Entity> --apply    # do it
twaco call <Thing> <Service> '{"x": 1}' --with-logs
```

The [quick start guide](documentation/QUICK_START.md) walks through it in full.

## Use it with an AI agent

twaco is also an MCP server, so Claude Code, Codex, Cursor or any MCP client can use its
commands as tools. From the solution's root:

```sh
twaco mcp configure claude-code --apply   # or codex, cursor; without --apply it shows the plan
```

Tools that change a server are dry runs unless the agent says otherwise, and results are short
summaries unless it asks for detail. `twaco mcp --tools core,entity` serves only the tool
groups an agent needs. See [MCP server](documentation/MCP_SERVER.md).

## Safe by default

- **Plans first.** Every command that changes a server shows a plan until you add `--apply`.
  The one exception is `twaco call`, which runs the service you name.
- **Conflicts are refused.** `deploy` and `entity push` will not overwrite a change someone made
  on the server since your last deploy, unless you `--force` it.
- **Changes are all or nothing.** Commands that change several files write them in one
  transaction, which the next command finishes or undoes after a crash.
- **Deletes are guarded and undoable.** `entity delete` refuses while something still depends
  on the entity, and keeps a backup for `entity restore`.
- **Credentials stay out of Git.** Profiles live under `.twaco/profiles/` or your home folder
  and are never printed.

## Documentation

| Document | What it covers |
| --- | --- |
| [Quick start](documentation/QUICK_START.md) | From a folder of exports to a first deploy |
| [User guide](documentation/USER_GUIDE.md) | How twaco works, and every part of the work with it |
| [Commands](documentation/COMMANDS.md) | Every command and flag |
| [Configuration](documentation/CONFIGURATION.md) | `twaco.toml` and server profiles |
| [MCP server](documentation/MCP_SERVER.md) | Setting up an agent, and the tools it gets |
| [Knowledge](documentation/KNOWLEDGE.md) | `guide`, `catalog`, `help`, `javadoc` and `types` |
| [Architecture](documentation/ARCHITECTURE.md) | How twaco is built, for contributors |

Contributors also have [Mutation classes](documentation/MUTATION_CLASSES.md),
[Transactions](documentation/TRANSACTIONS.md), [Command facade](documentation/COMMAND_FACADE.md),
[MCP schemas](documentation/MCP_SCHEMAS.md) and [Testing](documentation/TESTING.md).

## Contributing

Bug reports, fixes and new gates are welcome; read [CONTRIBUTING.md](CONTRIBUTING.md) first.
Report security issues privately, as [SECURITY.md](SECURITY.md) describes.

## License

[MIT](LICENSE). The maintainer made the twaco icon in `assets/icon`, under the same license.

ThingWorx is a trademark of PTC Inc. twaco is an independent project, not affiliated with,
sponsored or endorsed by PTC. The help center and Javadoc that `twaco help` and
`twaco javadoc` read are fetched from PTC's public site at run time and cached on your machine.
