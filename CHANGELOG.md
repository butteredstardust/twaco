# Changelog

All notable changes to twaco are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Diagnostic logs for every command, including `twaco mcp`. `--log <filter>` or `TWACO_LOG`
  writes them to stderr. `--log-file <path>` or `TWACO_LOG_FILE` appends them to a file.
  Logs are off by default. They never hold credentials, headers or bodies.

## [0.1.5] - 2026-10-08

### Added

- The MCP server has a `doctor` tool and a `bundle` tool (a dry run says whether the configured
  bundle is current, as `twaco bundle --check`; `dry_run: false` rebuilds it), and
  `config_table` can write a backup into the solution (`action: "backup"`).
- MCP `extract` and `sync` take several entities (`entities`), and `settings` shows one table
  (`table`), as the command line does.

### Changed

- MCP `guide` with no action lists the topics, searches when given text and reads when given a
  topic, as `twaco guide` does; before, it searched and refused for want of text.
- MCP `logs` returns 100 entries unless asked for more, as the command line does (was 200).
- `documentation/MCP_SERVER.md` says why `init` and `update` have no tool, and what differs
  between the tools and the commands on purpose.

### Fixed

- `doctor` showed a profile's `url` as written, credentials in it included; it now shows `***`
  in their place.
- `bundle --backend-only` worked out its notes over every collection instead of the ones it
  bundles, so a reference to an entity the backend bundle leaves out went unreported.
- An MCP `config_table` restore read a backup from any path on the machine; like every file a
  tool reads, it must now be inside the solution.
- A server error quoted the request's address as written, so credentials put into a profile's
  `url` (`https://user:token@host/`) could be printed, in its message and its debug form; both
  now show `***` there, and a reply echoing them is scrubbed.
- Backup files were named by the low byte of each character, so `Café` and `Cafǩ` shared one file
  and a restore lost one of them. A set's `backup.json` now records each entity's file, unique
  even where case is not told apart and cut to a safe length; sets saved before still restore.
- `repo pull` wrote two server files whose paths differ only in case (`A.txt`, `a.txt`) to one
  file on Windows and macOS, keeping whichever came last. Such a pull is now refused.
- An import zip or an extension package whose entries expand without end was read into memory
  until it ran out; an entry past 512 MiB (1 MiB for `metadata.xml`) is now refused.
- What a `[[check]]` hook with `needs_credentials` printed of those credentials was shown by
  `twaco check`; it is now `<redacted>`, in its text and in its JSON findings once decoded.
- `retemplate --to` and `--add-shapes` wrote a name into the XML as given, so a quote in it
  (`'P.New" x="1'`) added an attribute; names are now escaped (`P.B&amp;C`).
- `sync` rewrote a script, a mashup's content or a DataTable value spread over several CDATA
  sections as one, dropping a comment between them; it now refuses and names the element.

## [0.1.4] - 2026-10-08

### Added

- `twaco search` and the MCP `search` tool find server entities as Composer's Spotlight does:
  text anywhere in a name or a description, entity types (singular or plural, an unknown one
  refused instead of searching everything), a project, and a limit that says when it cut the
  list. Results are `Collection/Name`, ready for `entity get` or `export`.
- The MCP `entity_get` tool returns one entity's XML as the server has it, cut at `max_chars`.
  `entity get` on the command line, and the tool, take `Collection/Name` for any entity on the
  server, in the repository or not.
- `twaco doctor` reports profiles, backups and journal backups that git already tracks, which an
  ignore line added later does not untrack. A tracked profile fails (change the credentials it
  holds: history keeps them); a tracked backup warns. Only file names are shown.

### Changed

- `adopt` makes its writes as one transaction: an export it cannot adopt in full, or a run that is
  killed partway, leaves the repository as it was. Before, a write that failed left the earlier
  ones in place.
- `extract` writes each entity's sidecars as one transaction, so a crash never leaves a service's
  new definition beside its old script, and it leaves a sidecar that already holds what it should
  alone, so an editor watching it sees no change. The count of entities it reports counts each
  entity once, not once per kind of sidecar.
- `sync` writes each entity file once, with every kind of sidecar in it, instead of once per kind.
- Every server address for an entity is built from a checked collection and name. An import file,
  a backup set's `backup.json` or a repository entity whose name is empty, `.`, `..` or holds a
  `/` is refused with the name, instead of asking the server about the collection itself or
  another entity.

## [0.1.3] - 2026-10-08

### Changed

- The command line is parsed by clap, from one table of commands (`src/cli/spec.rs`). Every
  command answers `--help`; a mistyped flag is refused with the flag it most likely meant
  (`--cehck`: did you mean `--check`); `--flag=value` works; and a command that takes no
  operands refuses one instead of ignoring it (`twaco fmt stray`). Errors still start
  `twaco:` and exit 2. An unknown command says so in one line and points to `--help` instead
  of printing every command. An operand may be a negative number (`call ... -5`); one that
  starts with a dash otherwise goes after `--`. `documentation/COMMANDS.md` is generated from
  the same table.

## [0.1.2] - 2026-10-07

### Added

- `permissions diff` compares the run-time, design-time and visibility permissions in the entity
  XML with the server's, and `permissions push` makes the server's exactly the repository's,
  reading each set back. An import only adds: it never removes a grant, and it keeps the server's
  allow or deny for a principal the server already lists, so a deny in the repository could be
  silently ignored (verified on a live server). A ThingShape's or ThingTemplate's instance
  permissions (what its Things get) are sets of their own. MCP tools `permissions` and
  `permissions_push`.
- A deploy whose read-back differs only in permissions says so, and names the commands above.
- `permissions audit` checks each project's permission policy, `permissions.toml` in the
  project's root folder, against the entity XML without a server: roles with the groups and
  organizational units behind them, which roles may use which resources, and who sees which
  entity. It reports blocks that differ from the policy, unclassified services of a strict
  entity, principals the server refuses or no entity defines, and rules that match nothing.
  A project with a Solution Framework permission helper Thing is found to be in helper mode;
  the Solution Framework itself is not needed. MCP tool `permissions_audit`. With `--server`
  it also reads the server: each entity's permissions, the helper's tables, the policy's
  `[[platform]]` grants and memberships (what DeployComponent does, which an import cannot
  carry), and each role's organizational unit.
- `permissions init` drafts a project's `permissions.toml` from what it grants today, from the
  entity XML or with `--from-helper` from the permission helper's tables, so that
  `permissions apply` then changes nothing; what a policy cannot say (a deny, a principal that is
  not a group) is left `unmanaged` with a note. `permissions push --platform` adds the policy's
  `[[platform]]` grants and memberships the server lacks, and never removes anything. MCP tool
  `permissions_init`, and `platform` on `permissions_push`.
- `permissions apply` writes the policy into the entity XML: each Thing's run-time block, each
  ThingShape's and ThingTemplate's instance run-time block, and the role principals of each
  visibility block. Only blocks that differ change, in the export's layout; every changed file
  is written in one transaction. MCP tool `permissions_apply`. In helper mode it also writes the
  permission helper's `RoleGroupsAndOrganizations`, `RunTimePermissionsTable` and
  `VisibilityPermissionsTable` and the columns of their two DataShapes, keeping existing rows and
  their IDs, so the helper's mashup shows what the XML grants; the audit compares them too.

### Fixed

- `deploy` replaces a `${profile:key}` placeholder anywhere inside a string parameter, such as a
  connection URL or a JSON configuration passed as a string. Before, only a string that was
  exactly one placeholder was replaced, and a longer string reached the server with the
  placeholder text in it, while the redacted plan looked right. An array or a table cannot be
  embedded in a string and is refused.
- `sync --allow-add-remove` adds a service from a new sidecar folder and removes one whose
  folder is gone, with the entity's run-time permissions for it. Before, the flag only silenced
  the refusal: a new service was reported "already in sync" and never written, although
  DataShape fields were added and removed. A folder whose `definition.xml` names another service
  is refused.
- `rename entity` finds the entity followed by one of its members, such as
  `Acme.App.Manager.GetOrders` in `[validate] inherited_overrides`, in `twaco.toml` and the other
  text files, and renames it. Followed by anything else that is not another entity, it is left for
  review. Before, such a name was neither changed nor counted, and the plan said "0 review".
- A rename planned with `--text` no longer says the other files "were not changed; pass --text".
- An entity with permissions no longer reads back as "not kept" after a deploy. ThingWorx
  reorders principals and resources on import and fills in the permission kinds a resource left
  out; the comparison now treats each permission list as a set. Without a matching read-back no
  baseline was recorded, so every later deploy of the entity needed `--force`.

### Changed

- Entity hashes are version 5 (`v5:`). The instance permission blocks of a ThingShape or
  ThingTemplate are compared as sets too, as the entity's own blocks are. A baseline entry recorded by an earlier twaco counts as
  unrecorded, not as changed: an entity that matches the server reads `no-baseline-same` and its
  next deploy records it again. `twaco doctor` counts such entries, and
  `twaco entity status --all --record` records them again at once.

## [0.1.1] - 2026-10-06

### Added

- twaco has an icon. `twaco.exe` shows it in Explorer, and `assets/icon/` holds the sources, a
  macOS `.icns` and a Windows `.ico`.
- Every merge to `main` that changes what ships publishes a release, with archives for Windows,
  Linux and macOS. The patch number bumps on its own; a minor or major version is set by hand in
  `Cargo.toml`.
- Installers: a per-user Windows setup that adds twaco to `PATH`, a macOS `.pkg`, a Linux
  `.deb` and an AppImage. The plain archives stay, with a minisign signature each, and
  `SHA256SUMS.txt` covers every asset.
- `update` compares twaco with the latest release. `update --apply` downloads the release for
  this platform, verifies its signature, and replaces the binary. It is command-line only.
- Once a day, a command run in a terminal says on stderr when a newer release exists.
  `TWACO_NO_UPDATE_CHECK=1` turns this off; `mcp`, CI and a redirected stderr never check.
  An old copy of the release manifest cannot hide a newer release that twaco saw before.

### Changed

- MCP tool input schemas are generated from the typed request objects the tools read their
  arguments through, so a schema can no longer drift from what a tool reads; the arguments a tool
  accepts and the messages for refused ones are unchanged. Clients that negotiate the `2025-06-18`
  protocol revision or later also get an `outputSchema` for `projects`, `check`, `status`, `sync`,
  `extract`, `fmt`, `types`, `push` and `deploy`. See `documentation/MCP_SCHEMAS.md`.
- MCP tool failures now carry a stable `code` alongside their unchanged message; refused `push`
  results carry the corresponding code as well.

- MCP tool input schemas now declare `additionalProperties: false` and nested arguments are
  validated; the published tool definitions are held by a golden file.
- `entity delete` now has separate acknowledgements for repository-defined entities, outside
  dependents and FileRepository data loss; deleting a FileRepository Thing is refused unless
  `--allow-file-repository-data-loss` is given, and refusals carry stable codes.
- Rename now analyses service scripts with a real ECMAScript parser instead of a lexical scan,
  so calls split over lines or inside template literals are found, and text in strings,
  comments and regular expressions is not mistaken for code. A script the parser refuses
  (Rhino-only syntax such as `for each`) is no longer edited; its mentions are left for review.
- A variable counts as a Thing only if every declaration of it names the same `Things` entity and
  it is never reassigned or taken as a parameter. A service rename follows an `@function` tag
  only inside comments.
- The `GetDBInfo` reader now uses the ECMAScript parser; a script it refuses or whose literal
  cannot be read completely is reported as unsure, and the former JavaScript lexer is removed.
- `twaco --version` and `twaco doctor` name the commit a build came from (`0.1.0 (a1b2c3d4e
  2026-10-06)`, with `+dirty` for uncommitted changes), so a stale binary can be told from a current
  one. A build outside a git checkout says only the version.
- `twaco init --write` and `twaco init --agents` add twaco's local state to the solution's
  `.gitignore` when it is in a git repository, backups and the transaction journal included (a backup
  holds a server's copy of an entity, secrets and all); `twaco doctor` warns when any line is missing.
  An existing file is only appended to, in its own line endings.
- An applied `entity delete` that saves a backup now takes the workspace lock, as one that marks
  the rename ledger already did: two applies in the same second no longer share one backup folder.
- `new building-block` writes the project it adds to `twaco.toml` in that file's line endings, so a
  CRLF file no longer fails the line-endings gate; the rest of the file is left exactly as it was.
- CONFIGURATION.md says that `[[project.deploy.post_import]]` tables belong to the last
  `[[project]]` written above them.

### Added

- A crash-recoverable journal for local multi-file writes (`core::transaction`). It writes its
  intent to `.twaco/transactions/` before the first change, stages every new file and keeps a copy
  of every original beside it, and the next command to take the workspace lock finishes the
  interrupted operation or undoes it. When a person has edited a file in between, it refuses,
  names every path with the digests found and expected, and changes nothing. `twaco new building-block --apply`, `twaco move` / `copy` and every `twaco rename` are on it: a
  crash while it writes leaves the workspace as it was, or finished, once the next command runs;
  see `documentation/TRANSACTIONS.md`.

- `twaco impact <entity> [--member <name>]` (and the `impact` MCP tool) reports what changing an
  entity, or one service, property or field of it, would reach: the Things, templates, shapes,
  mashups and projects that depend on it, directly or through others, each at the strength of its
  weakest reference (structural, resolved or review), with the projects in deploy order, as text,
  JSON or a Graphviz graph. It is offline and read-only, says what it cannot see, and lists any
  input it could not read.

- `twaco unused` (and the `unused` MCP tool) lists the Things, templates, shapes and DataShapes that
  no entry point reaches, with what still names each (a dead cluster names itself). Entry points
  are what `twaco.toml` deploys, every mashup, anything that runs on events (subscriptions, Timers,
  Schedulers) and what the new `[unused] keep` list names. It is advisory and deletes nothing.

- `twaco docs` (and the `docs` MCP tool) writes the solution down from the repository: the projects
  in deploy order, how Things, templates and shapes inherit, every service with its signature, the
  DataShapes with their fields and where they are used, a dependency diagram, and the references
  that only look like a name. The text has no dates, so regenerating it and diffing shows what
  changed. `--out <file>` writes it atomically and refuses to replace a file without `--force`.
  It says what it does not cover: permissions are not read yet.

### Changed (internal)

- State-changing commands now keep their policy (lock, profile, plan versus apply, backup) in one
  executor each under `core::commands`; the command line and the MCP server only parse and
  project. Output is unchanged. A command that takes the workspace lock now reports what taking it
  did (temporaries swept, interrupted operations recovered): printed on the command line and
  returned as `notices` by MCP. See `documentation/COMMAND_FACADE.md`.

### Fixed

- `twaco impact` no longer drops a caller that calls one member of a Thing and also passes the
  Thing along in the same service: the passed Thing concerns every member, so asking about another
  member still finds it. A template or DataShape that inherits itself is now reported as an
  inheritance cycle. A Graphviz graph (and the `dot` form of the MCP tool) from a partly unreadable
  repository now says so, as the text and JSON forms always did.

- A rename that is refused for the database now says why. An entity or prefix rename in a
  solution with DBConnection tables said "a GetDBInfo it could not read completely" even when every
  `GetDBInfo` was read; it now says the tables' rows store entity names.
- A server error that echoes the request back (a reflecting proxy, a verbose error page) no longer
  makes twaco print the credentials: the password, the app key and the Basic token, as given,
  JSON-escaped and percent-encoded, are replaced by `<redacted>` in error text. Successful
  responses are returned unchanged.

### Deprecated

- `--force` on `entity delete`, and `force` for the `entity_delete` tool; use the relevant
  acknowledgement instead.

## [0.1.0] - 2026-10-03

The first public release.

### The repository

- `init` proposes a `twaco.toml` from the repository's entities and can write it; `projects`
  reports project roots and deploy order, and `doctor` checks resolved configuration, server
  reachability and missing requirements.
- `extract` and `sync` move service scripts, DataShape fields, mashup content and DataTable
  configuration between entity XML and sidecar files. Sync changes only the targeted byte spans,
  supports check-only and controlled add/remove modes, and can relayout script CDATA.
- `check` runs line-ending, sidecar, formatting, script-trap, code-order and project-validation
  gates plus declared hooks; optional live checking parses every script on the server and fails
  closed when it cannot run.
- `fmt` formats service scripts with the built-in TypeScript formatter or reports changes with
  `--check`.
- `types` generates editor declarations from repository entities and platform metadata, caches
  platform declarations, and can run TypeScript once across every service.
- `catalog` provides an offline, searchable catalog of services with their signatures,
  descriptions, origins and implementation status.
- `adopt` compares a Composer export with the repository after normalising platform-managed
  differences, reports the effective changes, and can apply them to selected entities.

### Refactoring

- `rename` plans and applies entity, prefix, DataShape field, service, parameter, configuration
  table and property renames across the applicable XML, sidecars, scripts, mashups and
  configuration. It checks the result in a scratch workspace, applies file changes atomically
  with rollback, records server follow-up in a typed ledger, and requires an explicit SQL or
  no-SQL choice when an entity, prefix or field rename touches DBConnection tables.
- `move` and `copy` relocate or duplicate services and properties among Things, Thing Templates
  and Thing Shapes, with inheritance conflict checks, caller reporting, optional renaming and an
  optional forwarding delegate for a moved service.
- `retemplate` changes a Thing's template, a template's base or implemented shapes and reports
  the effective members gained and lost throughout the inheritance tree; changes that orphan
  stored values or references require explicit acceptance.
- `new building-block` plans or creates standard, abstract and implementation building blocks as
  repository files, including their project, entry point, manager where applicable, groups,
  organization and `twaco.toml` project entry.

### The server

- `entity status`, `entity get` and `entity push` compare repository, server and recorded baseline
  state, fetch raw server XML, and plan or import one entity while refusing conflicts; forced
  overwrites save the server copy unless backups are disabled.
- `bundle` builds an ordered importable document offline and can exclude configured UI
  collections; `deploy` runs repository checks, bundles in project order, live-parses scripts,
  checks conflicts, imports, reads entities back, runs deploy services and records the baseline,
  with entity, project and backend-only scopes.
- `entity delete` plans dependency-guarded deletions, saves server XML in backup sets by default,
  deletes in dependency order and confirms absence; `entity restore` lists backup sets and plans
  or imports all or selected entities back, confirming each result.
- `entity carry` plans or copies run-time, design-time and visibility permissions from renamed
  entities to their replacements, maps principals through the rename ledger, reads changes back
  and records completed work.
- `datatable copy` plans or copies rows from a replaced DataTable, mapping fields by identical
  name, the rename ledger or explicit mappings, with controls for unmapped fields and populated
  targets.
- `db run`, `db query` and `db clean` execute SQL commands in a transaction by default, run
  read-only queries and clean up temporary Database Things; writes plan by default.
- `call`, `logs`, `logs level`, `settings` and `config-table` call services with optional captured
  logs, filter server logs, inspect or change logger levels, search subsystem settings without
  displaying PASSWORD values, and read, diff, back up or restore configuration tables.
- `repo`, `ext`, `export` and `import` manage file-repository content, inspect and change extension
  packages, export entities, collections, projects or source-control layouts, and plan or import
  XML, zip and source-control content while preserving property values and table rows by default.

### Release

- `package` creates offline importable bundles, source-control layout zips and extension
  packages for a project or the full solution.

### Knowledge and MCP

- `guide` lists, searches and reads the compiled workflow, verified platform quirks,
  service-code reference and solution-local documentation.
- `help` searches the ThingWorx Platform help for the server's version or another release and
  reads pages or selected sections as cached Markdown.
- `javadoc` searches ThingWorx Platform Java API classes and members and reads cached class,
  method, parameter, return and exception documentation.
- `mcp` serves 39 tools over stdio to MCP clients, with compact summaries, validated arguments
  and dry runs by default for tools that can write to a server.

[0.1.5]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.5
[0.1.4]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.4
[0.1.3]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.3
[0.1.2]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.2
[0.1.1]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.1
[0.1.0]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.0
