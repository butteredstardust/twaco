# Working on a ThingWorx solution with twaco

How to change a ThingWorx solution kept in source control with twaco, in the order the work
usually happens. `twaco mcp` serves the same work as MCP tools (`check`, `status`, `sync`,
`deploy`, `call`, `logs`, `guide` and so on); `tools/list` names them. A command that writes to
the server only plans unless given `--apply` (MCP: `dry_run: false`). The exception is `call`:
the CLI runs the service you name, so know what a service does before you call it. MCP `call`
without `dry_run` runs a service known to only read (a platform read the repository does not
define, or one `[call] read_only` in twaco.toml lists) and plans any other.

## Start here

- `twaco projects`: the solution's projects and their deploy order.
- `twaco doctor`: what resolved, whether the server answers, what is missing.
- `twaco check`: every offline gate, one exit code. Run it before and after a change.
- `twaco lint`: the script rules alone, each finding with its rule id and severity;
  `--explain <rule>` says why a rule exists, `--print-config` where its severity comes from.
- `twaco entity status --all`: which entities differ between the repository and the server.
- `twaco catalog`: every service, where it lives, its signature and what it is for.
  `twaco catalog <Entity>.<Service> --body` (or a service name only one entity declares) gives
  one service with its script, also when the entity inherits it; read this before grepping XML.
- `twaco impact <entity> [--member <name>]`: what changing an entity, or one service of it, would reach, with how sure each reference is; run it before a rename, a move or a delete. `--server` adds what the server says refers to it, including entities the repository does not have.
- `twaco trace <Entity>.<Service> [--param <name>]`: how each caller uses a service, where impact says who: per mashup, what runs the call, what sets each parameter (or unbound, with its default) and where each result field goes; per script, the call, the variable and what it reads; and what the service calls. Run it before changing a service's parameters or result.
- `twaco unused`: entities no entry point reaches (advisory); list anything used from outside the repository under `[unused] keep` in `twaco.toml`.
- `twaco docs [--out <file>]`: the solution written down (projects, inheritance, services, DataShapes, references to review); regenerate and diff to see what changed. `--out <file> --check` exits 1 when a committed document is missing or stale, for CI.
- `twaco test [--apply]`: the service cases of `tests/*.toml`, each called as test users (`[[test.user]]`, `twaco users seed`) and expecting a status such as 200, 401 or 404; plans until `--apply`, and needs `allow_test_users = true` in the profile.
- A project only on a server: declare it in `twaco.toml`, `twaco export project <P> --out p.xml`, `twaco adopt p.xml --apply`, then `twaco entity status --all --record`.
- `twaco guide --search <words>`: this knowledge, plus the project's own documents.
  `twaco help search <words>` searches the ThingWorx Platform help for the server's version.
  `twaco javadoc search <name>` searches the Java API of objects scripts call and Resource services;
  `twaco javadoc service 'Resources["X"].Y'` reads one service as a script names it.

Read the project's AGENTS.md before changing anything: it says what the project is for and
which of its parts are fragile.

## The repository layout

- **Entity XML is the source of truth.** Each entity is one file, under a folder named after
  its collection (`Things/`, `ThingShapes/`, `Mashups/` and so on), as Composer exports it.
- **Each service script is a sidecar file** beside its entity, under `src/`. Edit the script
  there, never inside the XML's CDATA. `twaco sync` writes sidecars back into the XML;
  `twaco extract` writes the XML's scripts out to sidecars.
- **A mashup's layout is a sidecar too.** It is `content.json` and its assets, written into
  `mashupContent` by `sync`.
- **`twaco.toml`** declares the projects, their folders and dependencies, the bundle's UI
  collections, checks and package metadata. Server profiles live in `.twaco/profiles/` and
  are never committed.

## Change a service

1. **Edit the sidecar script.** For a signature change (parameters, result, DataShape), edit the
   service definition in the entity XML too. To create a service, prefer `twaco thing service add
   <Entity> <Name> --param name:TYPE --returns TYPE --then '<code>' --apply`: it writes the
   exported signature and both sidecars correctly. `twaco thing service remove <Entity> <Name>
   --apply` removes an own service, its permissions and sidecars. A manual service folder can
   still be added or deleted with `twaco sync <entity> --allow-add-remove`; an empty `services/`
   directory means zero managed services until Git drops it, so the command is safer for removing
   the last one.
2. `twaco sync <entity> --check` shows what would change; `twaco sync <entity>` writes it.
3. **Run the gates:** `twaco check`. `twaco types --check` type-checks every service against the
   entities' declarations; `twaco check --live` also has the server parse each script.
4. **Deploy only what changed:** `twaco deploy --only <entity> --apply`. A plain `twaco deploy`
   is the plan: what it imports, and whether anyone changed the server since the last deploy.
5. **Call it, including the paths that should fail:**
   `twaco call <Thing> <Service> '<json>' --with-logs`. That prints the result, then what the
   call wrote to ScriptLog and ApplicationLog.

The gates passing is not evidence a service works. They check structure, never behaviour; most
real defects are declarations that drifted from the code. Call the service.

## Permissions

An import only adds permissions: it never removes a grant the server has, and never changes the
allow or deny of a principal the server already lists. A deploy that reads an entity back as
"not kept" says when only its permissions differ.

- `twaco permissions audit [--server] [--detail]`: each project's `permissions.toml` (roles,
  run-time rules, visibility) against the entity XML; exit 1 on any error. `--server` also
  reads the server: entity permissions, the helper's tables, `[[platform]]` grants and
  memberships (what DeployComponent does), and each role's organizational unit. A project's
  permission helper Thing, if any, is found on its own; the Solution Framework is not needed.
- `twaco permissions init [--from-helper] [--apply]`: drafts a project's `permissions.toml`
  from what it grants today (the entity XML, or the helper's tables); `apply` then changes nothing
  but what the draft's notes name.
- `twaco permissions push --platform [--apply]`: adds the `[[platform]]` grants and memberships
  the server lacks (what DeployComponent does); never removes anything.
- `twaco permissions apply [--apply]`: writes the policy into the entity XML (only blocks that
  differ). Then deploy, and `permissions push`: an import never removes a grant.
- `twaco permissions diff <entity>|--all`: each run-time, design-time and visibility set in the
  entity XML (and a shape's or template's instance sets) against the server's; exit 1 when any
  differs.
- `twaco permissions push <entity> --apply`: makes the server's sets exactly the repository's and
  reads them back. Without `--apply` it is the plan.

## Read what the server says

- `twaco logs ScriptLog --since 10m --level WARN`: newest first, local times.
  `--grep <text>` filters the message; `--user` and `--thread` match exactly.
- `twaco logs level ScriptLog`: the log's level and its subloggers. Changing a level is the
  whole server's change, so it is a plan unless `--apply`, and the undo is printed. Put it back
  afterwards.
- `twaco settings --search <words>`: a subsystem setting by name or description, read-only.

## Configuration tables and data

- **Back up a configuration table before testing anything that writes one:**
  `twaco config-table <Thing> <Table> --backup before.json`. Then use `--diff` against the
  repository, and `--restore before.json --apply` to put it back.
  `SetConfigurationTableRows` replaces a whole row; it does not patch it.
- **A DataTable's rows are never in its entity XML.** Deploying the entity does not seed or
  clear them.

## Take a designer's drop

To rename an entity or a whole building block, use `twaco rename entity <old> <new>` or
`twaco rename prefix <old> <new>` (MCP `rename`). It plans by default; `--apply` rewrites every
reference, moves the files and records the rename in `.twaco/renames.json`; `--text` also changes
docs, sql and localization files. ThingWorx has no rename, so the old entities stay on a server
until deleted, and persisted values, DataTable rows and memberships do not follow: read the
"not carried over" list it prints before you deploy.

A designer changes mashups in Composer and exports them. `twaco adopt <export.xml>` compares
the export with the repository and reports what really changed, ignoring Composer's noise.
`twaco adopt <export.xml> --apply` writes it in. Then `twaco check` and commit. A backend-only
deploy (`twaco deploy --backend-only`) leaves the designer's collections alone, so backend work
does not roll back their unexported changes.

## Release

- **`twaco package bundle --out <file>`:** one importable XML, from the repository, offline.
  It takes `--project P`, `--backend-only` or `--frontend-only`.
- **`twaco package extension --out <file.zip>`:** an extension package, per project
  or for the solution. `--editable` decides whether the entities stay editable once installed.
- **`twaco package source-control --out <file.zip>`:** the `<Project>/<Collection>/<Name>.xml`
  layout.
- **`twaco ext import <zip>`:** the server validates a package without installing it.
  `--apply` installs it.

## Never on a live server

- **Do not import entity by entity what the project imports as one bundle.** An entity can land
  in `PTCDefaultProject`, or before what it depends on.
- **Do not deploy over someone's server-side change.** `deploy` and `entity push` refuse when
  the server changed since the last baseline; `--force` is for when you have looked.
- **Do not change users, groups, subsystem settings, or another project's entities** unless
  asked.
- **Name throwaway test entities so they are easy to find** (`ZZ.<you>.*`), and delete them
  afterwards. DataShapes and Mashups have no delete service; see the quirks topic.
- **Never print or commit credentials.** Profiles and `.twaco/` stay out of git.

## Where knowledge lives

- **This topic and the platform's:** `quirks`, verified-live platform behaviours, and
  `service-code`, the service-writing reference, ship with twaco:
  `twaco guide <topic> --section <heading>`.
- **The project's own:** AGENTS.md, CLAUDE.md and `docs/`. `twaco guide` searches them too.
  Write what you learn there, not in a chat: the next agent starts from those files.
- **The ThingWorx Platform help:** `twaco help search` and `twaco help page`.
- **The ThingWorx Platform Java API:** `twaco javadoc search`, `twaco javadoc class` and
  `twaco javadoc service`.

## Updating entity metadata and menus

`twaco entity set Things/Acme.Manager --description "Loads the page"` plans an existing
entity's root description. Empty text clears it. Use `--apply` to write, or `--json` for the plan.
Menu icons use `twaco new menu Acme.Main_MN --item Home=Acme.Home_MU --icon Acme.HomeIcon`
or `twaco entity set Acme.Main_MN --icon Acme.HomeIcon`.

`twaco menu item set Acme.Main_MN Home --title Start --icon Acme.HomeIcon` changes only the
given fields and keeps item order. `--mashup Acme.Start_MU`, `--description "Open the start page"`
and `--default` change the destination, tooltip and default item. Add a sub-Menu with
`twaco menu item add Acme.Main_MN --submenu Acme.Sub_MN`; set an existing item with `--submenu`.
Sub-Menu items have an empty title; select them by their target Menu name on set/remove.
Menu item commands plan unless `--apply`.

`twaco mashup describe Acme.Home_MU --detail` includes properties differing from catalogue
defaults. Text values are abbreviated; `--json --detail` retains full values.
`twaco mashup new Acme.Copy_MU --from Acme.Home_MU --replace page-current=page-nested`
changes whole tokens in copied content string values; repeat `--replace` for other tokens.
The copy reports every replacement and refuses an old token that occurs nowhere.

## Nested menus as an accordion

`twaco menu nest Acme.Main_MN --group "Basics=cds:icon_home:Intro=Acme.Intro_MU,Setup=Acme.Setup_MU"
--group "Advanced=cds:icon_settings:Tuning=Acme.Tuning_MU"` writes one leaf Menu per group
(`Acme.Main_Basics_MN`: its menuLabel is the label, its Menu-level icon the icon, one item per
Mashup) and the root `Acme.Main_MN`, whose items link to the leaves in the given order.
The icon is a `cds:` name, a MediaEntity or empty. Rerunning with the same groups writes
nothing; a changed group rewrites only its own Menu (an item keeps its image and default
under the same title, and every Menu keeps its description). The root keeps only sub-Menu
links, so it refuses a root that holds Mashup items. Go deeper with `menu item add --submenu`.

`twaco mashup recipe nested-menu Acme.Page_MU <menubar> --menu Acme.Main_MN [--width 300]
[--no-auto-collapse]` sets the accordion properties on the menu bar, gives it Width and
MaxWidth, and fixes its column at that width when the menu bar is alone in it. It refuses a
Menu without sub-Menu items. Applied with `recipe collapsible` on the same column, the menu
keeps AlwaysOpen true so it keeps its labels.

`twaco mashup recipe navigate-on-click Acme.Page_MU <collection|grid> --field PageName
[--window replace|new|popup|modal-popup]` adds a navigation function, binds its TargetMashup
from the selected row's field of the widget's own data call, and runs its Navigate on the
collection's CellClicked or the grid's RowClicked. A widget that cannot select rows gets
single selection; the plan warns when AutoSelectFirstRow is on, which would navigate on load.

## Composer links without a server request

`twaco url Acme.Thing` prints the Composer General tab link using the selected
profile (`--profile <name>`). `--json` returns `{"url":"..."}`. `--tab services`
applies to Things, ThingTemplates and ThingShapes; `--tab design` to Mashups;
`--tab fields` to DataShapes. Every type supports the default `attributes` tab.

Composer routes checked live on 10.2: Thing (also Data Tables, Streams, Schedulers,
Value Streams and Timers), ThingTemplate, ThingShape, DataShape, Mashup (gadgets
are Mashups too), Menu, MediaEntity, ApplicationKey, MCPNamespace, QueueProvider,
Group, Role, PermissionGroup, AIAgent, Authenticator, DirectoryService,
LocalizationTable, Network, NotificationDefinition, Organization,
PersistenceProvider, Project, Resource, StateDefinition, StyleDefinition,
StyleTheme, Subsystem, ThingGroup and User. Model and data tag vocabularies use
`ModelTagVocabulary` and `DataTagVocabulary`. Composer has no details page for
ExtensionPackage, Log, PersistenceProviderPackage, QueueProviderPackage,
ScriptFunctionLibrary, ThingPackage or Widget, so `twaco url` refuses them.
Dashboard and NotificationContent routes are unverified.
