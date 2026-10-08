# Configuration

twaco reads two kinds of configuration:

- **`twaco.toml`** describes the solution. It is committed. twaco looks for it in the working
  directory and then upwards, so any command works from any folder of the solution.
- **Server profiles** say how to reach a ThingWorx server. They hold credentials and are
  never committed.

`twaco init` proposes a `twaco.toml` from the entities themselves; most solutions need little
beyond what it writes.

## `twaco.toml`

A complete example. At least one `[[project]]` table is required, with its `name`. Every
other table is optional, but one you declare needs its own required keys: a `[[check]]` its
`name` and `command`, a `[[project.deploy.post_import]]` its `thing` and `service`.

```toml
[solution]
name = "Acme"                 # used for package metadata and AGENTS.md
src = "src"                   # where sidecars live (default "src")
dist = "dist"                 # where bundles are written (default "dist")

[[project]]
name = "Acme.Core"            # required: the projectName its entities carry
root = "core"                 # where its collection folders are (default ".")
collections = []              # limit to these collection folders (default: all found)
depends_on = []               # projects that must be imported first

[[project]]
name = "Acme.Dashboards"
root = "dashboards"
depends_on = ["Acme.Core"]

# Optional: a service to call after this project's entities are imported and read back.
[project.deploy]
entry_point_thing = "Acme.Dashboards.EntryPoint"
deploy_service = "DeployComponent"
deploy_parameters = { deploymentConfig = { databasePassword = "${profile:database_password}" } }

[[project.deploy.post_import]]
thing = "Acme.Dashboards.Manager"
service = "SeedDefaults"
parameters = { overwrite = false }

[bundle]
name = "bundle.xml"                   # the whole-solution document (default)
backend_name = "bundle.backend.xml"   # the backend-only document (default)
ui_collections = ["Mashups", "StyleThemes", "MediaEntities"]  # what a designer owns

[format]
indent_cdata_payload = false  # true keeps scripts indented to their <code> element

[gates]
live = false                  # true: every `check` also has the server parse every script
advisory = []                 # built-in gates that report without failing, e.g. ["code order"]

[validate]
inherited_overrides = ["Acme.Dashboards.Manager.GetDashboards"]

[unused]
keep = ["Acme.Dashboards.Database", "Things/Acme.Dashboards.Api.*"]  # used from outside

[adopt]
ignore_paths = ["Things/Acme.Dashboards.Database/**/password"]
generated_services = ["Acme.Dashboards.Manager.GetVersion"]

[types]
tsc = ["npx", "tsc"]          # the TypeScript compiler command (default: tsc on PATH)

[help]
version = "10.1"              # the help center release (default: the server's own)

[repositories]
root = "filerepository"       # one subfolder per FileRepository (default)

[package]
version = "1.2.0"             # extension package version (default 1.0.0)
group = "com.acme"            # groupId (default: the solution's name)
vendor = "Acme"
minimum_thingworx = "9.6.0"   # (default 9.0.0)

[knowledge]
paths = ["AGENTS.md", "docs"] # markdown `twaco guide` reads (default AGENTS.md, CLAUDE.md, docs/)

[[check]]
name = "eslint"
command = ["npx", "eslint", "src", "--format", "json"]
gate = true                   # a non-zero exit fails `check` (default true)
needs_credentials = false     # pass TWACO_*/TWX_* variables to it (default false)
timeout_seconds = 120         # (default 120)
```

### `[solution]`

| Key | Default | Meaning |
| --- | --- | --- |
| `name` | empty | The solution's name: the default package group, and the name AGENTS.md uses. |
| `src` | `src` | The sidecar root, relative to the solution. Each entity's sidecars are in `<src>/<Entity>/`. |
| `dist` | `dist` | Where `twaco bundle` writes. |

### `[[project]]`

One table per ThingWorx project. A single-project repository has one.

| Key | Default | Meaning |
| --- | --- | --- |
| `name` | required | The `projectName` the project's entities carry. An entity belongs to the project its own `projectName` names; a file filed under another project's folder is reported. |
| `root` | `.` | The folder holding the project's collection folders (`Things/`, `Mashups/` and so on). |
| `collections` | all | Only these collection folders. |
| `depends_on` | none | Projects imported before this one. `deploy` orders projects by it, and an extension package's `dependsOn` lists them. |

**`[project.deploy]`:** a project can ask for a service call once its entities are imported
and read back. `entry_point_thing` and `deploy_service` name it, and `deploy_parameters` is
its JSON input as a TOML table. Each `[[project.deploy.post_import]]` names one more call:
`thing`, `service`, optional `parameters`, and `target` when the host is not a Thing
(`ThingTemplates/<Name>`, for instance).

TOML attaches `[project.deploy]` and `[[project.deploy.post_import]]` to the last `[[project]]`
written above them, by position. Keep a project's deploy tables directly under that project, and
add a new `[[project]]` after them, not before: a table written under the wrong project belongs to
that project.

A `${profile:key}` placeholder anywhere in those parameters is replaced at deploy time by
`key` from the server profile. A secret, such as a database password, then lives in the
profile and never in the repository. Plans and errors show the placeholder, never the value.
A string that is exactly one placeholder takes the profile value with its type, so
`"${profile:port}"` can send a number. A placeholder inside a longer string, such as a
connection URL or a JSON document passed as a STRING parameter, is replaced by the value's
text, as it is: a value holding `"` or `\` inside a JSON string must be written escaped in the
profile. Only a string, number, boolean or date can sit inside a longer string; an array or a
table there, like an unknown key, stops the deploy before anything is imported.

### `[bundle]`

`twaco bundle` and `twaco deploy` build one importable document from the repository.
`ui_collections` names the collections a designer owns in Composer. `--backend-only`
leaves them out, so backend work can deploy without rolling back a designer's unexported
changes. `twaco package bundle --frontend-only` takes only them.

### `[format]`

`indent_cdata_payload = true` keeps scripts indented to their `<code>` element inside the
XML, as some older tools wrote them. The default, `false`, writes them flush left. `init`
proposes whichever the repository already uses, so adopting twaco never relayouts a
repository on its own. `twaco sync --all --relayout` moves a repository to the configured
layout once.

### `[gates]`

`live = true` makes every `twaco check` also send every script to the server's parser, as
`check --live` does. That gate fails closed: without a reachable server, `check` fails.
`deploy` always parses on the server, whatever this says.

`advisory` names built-in gates whose findings are reported (as `warn`) without failing
`check` or blocking `deploy`: `line endings`, `sidecars`, `formatting`, `script traps`,
`code order`, `project`. The live parse is not among them: `deploy` always has the server
parse every script, and stops on a script it refuses. It is for a solution adopting twaco over code written
before those gates existed; take a gate off the list once its findings are fixed. A gate that
cannot run still fails, and a name that is not a gate is refused.

### `[validate]`

`inherited_overrides` lists `Entity.Service` names that implement locally a service whose
definition comes from a shape or template; a bare `Service` allows it on every entity. That is legitimate, and impossible to tell from a
mistake without being told, so `check` reports each one until it is listed here.

### `[unused]`

What `twaco unused` treats as in use without anything in the repository referring to it:

- **`keep`:** names or `Collection/Name` patterns, `*` standing for any run of characters. A
  database connection Thing that the platform calls, an entity a REST client or a connected
  system uses, anything reached from outside the repository, belongs here. A pattern that matches
  no entity is reported, so a typo does not silently keep nothing.

Mashups, things a deploy runs (`[project.deploy]`) and anything that runs on events
(subscriptions, Timers, Schedulers) are entry points already and need no entry.

### `[adopt]`

How `twaco adopt` reads a designer's export:

- **`ignore_paths`:** node paths never reported as a change, written
  `Collection/Entity/node/path`. `*` matches within one step and `**` across steps. Use it
  for values that differ on every export by design, such as live property values or a
  password encrypted with the designer's key.
- **`generated_services`:** `Entity.Service` names whose body this repository generates, so
  the repository wins.

### `[types]`, `[help]`, `[repositories]`, `[package]`, `[knowledge]`

| Key | Meaning |
| --- | --- |
| `types.tsc` | The TypeScript compiler command for `types --check`, before twaco's own `-p` arguments. Default: `tsc` (`tsc.cmd` on Windows) on `PATH`. |
| `help.version` | The help center release `twaco help` reads, such as `10.1`. Default: the server's version, else the newest. |
| `repositories.root` | The folder holding one subfolder per FileRepository, for `repo status`, `push` and `pull`. Default `filerepository`. |
| `package.*` | Extension metadata for `twaco package extension`: `version` (1.0.0), `group` (the solution's name), `vendor`, `minimum_thingworx` (9.0.0). Each project's `artifactId` is its name in lower case. |
| `knowledge.paths` | Markdown files or folders `twaco guide` reads besides its built-in topics. Each must exist and stay inside the solution. Default: AGENTS.md, CLAUDE.md and `docs/`, where they exist. |

### `[[check]]`: your own gates

Each `[[check]]` runs a command as part of `twaco check`, from the solution root, with no
input.

- **Findings:** a line it prints as a JSON object with a `message` becomes a finding. It may
  also carry `file`, `line`, `rule` and `gate`. Any other line is kept as text.
- **Exit status:** a non-zero exit with no findings is reported as one finding.
- **Limits:** a command that outlives `timeout_seconds` is killed and reported, and so is one
  that prints too much.
- **Credentials:** `TWACO_*` and `TWX_*` variables are removed from its environment unless it
  sets `needs_credentials = true`.
- **`gate = false`:** the check reports without failing the run.

`twaco types --check --json` speaks this protocol, so
`command = ["twaco", "types", "--check", "--json"]` adds type checking to `check`.

## `permissions.toml`: who may use a project

A project's permission policy lives in `permissions.toml` in the project's root folder (the
`root` of its `[[project]]`). It is the source of truth for the project's permissions:
`twaco permissions audit` checks the entity XML against it, offline, and `twaco permissions apply`
writes it there. `twaco permissions init` drafts one from what a project grants today. A project
without the file is left alone.

The policy needs nothing from the Solution Framework. When the project has a permission helper
Thing (template `PTCDTS.Base.ComponentPermissionHelper_TT`, which ships in `PTCDTS.Base` and so
exists on a server with only the common blocks), the project is in helper mode.

```toml
project = "Acme.App"           # optional; must be the project whose folder holds the file
mode = "auto"                  # auto (helper mode when a helper Thing exists) | helper | plain
organization = "Default_OR"    # default; a name without a dot is in the project
strict = ["Orders_TS"]         # every service of these entities must match a [[runtime]] rule
unmanaged = ["Legacy*"]        # entities whose blocks the policy leaves alone

[[role]]
name = "viewer"                # the permission helper's column name
group = "Viewer_UG"            # Acme.App.Viewer_UG

[[role]]
name = "editor"
group = "Editor_UG"
includes = ["viewer"]          # an editor gets every grant a viewer gets

[[role]]
name = "allUsers"
group = "Default_UG"
org = "organization"           # visible through Acme.App.Default_OR itself

[[runtime]]
entities = ["Orders_TS"]       # full names or the part after the project's prefix; globs
action = "ServiceInvoke"       # the default; or PropertyRead, PropertyWrite, EventInvoke, EventSubscribe
resources = ["Get*"]           # `*` is every named resource
except = ["GetSecret"]
roles = ["viewer"]

[[runtime]]
entities = ["Orders_TS"]
resources = ["GetSecret"]
roles = []                     # classified, granted to no one

[visibility]
roles = ["viewer", "editor", "allUsers"]   # the default: every role
remove = ["PTC.SolutionFramework.*"]       # principals to drop from every managed block

[[visibility.rule]]            # the first rule that names an entity decides
types = ["Mashup"]
names = ["*Admin*"]
roles = ["editor"]
```

| Key | Meaning |
| --- | --- |
| `[[role]]` | `name`, `group`, optional `org` and `includes`. A role's visibility principal is the organizational unit `<organization>:<group>`; `org = "organization"` uses the organization itself, `org = "none"` gives no visibility, and any other value is a full principal (with a `:`, a unit). |
| `[[runtime]]` | Allows `roles`, and every role that includes them, the `action` on the `resources` of the `entities`. Resources are what the entity defines, what its block lists, and the rule's literal names (a service inherited from a template is named literally). `entity_wide = true` also grants the entity-wide resource, which ThingWorx writes `*`. |
| `[visibility]` | `roles` see every entity no rule names. Principals the roles do not own are kept, unless `remove` names them. |
| `[[platform]]` | Grants and memberships outside the project, which an import cannot carry: `grant = { entity = "Resources/EntityServices", action = "ServiceInvoke", resource = "ReadEntityDefinitionAsJSON" }` or `member_of = "<group>"`, with `roles` (and every role that includes them) and an optional `requires = "<project>"`. `permissions audit --server` checks them; `permissions push --platform` adds what is missing and never removes. |

The policy owns the run-time block of each Thing in the project, and the instance run-time block
of each ThingShape and ThingTemplate, unless `unmanaged` names the entity: a grant no rule makes
is a difference. It owns the role principals of each entity's visibility block. Rules only allow.

**Helper mode.** In helper mode the policy also owns the helper Thing's three tables and the
columns of the two DataShapes behind them: `RoleGroupsAndOrganizations` (a `<role>Group` and a
`<role>Org` row per role), `RunTimePermissionsTable` (a row per entity, resource and action, a
`<role>Group` column per role) and `VisibilityPermissionsTable` (a row per entity, a `<role>Org`
column per role). A role's `name` is therefore the helper's column name. The run-time rows the
helper has keep their order and IDs; a row is added for each service of a Thing, ThingShape or
ThingTemplate that has none, and for each granted resource without one. The helper's mashup then
shows what the entity XML grants, and applying it there changes nothing.

## Diagnostic logs

Logs are off by default. They show what twaco did when a command or an MCP tool misbehaves.
They never hold a credential, a header, a request body, a response body or an MCP tool's
`arguments`. A subprocess log holds the program and the number of arguments, never the
argument values. An MCP message log holds a numeric `id`, and `<string>` for a string `id`.

| Flag | Variable | Effect |
| --- | --- | --- |
| `--log <filter>` | `TWACO_LOG` | Turn logs on. Write them to stderr. |
| `--log-file <path>` | `TWACO_LOG_FILE` | Append logs to a file instead of stderr. |

- Every command takes both flags, including `twaco mcp`.
- A flag overrides its variable.
- `--log-file` alone means `debug`.
- A filter is a level (`error`, `warn`, `info`, `debug`, `trace`) or a directive list such as
  `twaco::core::server=trace`. A bare level applies to twaco only, so dependencies stay quiet.
- Levels: `warn` for an abnormal state twaco recovers from, `info` for one line per command,
  MCP message and deploy phase, `debug` for each server request, lock action, transaction
  stage and subprocess.
- An invalid filter or an unopenable file prints one `twaco:` warning on stderr. The command
  then runs without logs.
- Twaco never writes a log to stdout. A log file that is standard output (`/dev/stdout`) gets
  the same warning and is ignored. This check runs on Unix only.

```sh
twaco deploy --log debug
twaco entity status --all --log-file twaco.log
```

## Progress

Some commands send many requests or wait a long time for the server. These commands show
progress on stderr:

- `deploy`, including the live parse gate, one step per service script
- `entity status` and `entity push`
- `check --live`, one step per service script
- `permissions` (compare and push), `permissions push --platform` (one step per platform
  entry), `permissions audit` and `permissions apply`
- `repo` (list, status, get, change and sync)
- `types --platform`, and the type check of `types --check`
- `import` and `export`
- `ext import`

Progress follows these rules:

- A bar shows when stderr is a terminal. With a pipe, a file or a CI run, twaco draws nothing.
- Logs on stderr (`--log` without `--log-file`) switch the bars off. Use `--log-file` to keep
  both.
- Twaco never draws on stdout. Stdout is the same with or without a bar.
- A bar shows phase names and entity names. It never shows a URL, a parameter or a credential.

## Colour

Twaco colours status words in terminal output. Each stream is decided on its own.

- Stdout carries `ok`, `FAIL`, `BROKEN`, `warn` and the status verdicts.
- Stderr carries the `twaco:` prefix of an error.
- Colour is off when the stream is not a terminal. Piped output has no escape codes.
- `NO_COLOR` set to any non-empty value turns colour off. It wins over every other setting.
- `FORCE_COLOR` set to `1` turns colour on, also for a pipe. `NO_COLOR` wins over `FORCE_COLOR`.
- A CI run (`CI` set) turns colour off unless `FORCE_COLOR` is set.

The text is the same with and without colour. Only the escape codes differ.
MCP output never has colour.

## Server profiles

A profile is a TOML file named after it, `<name>.toml`, looked for in this order:

1. `.twaco/profiles/<name>.toml` in the solution;
2. `~/.twaco/profiles/<name>.toml` in your home folder.

```toml
url = "http://localhost:8080/Thingworx"   # the server's /Thingworx address
username = "Administrator"
password = "..."
app_key = "..."                           # optional; see below
database_password = "..."                 # any other key, for ${profile:key} placeholders
```

Commands use the profile `default` unless given `--profile <name>`, or `profile` over MCP.
Environment variables override a profile's fields, or replace the file entirely:
`TWACO_URL`, `TWACO_USERNAME`, `TWACO_PASSWORD` and `TWACO_APP_KEY` (or the `TWX_` spellings).

twaco signs in with the username and password: ThingWorx's Importer, which every deploy
uses, does not accept app keys. A profile is only read by a command that talks to a server,
so offline commands work without one. Profile values are never printed; `twaco doctor`
shows which file or variables a profile resolved from.

Keep profiles, and the backups a delete or a forced push saves, out of git: a backup holds the server's
copy of an entity as it is, a database Thing's password included. A solution's `.gitignore` should
hold the lines below. `twaco init --write` and `twaco init --agents` add the missing ones, and
`twaco doctor` warns when any is missing. An ignore line does not untrack a file committed before it, so
`twaco doctor` also asks git whether it tracks anything under `.twaco/profiles`, `.twaco/backups` or
`.twaco/transactions`: a tracked backup is a warning, a tracked profile a failure (history keeps it,
so change the password it holds).

```gitignore
.twaco/profiles/
.twaco/lock
.twaco/lock.holder
.twaco/backups/
.twaco/transactions/
.twaco/types/
.twaco/platform.json
**/services/*/jsconfig.json
**/services/*/twaco-globals.d.ts
```

Commit `.twaco/baseline.json`. It records each entity's state at the last deploy or push (and
what `entity status --record` adopted), so a later deploy can tell a teammate's server-side
change from yours.

Each recorded hash carries the version of the comparison form it was made with (`v5:...`). When a
new twaco compares differently, an entry from the old version counts as unrecorded rather than
as a change on both sides: an entity whose two sides match reads `no-baseline-same` and is
recorded again by its next deploy, and `twaco doctor` counts the old entries.
`twaco entity status --all --record` records again, at once, every entity that matches the server.
