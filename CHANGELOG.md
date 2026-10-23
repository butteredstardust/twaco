# Changelog

All notable changes to twaco are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.29] - 2026-10-23

### Added

- `doctor --json` and `projects --json` return the same structured reports as their MCP tools;
  the projects report, and MCP `projects`, now list misfiled entity documents.

### Fixed

- `types` whose write fails after other generated files were already updated names those files
  (the first five) and says to run it again, instead of reporting only the failed write.
- The MCP server bounds what a client can make it hold: at most 32 queued calls (more are
  answered busy, `-32000`) and messages of at most 32 MiB (a longer one is answered `-32000` and
  skipped).
- Workspace writes now refuse symbolic links and Windows junctions below the solution root,
  preventing writes through `.twaco`, sidecar, project, repository, or localization roots.
- `types --check` runs its configured TypeScript compiler without twaco credentials, with a
  600-second deadline and a 16 MiB cap on each output stream; compiler output also redacts
  credential values before twaco returns a finding or error.

## [0.1.28] - 2026-10-22

### Added

- `twaco entity pull <entity>...|--all` (and MCP `entity_pull`) takes the server's version of an
  entity whose working file is unchanged since its baseline: the server's export in the file's own
  envelope, layout and line endings, its sidecars, and its baseline. A deploy whose read-back finds
  an entity changed by the server now names it.
- `new thing --template`, `new thing-template --base` and `--shape` fetch a platform template or
  shape the repository does not use yet (FileRepository, RemoteThing) into the platform cache.
- `twaco new project` writes a Project entity (and its `[[project]]` with `depends_on` when the
  solution does not declare it yet), so a solution's entities stop landing in `PTCDefaultProject`
  on import; `twaco new group` writes a Group without members.
- `config-table <thing>` without a table (MCP `config_table` action `list`) lists the Thing's
  configuration tables.

### Changed

- `repo mv` moves the copy under `filerepository/` with the server's file, and `repo rm` removes it
  when it is identical to what the server had (a differing copy is kept and named).
- `localization set --table <language>` creates that table's file on first use, as it does for
  Default, instead of asking for `localization new`.
- `adopt` reports an element only one side has as one difference, with its attributes, and no
  longer counts a stored property value's Timestamp and Quality, as `entity status` does not.
- A recipe whose binding is already in the mashup leaves it as it is instead of refusing.

### Fixed

- `deploy` refuses a project that is neither bundled nor present on the server, preventing its
  entities from silently landing in `PTCDefaultProject`.
- `deploy` says how many baselines it recorded instead of "baseline written once".
- `call` refuses arguments the service's signature in the repository rules out (an undeclared
  parameter, a value its base type cannot take, a required parameter left out), and a 406 from the
  server says to check them.
- The help shows the `--state` spec format of `new state` and `state set`, and the error for a
  malformed spec quotes it; `thing service add --then` and `entity push` show what they take.
- `logs` names the platform's logs when the server does not know the one asked for.
- `entity delete` of Projects deletes one whose `dependsOn` names another in the set first; the
  server refuses to delete a Project another still needs.
- `entity status` reports `converged`, not `both-changed`, when the working file and the server
  both changed since the baseline to the same content (as after `adopt --apply` of a server
  change); it is not drift and `--record` records it.

## [0.1.27] - 2026-10-22

### Added

- `init --from-server` in a folder that already has a `twaco.toml` adds the named projects, each in
  its own folder: `twaco.toml` is only appended to, and only the added projects' sidecars and
  baseline are written.
- `init --from-server` also pulls the localization tokens the new projects own; a failed pull is
  reported and left to `twaco localization pull`.
- MCP tool `init_from_server` does the same in the MCP server's root.

### Fixed

- `init --from-server A B --apply` no longer reads `--apply`, `--json` or `--profile` after the
  project names as more project names.
- `init --from-server` proposes `[[server_owned]] parts = ["members"]` for a Group or Organization
  whose user members the server's export leaves out, so it records in sync instead of never; and
  drops the deploy-order hint when the server gave the projects' `depends_on`.
- A new entity written by `init --from-server` or `adopt` keeps its export's CRLF line endings in
  the envelope too, instead of mixing LF lines in.

## [0.1.26] - 2026-10-21

### Added

- `init --from-server <project>...` starts a repository in an empty folder from projects that
  exist only on a server: one file per entity, sidecars, `twaco.toml`, the agent files and the
  baseline, planned unless `--apply`.
- `localization pull`, `set` and `remove` edit one table at a time in extension exports that hold
  several localization tables, preserving every byte outside the table being changed.

### Fixed

- A new entity whose script is split across CDATA sections (to hold a `]]>`) was written with the
  sections framed by new whitespace, which changed the script, so it never came out in sync.

## [0.1.25] - 2026-10-21

### Added

- `adopt` maps the literal arguments scripts pass to a service parameter the export retypes to
  values the new base type takes (`"42"` to `42` for a `LONG`), on apply, and names the ones it
  cannot map and a default the new type does not take.
- `deploy` checks `requirements.toml` against the server it deploys to, in its `requirements`
  gate: an unmet platform, extension or entity requirement stops it before any import.
- `doctor` warns when a `[[server_owned]]` rule names `members` but matches no Group or
  Organization, or property values but matches no Thing or ThingTemplate: such a part hides
  nothing.
- The `localization tokens` gate notes, without failing, each language table's untranslated
  solution tokens and the Default tokens no entity or sidecar script names. MCP `check` with
  `detail: true` gives each gate its `notes`.

### Changed

- A deploy the gates stop lists what each blocking gate found (CLI text and `--json`, and MCP
  `blocking_gates[].first`, all with `detail: true`), and its stage is `gates`, not
  `offline gates`: the live parse and requirements gates read the server, so `twaco check` alone
  could not show them.

### Fixed

- On Windows, a gate's finding for an unreadable file named only the drive letter (`C`) as its
  file; both gates now name it relative to the repository.
- A `[[check]]` hook with `needs_credentials` that printed a server URL carrying
  `user:password@` printed the password; twaco now hides the user and password, and the
  password alone, as written and decoded.
- `requirements check --profile <name>` without `--server` ignored the profile; it now refuses.
- A push refused for a missing baseline says how to settle it: look at the server's copy,
  bring its change in or overwrite it with `--force --apply`, and record the baseline once both
  sides agree.

## [0.1.24] - 2026-10-21

### Added

- `[[server_owned]]` declares members, permissions, configuration tables or cells, and property
  values that belong to each server. Status and guards omit them; push and deploy preserve the
  current Exporter values in memory, and adopt keeps the repository's copy. Changed ownership
  rules invalidate the affected baselines. Status detail and plans expose the parts over CLI
  and MCP; push and deploy accept `--json`, and doctor reports rules that match no entity.
  An encrypted owned value is compared out but never carried, since an import cannot carry it;
  `entity push` refuses such an entity without `--force`.

- `contract diff --against <git rev>` compares public contracts offline, classifies breaking
  and compatible changes, recognizes ledger renames and lists current repository callers.
  Exit 1 flags breaking changes for CI; MCP exposes the read-only `contract_diff` tool.

- `requirements init` drafts a reviewed `requirements.toml` for external entities, extensions,
  projects and the minimum platform version. `requirements check` gates the repository offline;
  `--server` also verifies the dependencies with read-only server requests. MCP exposes
  `requirements_init` and `requirements_check`, and `check` runs the offline requirements gate
  when the contract exists (advisory through `[gates] advisory`).

### Fixed

- `adopt` preserves repository bytes and set-member order for unchanged content, lays out real
  changes like the repository, and strips incoming export-only metadata.
- `entity status --record` reports the recorded count and post-record verdicts and exit status.
- Repository entity operands accept `Collection/Name`, including collection-scoped short names.

## [0.1.23] - 2026-10-19

## [0.1.22] - 2026-10-19

### Added

- A `mashup content` gate in `twaco check` (and so before `deploy`): the content rules of
  `twaco mashup validate`, such as a binding to a property its widget does not have, now fail the
  run. A solution adopting it can list it in `[gates] advisory` until its findings are fixed.

### Fixed

- `mashup validate` accepts `DataSource1` to `DataSource<NumberOfSeries>` on a chart with
  `MultipleDataSources`, one data source per series.
- The help of `bundle` says the document is always written (`--apply` changes nothing), and the
  help of `db query` says it imports a temporary `ZZ.Twaco.Sql.*` Thing and deletes it.

### Added

- `network show --server`, `network diff` and `network pull` inspect and reconcile live Network
  connections; pull splices only the connections that differ. Deploy plans say how many
  connections each Network import adds, moves and removes on the server.

- `twaco network list|show|add|move|remove|import` reads and edits repository Network hierarchies as
  byte-preserving XML splices. `network import` maps RFC 4180 asset CSV files and can create
  Things in the same transaction; MCP `network` and `network_write` expose the hierarchy and its
  plans. `new network` creates an empty Network entity and the `networks` check gate catches
  connection shapes the server refuses.

### Changed

- Entity hashes are version 7: an Organization's connections and organizational units, and the
  keys of a mashup's JSON, compare in any order, since a 10.1 server returns both reordered. An
  unchanged Organization or mashup no longer reads as changed on the server. Baselines recorded
  by an earlier twaco count as unrecorded: run `twaco entity status --all --record` once.

## [0.1.21] - 2026-10-17

### Added

- `twaco catalog <Entity>.<Service>` (MCP `catalog` with `entity: "Entity.Service"`) narrows the
  catalog to one service, including one the entity inherits; a bare service name works when only
  one entity declares it, and names the candidates when several do. `--body` (`body: true`) adds
  each service's script, from its sidecar or, never extracted, from the declaring entity's XML.
- The AGENTS.md that `twaco init --agents` writes lists the commands to use before grepping:
  `catalog <Entity>.<Service> --body`, `impact --member`, `trace`, `mashup describe`, `entity
  status` and `permissions diff`. An existing AGENTS.md is never replaced.
- `twaco appkey create <name> --user <user>` (MCP `appkey`) creates an application key on the
  server, always with an expiration (`--expires`, default 30 days; the server's own default is one
  day), and `appkey show <name>` reads an existing one. The key is printed only with `--show`, or
  written as a profile's `app_key` with `--save <profile>`. `appkey delete` is `entity delete
  ApplicationKeys/<name>`.
- `twaco call ... --as <test user> --lang <code>` (MCP `call` with `lang`) makes one call in
  another language (`de`, or a preference list `fr,de`). ThingWorx takes a call's language only
  from the user's stored preference, so the test user's preference is set for the call and put
  back after it, even when the call fails; `--lang` without `--as` is refused.
- `twaco thing service add <Entity> <Name>` (MCP `entity_edit` action `service_add`) creates a
  script service on a Thing, ThingTemplate or ThingShape: its definition from `--param
  name:TYPE[:DataShape]` and `--returns`, its script, and both sidecars, written as `sync` would.
  `thing service remove` (`service_remove`) removes one with its run-time permissions and sidecar
  folder, and refuses while something still calls it unless `--force`.

### Changed

- The value name of `--param` in clap's messages and shell completions is `name:TYPE[:...]` for
  `trace`, `mashup new` and `thing service add` alike (it was `name:TYPE[:DIRECTION]`); the help
  text still gives each command's own form.

### Fixed

- A number default in a DataShape's `fields.json` (and a property's `--default`) goes into the
  XML as the server stores it, Java's `Double.toString` for NUMBER (`3` -> `3.0`, `10000000` ->
  `1.0E7`, `0.0001` -> `1.0E-4`) and the plain integer for INTEGER and LONG (`07` -> `7`). Before,
  the deploy read-back reported such a DataShape "not kept", and property defaults of 10^7 and up
  or below 10^-3 were written in a form the server rewrote.
- Removing an entity's last service: deleting its sidecar folder used to leave the service in
  the XML without a word (`sync` treated the entity as unmanaged and `check` passed). An existing
  empty `services/` folder now manages zero services, so `sync --allow-add-remove` removes it and
  `check` reports it; `thing service remove` does it in one step.
- `sync` keeps the `accumulatedDataShape` of the DataTables and Streams on a DataShape in step
  with it, as the server does on import: a removed field is kept and marked deleted, a new one gets
  the next ordinal. Before, the repository's tables drifted from the server after every DataShape
  change (deploy read-back reported them "not kept"). A DataTable's `datatable.json` follows too,
  `sync <DataShape>` brings its tables along, and the `sidecars` gate of `check` reports a table
  that does not follow its DataShape (`accumulated-shape-stale`).
- `rename field` treats a DataTable's or Stream's accumulated shape as the server does, a delete
  and an add, instead of renaming the entry in place, which the server never kept; Streams were
  skipped before. The plan now says that values stored under the old field in DataTable and
  Stream rows are not carried over.

## [0.1.20] - 2026-10-17

### Changed

- The library's modules are nested by domain (`core::mashup::*`, `core::rename::*`,
  `core::entity::*`) and most of its items are now crate-private, so the `twaco` library API is
  smaller. The CLI and MCP server are unchanged.

### Fixed

- A failed MCP tool call no longer puts its `{error, code}` object in `structuredContent`, where
  it broke the tool's published `outputSchema`; the error stays in the text block.
- The warning for a profile file other users can read now shows on stderr, once per run, without
  `--log`; before, it reached only the logs.
- A server read that timed out is no longer sent a second time, which doubled the wait; only a
  read whose connection failed or dropped is retried.
- `TWACO_MCP_TOOLS` set to an empty value serves every tool group instead of refusing to start.
- The MCP server answers a tool call that fails with an internal error instead of leaving the
  client waiting, and a call that reuses a finished call's id can still be cancelled.
- `twaco completions zsh` escapes the colons in value names such as `name:TYPE[:DIRECTION]`,
  which broke completion of seven flags (`trace --param`, `adopt --take`, `mashup add --input`
  and others). A flag's description in the script ends where its help line moves on to other
  flags.
- The `completions` help and README no longer promise action words in every shell: clap_complete
  offers them in bash and zsh only. The 0.1.19 notes also said a missing action keeps its
  message; that holds only when nothing follows it: `mashup param M.MU Count` gets clap's
  refusal naming `M.MU`.

## [0.1.19] - 2026-10-17

### Added

- `twaco completions <shell>` writes a shell completion script (bash, elvish, fish, powershell or
  zsh) on stdout, built from the same command tree twaco parses with: every command, flag and
  action, described in the words `twaco` lists. It needs no solution and never prints the update
  notice.

### Changed

- The command-line parser is now typed clap derive structs, one beside each command's handler,
  in place of the hand-written flag table. No change to commands, flags or messages.
- clap now refuses a bad `--timeout` (`call`, `db run`, `db query`) or an `--only-projects` that
  names no project (`deploy`) before the command runs, in its own words: `twaco: invalid value '0'
  for '--timeout <seconds>': needs a positive whole number of seconds` (was `twaco: --timeout needs
  a positive whole number of seconds`) and `twaco: invalid value ' , ' for '--only-projects <a,b>':
  needs at least one project` (was `twaco: --only-projects needs at least one project`), each
  followed by `For more information, try '--help'.`; exit 2 as before.
- The sub-action of `users` (`list|seed|delete`), `help` (`search|page`), `javadoc`
  (`search|class`) and `mashup param` (`add|set|remove`) is checked by clap before the command
  runs, so a word it does not take is refused with the words it does, for example
  `twaco: invalid value 'frob' for '[action]'` and `[possible values: list, seed, delete]`. This
  replaces `users takes: list | seed | delete` (which came only after the solution was found),
  ``help has `search` and `page`, not "frob"``, ``javadoc has `search` and `class`, not "frob"`` and
  `action must be add, set or remove`. `users` given anything after its action now gets clap's
  `unexpected argument` refusal. A missing action keeps its message.

## [0.1.18] - 2026-10-17

## [0.1.17] - 2026-10-17

## [0.1.16] - 2026-10-17

## [0.1.15] - 2026-10-17

### Fixed

- `datatable copy` refuses a row count the server sends negative, fractional or out of range,
  instead of reading it as 0. Close-match ranking in `javadoc` no longer wraps for names more
  than 255 edits apart.

## [0.1.14] - 2026-10-17

### Security

- `zip` 9.0.4 (from 8.6.0) for RUSTSEC-2026-0368 and RUSTSEC-2026-0369. twaco never calls the
  affected `extract`, so it was not exposed, but the advisories fail the dependency check.

### Changed

- MCP `entity_new` lists its kinds in its schema, so a client can offer them and a wrong one is
  refused before the tool runs.
- Parallel server reads hand each worker the next entity as it frees up, so one slow request no
  longer holds back a fixed share of the others.
- A profile file other users can read gets a warning on Unix, as ssh gives; passwords and app
  keys are zeroed in memory once used.
- Every MCP tool, all 73, now publishes an `outputSchema` describing its successful result (it
  was 9); clients on the `2025-06-18` revision or later see them.

## [0.1.13] - 2026-10-16

### Fixed

- `check` and `fmt` no longer skip a file or folder they cannot read: the gate fails and names it.
  Script gates walk the scripts once even through a linked folder, and find a service named like
  a build folder (`build`, `target`, ...). Every walk now honours `.gitignore` the same way and
  never follows links.
- A write to a ThingWorx server is never retried; a read whose connection drops is tried once
  more, after a pause. Connecting gives up after 10 seconds instead of waiting for the whole call.
- A replaced file's folder is synced after the rename on Unix, so a crash cannot lose the rename.
- Every MCP tool failure carries a code: `javadoc` failures used to come back `unclassified`; a
  failed fetch is now `server_unreachable` or `io_error`, an unknown member `invalid_arguments`.

## [0.1.12] - 2026-10-16

### Added

- `twaco mcp --tools <groups>` (or `TWACO_MCP_TOOLS`) serves only the named tool groups: `core`,
  `entity`, `refactor`, `mashup`, `permissions`, `data`, `server` and `reference`. All 73 tool
  definitions come to about 89 KB of an agent's context; `core` is about 25 KB. The default is
  still every tool.
- The MCP server answers `ping` and `tools/list` while a tool call runs, and honours
  `notifications/cancelled`: a queued call that is cancelled never runs, and a running one sends
  no further progress and no response. Calls still run one at a time, in order.

## [0.1.11] - 2026-10-16

### Changed

- `twaco.toml` refuses a key or table it does not know, naming it and the keys it expects.
  Before, a misspelled `[gatez]` or `depends_onn` was ignored, so `check` could run fewer gates or a
  deploy go out of order without a word. Free-form tables such as `deploy_parameters` are unchanged.
- MCP tool annotations are set per tool. `openWorldHint` is true for the tools that can reach a
  ThingWorx server or the Platform help; `destructiveHint` is false for tools that only add or
  regenerate (`fmt`, `types`, `status`, `bundle`, the `new` tools, ...); writing tools now carry
  `idempotentHint`. Read-only tools no longer carry `destructiveHint`.

## [0.1.10] - 2026-10-16

### Added

- `twaco trace <Entity>.<Service>` (MCP `trace`): how each caller uses a service. For each mashup,
  what runs the call, what sets each parameter (or that it is unbound, and the default it gets) and
  where each result field goes; for each script, the call, the variable and what it reads of the
  result; and what the service itself calls.
- `twaco adopt` lists base-type changes (a mashup parameter retyped from LONG to STRING, say) under
  their own heading, and as `type_changes` in `--json` and MCP `adopt_report`; before, they were one
  node in an entity's count unless `--detail` was given.
- A `result shapes` check gate: a service whose script builds its result from another DataShape
  than its definition declares. In 391 real services built this way it found 10, often a
  same-named DataShape of another project.
- `twaco call ... --as <test user>` (MCP `call` with `as`) makes one call as a declared
  `[[test.user]]`, with the profile's `test_password`, on a profile that allows test users.
- `twaco thing property|subscription|shape`, `timer set`, `scheduler set`, `style set`, `media set`,
  `menu item` and `state set` (MCP `entity_edit`) change existing entities as Composer would, plan
  until applied, and keep the stored values of persistent properties in every inheriting Thing.
- `twaco new thing|thing-template|thing-shape|data-shape|timer|scheduler|data-table|stream|value-stream|media|style|state|menu|theme` (MCP `entity_new`)
  plans or creates common ThingWorx 10.1 entity exports in the repository. DataShapes get their
  `fields.json` sidecar and DataTables get `datatable.json`; timers and schedulers require a run-as
  user and can carry an event script.

- `twaco mashup add-contained` (MCP `mashup_add_contained`) adds a repository child mashup as a
  Composer-compatible `mashupcontainer`, including its parameter cache and static input values.
  Bindings and `mashup describe` now treat visible child parameters as the container's properties;
  a binding repairs a stale container cache in the same edit.

- `twaco mashup param add|set|remove` (MCP `mashup_param`) keeps mashup parameter definitions,
  content metadata, binding base types and every containing mashupcontainer cache in step, planning
  until applied in one transaction. `mashup new --param name:TYPE[:DIRECTION]` creates scalar
  parameters with a new mashup, and `mashup describe` reports container cache health.
- `twaco mashup templates` (MCP `mashup_templates`) lists the 14 embedded responsive mashup templates and their slots. `mashup new --template <name>` creates any one of them, accepts aliases, reports its slots, places master pages in the template's master slot, and finds menu bars from the generated document; `--layout` remains a compatibility alias.
- `twaco mashup on <mashup> --event <address> --invoke <address>` (MCP `mashup_on`) makes a widget event (from its catalogue contract), the page's `mashup:Loaded` or a call's `ServiceInvokeCompleted` run a data service or a widget service, adding a Thing service call when needed. It plans until applied and updates the entity plus managed content sidecar atomically.
- `twaco mashup bind <mashup> --from <address> --to <address>` (MCP `mashup_bind`) adds one checked mashup data binding, creating a Thing service call when needed. It plans until applied and updates the entity plus managed content sidecar atomically.
- `twaco mashup add <mashup> <widget>` (MCP `mashup_add`) adds a catalogue widget to an existing responsive mashup, in a container or beside another widget, with simple label and property settings. It plans until applied and updates the entity plus managed content sidecar atomically.
- `twaco widgets [<widget>] [--search <text>]` (MCP `widgets`) reads the embedded ThingWorx 10.2 widget catalogue without a solution or server: its types, Composer labels, aliases, containers and binding contracts; detail gives bindable properties with directions, events and services. `--json` returns the same data.
- `twaco mashup describe <mashup>` (MCP `mashup_describe`) shows what a mashup holds and how it is wired, without its XML: the widget tree (row/column containers, which widget fills its container, text and `[[tokens]]`), its parameters, each service it calls with what sets each declared parameter (a binding, `static`, or `unbound`) and what runs it, each binding (one row per property map, with a warning when a binding has more than one, since the runtime applies only the first) and each event. `--json` for the same as data.
- `twaco mashup validate <mashup>|--all` (MCP `mashup_validate`) reports what ThingWorx would load but not honour: bindings and events to widgets, sources or calls that are gone, bindings against a widget's contract, several property maps, contained-mashup drift. On two real solutions it found 110 orphaned bindings, a misspelt expression input and three stale popup caches, and nothing that was not a problem.
- `mashup describe` says what each function widget does: what an expression computes and feeds, what a validator checks and runs, which mashup a navigation function opens and passes, what a status message shows and when.
- `twaco mashup recipe <recipe> <mashup> ...` (MCP `mashup_recipe`) writes the wiring real mashups repeat in one edit: enable-when, show-when, text, message, confirm, popup, close-after, auto-refresh and validate, and chart, columns and picker, which configure a chart, a grid or a dropdown from a service result and check every field against its DataShape.
- `twaco mashup event add <mashup> <name> --when <event>` (MCP `mashup_event`) gives a mashup an event its parents can react to: a NUMBER parameter set to the time when the event fires, the way real mashups signal since ThingWorx has no contained-mashup events. `mashup on --event ui:<container>.<name>` in a parent reacts through an events router.
- The widget catalogue knows the function widgets ThingWorx 10.2 offers: expressions, validators, events routers, navigation functions, status messages and auto-refresh, with their events and services. `mashup add` puts one beside the layout (`--input name:TYPE` declares an expression's or validator's inputs), and `mashup bind` / `mashup on` wire their inputs, outputs, events and services.
- `twaco check` has a `contained mashups` gate: a binding to a parameter that a contained mashup's or navigation function's copy of the parameters lacks, or that the mashup it shows no longer declares. ThingWorx drops those values without a word; three real popups in a shipped product had them. `mashup param set <mashup> <parameter>` with no other change refreshes every copy and leaves the bindings alone.
- `mashup new --template tabs --tab "Label=Mashup"...` (MCP `mashup_new` with `template: "tabs"` and `tabs`) makes a page whose tab set shows a mashup in each tab, each in the tab container Composer gives it with the mashup's parameters cached; two more templates: `sidebar-header` (the stock ThingWorx master shape) and `dashboard`.
- `twaco mashup new <name>` (MCP `mashup_new`) creates a ThingWorx 10.2 responsive mashup in the repository as Composer would: the entity file from the 10.2 template and its `content.json` / `custom.css` sidecars, written by the same sync as any mashup, in one transaction. `--layout sidebar` binds required `--primary-menu` and optional `--secondary-menu` INFOTABLE services to its menu bar and loads them with the page; `--layout header-footer` creates fixed header and footer regions. `--master` makes a master mashup with a page container filling the center; `--theme` takes a StyleTheme in the solution (default PTC Convergence Theme); `--visibility Organization/<name>` or `OrganizationalUnit/<org>:<unit>` grants visibility, and nothing is granted unless asked. Plans until `--apply`.

### Changed

- Entity hashes are version 6: a Thing's stored values (`ThingProperties`) compare in any order and
  without their timestamp and quality, because a 10.1 export reorders them and an import re-stamps a
  value whose property changed. Baselines from v5 count as unrecorded until the next deploy
  or `entity status --all --record`.

### Fixed

- `twaco deploy` and `entity status` handle a MediaEntity the server does not have yet: the
  export route answers it with an empty collection and HTTP 200, which read as malformed XML and
  stopped the deploy. It is now absent, like any other missing entity.
- A flag with an optional value, given bare, no longer takes the next flag as its value
  (`--default --label Main`); a negative value still works.
- `twaco entity delete` no longer fails on a disabled Thing, whose dependency service answers
  HTTP 503: its dependents are reported as unknown, refused under `outside_dependents` until
  acknowledged, and warned about when they are.
- `twaco impact <entity> --member <service>` narrows mashups to those that bind that service. An exported mashup lists a data source's services as `Services: [{Name, Target}]`, which the index did not read, so every mashup binding any service of the entity was reported as a dependent of each of them.

## [0.1.9] - 2026-10-15

### Added

- `twaco reproject <entity>... --to <project>` (MCP `reproject`) moves entities to another project: their `projectName`, and their files under that project's root, in one transaction. The plan names each reference the move puts outside the deploy order, with the `depends_on` that allows it.

- The user guide shows how a project that is only on a server becomes a repository: `export project`, `adopt --apply`, `entity status --all --record`.

- `twaco test [<filter>...]` (MCP `test`) runs the service cases of `tests/*.toml` (`[test] cases` names another folder): each case calls a service as each of its test users or groups and expects a status (200, 401, 404, ...) and optionally a row count and first-row fields. Without `--apply` it reads, checks and lists the cases; with it, it calls them and exits 1 when one fails. Running needs `allow_test_users = true` in the profile.
- `twaco users list|seed|delete` (MCP `users`, `users_write`) manage test users: the accounts `[[test.user]]` declares, each with its groups, all with the profile's `test_password`. `list` shows whether each exists, signs in and is in its groups; `seed` creates what is missing (users, memberships, an optional `[test] project`) and changes nothing that exists; `delete` removes the users. Both writes plan until `--apply` and need `allow_test_users = true` in the profile.

- `twaco check` has a `localization tokens` gate: a mashup, menu or other entity that names a `[[token]]` under one of the solution's token prefixes that no Default table file holds fails it, since the label would show the raw `[[name]]`. A language table token Default lacks, a token in one table twice, and an unreadable table file fail it too. Tokens outside the solution's prefixes (the platform's, an extension's) are counted as not checked. `[gates] advisory = ["localization tokens"]` makes it report without failing.

- `twaco docs --out <file> --check` writes nothing and exits 1 when the file is missing or no longer matches what `twaco docs` would write with the same `--detail` and `--json`, so CI can fail a pull request whose committed solution document is stale. A CRLF checkout of an up-to-date file is up to date.

### Changed

- `twaco deploy` of the whole solution carries the localization tables: it plans them before anything is imported, stopping on a table the server would refuse, and after the entities are imported it imports each table that differs from the server, Default first, and reads it back. It never prunes. A deploy of some projects or entities, or `--backend-only`, leaves them to `twaco localization push`. The MCP `deploy` result has a `localization` list, and a failure there is `stage: "localization"` with `entities_deployed`.

- `twaco rename prefix` renames the tokens in the localization tables (`[localization] root`) together with the `[[token]]` references to them, without `--text`: `rename prefix Acme.App Acme.Platform` turns `Acme.App.Save` into `Acme.Platform.Save` in every table file's name cells and every entity. Values, comments and XML files that are not tables stay other text (`--text`). An entity rename leaves the tables alone, as it leaves a dotted `[[token]]` alone. Before, the references changed and the tables kept the old names unless `--text` was given. The plan has a `localization` row and the JSON a `localization` count.

- A localization file holding several tables, as an extension ships them, is read table by table instead of being unreadable: `localization status`, `push` and the `localization tokens` gate use it, and `push` imports each of its tables alone. `pull`, `set` and `remove` refuse to edit such a file and say to split it.

- `repo pull --apply` downloads every file before it writes any, then writes them all as one journaled transaction: a download that fails midway, a failed write or a killed process no longer leaves some files copied. A file that appears or changes locally while the pull runs is refused, not overwritten. A pull needs the repository folder (`[repositories] root`) to be inside the solution.
- `localization pull --apply` and `localization remove --apply` write every table file in one journaled transaction, or none, and refuse a file edited since they read it. `localization new` and `set` write through the same journal. Both refuse a `[localization] root` outside the solution.

### Security

- rustls 0.23.46: fixes RUSTSEC-2026-0346 to 0349 in the TLS client every server command uses.

## [0.1.8] - 2026-10-14

### Added

- `twaco check` has a `mashup bindings` gate: a mashup that binds a field its service's result DataShape does not declare, passes a parameter the service does not have, or calls a service its entity does not have fails it. ThingWorx imports all three and the widget quietly draws without the value. What twaco cannot see (a DataShape or entity outside the repository, a platform service before `twaco types --platform` has cached them) is counted as not checked rather than guessed. `[gates] advisory = ["mashup bindings"]` makes it report without failing.
- `twaco unused --fields` (and `fields` on the `unused` MCP tool) lists, for each service result mashups bind field by field, the fields no mashup binds or names.

### Changed

- `adopt --apply` without a base no longer writes a mashup or media entity it can only call unknown: it is a conflict, as everywhere else. `--take theirs:ui` takes every UI conflict and unknown at once.
- `adopt --base <git-rev>` reads the revision with one git process instead of one per file: about ten times faster on a solution of 700 files.

### Fixed

- `adopt --apply` no longer overwrites a service or mashup sidecar edited since the last `twaco sync` when the collaborator changed the same entity.
- `adopt --take ours:<Entity>` keeps the entity whole; it used to be accepted and ignored.
- `adopt --apply` lists a theme, menu or other UI entity it does not write as `not adopted`, instead of saying nothing.
- `adopt --apply` keeps the CRLF line endings of a service sidecar it changes.
- `adopt` without a base recognises a committed script that lives only in the entity XML as stale, not unknown.
- `adopt --apply` refuses a backend entity or service name that cannot be a file name, naming it.
- `adopt --base <git-rev>` reads files whose names git quotes (non-ASCII), skips submodules, and refuses a revision that starts with `-`.
- `handoff record` refuses two files with the same name, and an interrupted recording no longer breaks every later `adopt`.

## [0.1.7] - 2026-10-09

### Added

- `twaco localization` and the MCP `localization` and `localization_write` tools compare, synchronize and edit localization table files.
- Three-way `adopt` collaboration, recorded handoffs, and `--base`, `--only` and `--take` support designer and backend handoffs.

### Changed

- `adopt --fail-on-revert` now counts stale, conflicting and unknown services only, and `adopt --apply` writes safe backend changes as well as UI changes.

### Fixed

- `twaco import` of a localization table that already exists no longer reports a failure: the Importer answers partial-success although it took the tokens.
- A designer's stale mashup no longer overwrites a newer repository mashup when an adopt base is known.

## [0.1.6] - 2026-10-08

### Added

- Progress for long commands. `deploy` (also its live parse gate), `entity status`, `entity push`, `check --live`, `permissions`
  (also `permissions push --platform`), `repo`, `types`, `import`, `export` and `ext import`
  draw a progress bar on stderr when stderr is a terminal. With a pipe, a file or a CI run, twaco
  draws nothing, and stdout does not change. Logs on stderr switch the bars off.
- Colour in terminal output. `check`, `entity status`, deploy results and the `twaco:` error prefix
  use colour. A non-empty `NO_COLOR` turns it off and wins over `FORCE_COLOR`. `FORCE_COLOR` turns it on. Piped output stays plain and
  byte-identical.
- MCP progress notifications. A `tools/call` request with `_meta.progressToken` gets
  `notifications/progress` lines before its response, for `deploy`, `status`, `types`, `import`,
  `export`, `extension_write`, `check`, `push`, `permissions`, `permissions_audit`,
  `permissions_apply`, `permissions_push`, `repo` and `repo_write`. Twaco sends at most about ten per second.
- Diagnostic logs for every command, including `twaco mcp`. `--log <filter>` or `TWACO_LOG`
  writes them to stderr. `--log-file <path>` or `TWACO_LOG_FILE` appends them to a file.
  Logs are off by default. They never hold credentials, headers, bodies, subprocess argument
  values or string MCP message IDs. A log file that is standard output is ignored with a warning.
- Logs show workspace lock actions, transaction stages, recovery decisions, subprocesses
  (`[[check]]` hooks, the TypeScript compiler, `git`) and each phase of a deploy, rename or adopt.
  A stale temporary that twaco removes and an interrupted operation that twaco recovers log a warning.

### Fixed

- A server error message no longer shows a credential that appears in the request URL. This
  happens when an entity name holds a secret. Every client error now hides the password, the app
  key and secret profile values in its URL, reason and body.
- A server error body no longer shows part of a secret that crosses the 4096-byte cut. Secrets
  are now removed before the cut.
- Progress bars stay off when `--log-file` names stderr, for example `/dev/stderr`.

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

[0.1.29]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.29
[0.1.28]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.28
[0.1.27]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.27
[0.1.26]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.26
[0.1.25]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.25
[0.1.24]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.24
[0.1.23]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.23
[0.1.22]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.22
[0.1.21]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.21
[0.1.20]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.20
[0.1.19]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.19
[0.1.18]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.18
[0.1.17]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.17
[0.1.16]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.16
[0.1.15]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.15
[0.1.14]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.14
[0.1.13]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.13
[0.1.12]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.12
[0.1.11]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.11
[0.1.10]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.10
[0.1.9]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.9
[0.1.8]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.8
[0.1.7]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.7
[0.1.6]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.6
[0.1.5]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.5
[0.1.4]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.4
[0.1.3]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.3
[0.1.2]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.2
[0.1.1]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.1
[0.1.0]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.0
