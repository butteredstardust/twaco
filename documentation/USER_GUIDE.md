# User guide

How twaco sees a solution, and how to do each part of the work with it. [Quick
start](QUICK_START.md) gets a repository set up; [Commands](COMMANDS.md) lists every flag.

## The repository

### Entity XML is the source of truth

Each entity is one XML file, exactly as ThingWorx exports it, in a folder named after its
collection: `Things/`, `ThingTemplates/`, `ThingShapes/`, `DataShapes/`, `Mashups/` and so on.
twaco never re-serialises these files. It finds the byte range it needs and replaces only
that, so a sync that changes one script changes those bytes and nothing else: not attribute
order, not whitespace, not CDATA boundaries.

An entity belongs to the project its own `projectName` names. A file filed under another
project's folder is reported by `check`, because it would otherwise import into the wrong
project silently.

### Starting from a project that is only on a server

A project built in Composer becomes a repository with three commands. Declare the project, take
the server's export of it in, and record that the files are what the server holds:

```sh
printf '[[project]]
name = "Acme.App"
' > twaco.toml   # one [[project]] per project, with depends_on
twaco export project Acme.App --out acme-app.xml
twaco adopt acme-app.xml --apply           # one file per entity, service sidecars extracted
twaco entity status --all --record         # the baseline: these files are what the server has
twaco init --agents                        # AGENTS.md, CLAUDE.md and the .gitignore lines
twaco check                                # a script Composer formatted fails `formatting`: twaco fmt
```

`adopt` writes each entity the export holds and the solution lacks, in the solution's layout, with
its sidecars; nothing that exists is overwritten. Repeat the export and adopt for each project the
solution keeps. Delete the export file afterwards: the entity files are the source now.

### Sidecars are what you edit

`twaco extract` writes the parts worth editing out of the XML into files under `src/`
(`[solution] src`):

| Sidecar | From | Path |
| --- | --- | --- |
| A service's code | a Thing, ThingTemplate or ThingShape | `src/<Entity>/services/<Service>/script.js` |
| A service's signature | the same | `src/<Entity>/services/<Service>/definition.xml` |
| A DataShape's fields | a DataShape | `src/<DataShape>/fields.json` |
| A mashup's layout and CSS | a Mashup | `src/<Mashup>/mashup/content.json`, `custom.css` |
| A DataTable's configuration | a DataTable Thing | `src/<DataTable>/datatable.json` |

`twaco sync` writes them back into the XML. Commit the XML and its sidecars together; the
`sidecars` gate fails when they disagree.

A service or a DataShape field appearing or disappearing is a bigger change than an edit, so
`sync` refuses it unless given `--allow-add-remove`. With it, a new service folder holding a
`definition.xml` and a `script.js` adds the service, its Script implementation laid out like
the entity's others, and a deleted folder removes the service together with the entity's run-time
permissions for it. The folder's name and the `name` in its `definition.xml` must agree. A
sidecar naming a SQL service, or a service the entity only overrides, is refused: those are not
added from sidecars.

An existing but empty `services/` directory means that the entity deliberately manages zero
services: `sync --allow-add-remove` removes its last script service, and without the flag reports
the removal. An entity with no `services/` directory remains unmanaged. Git does not retain empty
directories, so use `thing service remove` for a durable last-service removal.

## The change loop

```sh
twaco sync <Entity>                         # sidecars into the XML
twaco check                                 # every offline gate
twaco deploy --only <Entity>                # the plan
twaco deploy --only <Entity> --apply        # the import
twaco call <Thing> <Service> '<json>' --with-logs
```

Then commit. `deploy --apply` and `entity push --apply` record what they sent in
`.twaco/baseline.json`; commit that too.

### The gates

`twaco check` runs every gate and exits 0 when no blocking gate fails; a `[[check]]` hook
with `gate = false` reports its findings without failing the run. `--detail` lists every finding;
without it, each gate gives a summary.

| Gate | Fails when |
| --- | --- |
| line endings | a file mixes CRLF and LF, which is a tool's half-rewrite. Files the solution's own `.gitignore` files match are skipped (patterns only: a tracked file that matches one is skipped too), and so is anything a `.ignore` file lists, for tracked files kept verbatim such as raw exports. Ignore files above the solution and global excludes do not count |
| sidecars | a sidecar and its XML disagree, a service sidecar lacks its `definition.xml` or `script.js`, a fields or DataTable sidecar cannot be read, or a sidecar directory has no entity; an entity without sidecars is simply unmanaged |
| formatting | a script is not as the built-in formatter writes it (`twaco fmt` fixes it) |
| script traps | a script uses something the ThingWorx Rhino engine punishes (see below) |
| code order | a service's main code is below its helper functions |
| project | the entity files disagree with themselves or each other: a file named for another entity, two files for one entity, a misfiled or project-less entity, a service declared without its implementation or the reverse, a mashup whose JSON does not parse |
| mashup bindings | a repository-resolved mashup service, result field or parameter is not declared; unresolved platform and inherited DataShape details are left unchecked |
| mashup content | what `twaco mashup validate` finds in a mashup's content: a binding to a property, event or service its widget or data source does not have, a binding in the wrong direction, mixed container layouts. Widgets outside the catalogue (extensions) are not judged |
| contained mashups | a binding to a contained mashup's or navigation function's parameter that its copy of the parameters lacks (the value is dropped) or that the mashup it shows no longer declares; `twaco mashup param set <mashup> <parameter>` refreshes every copy; a stale copy nothing binds is only counted |
| localization tokens | an entity names a `[[token]]` under one of the solution's token prefixes that no Default table file holds (the label would show the raw text), a language table holds a token Default lacks, a token is in one table twice, or a table file cannot be read. Tokens outside the prefixes are the platform's or an extension's and are not checked |
| result shapes | a service's script builds its result with `CreateInfoTableFromDataShape` from another DataShape than its definition declares, named by a literal or a `const` holding one (a mashup bound to the declared shape finds other fields). A name computed at run time is not checked |
| your hooks | a `[[check]]` command fails ([Configuration](CONFIGURATION.md#check-your-own-gates)) |
| live parse | with `--live` or `[gates] live`: the server's own parser rejects a script |

The script traps are runtime failures that look like valid JavaScript:

- **`for-in`:** `for (x in y)` is a syntax error in the script engine.
- **`const-in-loop`, `const-redeclared`:** `const` is function-scoped there, so two blocks
  collide, and the caller sees "No service handler defined".
- **`indexed-rows`:** indexing `rows.toArray()` re-reads the first row.
- **`unbounded-loop`:** a `while` loop that can run forever pins a platform thread.
- **`nan-risk`:** a NaN in a result row makes the whole service fail to serialise.
- **`read-before-assign`:** a variable read before it is assigned.

`twaco unused --fields` is a separate advisory report: for each service result that mashups bind
field by field, the fields no mashup binds or names. A result some mashup binds whole (a grid
showing every column) is not judged. Only mashups are read, so a field a script, another service
or a REST client reads looks unused: review before removing one.

`twaco types --check` type-checks every service against declarations generated from the
entities, the DataShapes and the server's platform metadata. A typo in a property name or a
wrong parameter is caught before a deploy; a wrong one is suppressed with `// @ts-ignore` on
the line above.

### Status, the baseline and conflicts

`twaco entity status --all` compares each entity three ways: the repository, the server, and
the baseline. The baseline holds two hashes per entity, the repository's and the server's,
recorded at the last deploy or push (or by `--record`, below); each side is compared with its
own.

| State | Meaning |
| --- | --- |
| in-sync | neither side changed since the baseline (the two need not be identical: the server may keep an entity slightly differently) |
| local-changed | you changed it since the baseline |
| server-changed | someone changed it on the server since the baseline |
| both-changed | both did: deploying would lose their change |
| not-on-server | the server does not have it: never deployed, or deleted there |
| no-baseline-same | no baseline yet, and the repository and the server agree |
| no-baseline-differs | no baseline yet, and they differ: twaco cannot tell whose change it is |

The comparison ignores what ThingWorx changes on its own: change history, owners, timestamps
and the like. `entity status --record` records the current state as the baseline when the
two sides already agree, for a repository adopting twaco.

`deploy` and `entity push` refuse an entity whose state is server-changed or both-changed,
one that differs from the server with no baseline (the common ancestor is unknown), and one
the baseline knows but the server no longer has (deleted there).
Look at the server's version first (`twaco entity get <Entity>`), bring its change into the
repository if it matters, and then deploy, or `--force` to overwrite it on purpose.

### Deploys

`twaco deploy` imports the solution, or the part `--only <entity>` and `--only-projects`
name. Without `--apply` it plans and stops. With it, it:

1. runs the offline gates, unless `--skip-checks`;
2. builds one bundle per project, projects in `depends_on` order, collections in the order
   the importer needs;
3. plans the localization tables, as `twaco localization push` does, and stops on a table the
   server would refuse (a deploy of the whole solution only);
4. sends every script to the server's parser, and stops on any failure;
5. checks every entity for a conflict with the baseline;
6. imports each bundle, reads each entity back, and fails if the server did not keep it;
7. calls the project's deploy and post-import services, if `[project.deploy]` names them;
8. records the new baseline;
9. imports each localization table that differs from the server, Default first, and reads it
   back. It never prunes: `twaco localization push --prune` deletes server tokens.

With `--only`, the project's deploy service still runs, but its `post_import` calls are
skipped: they are written for a whole project's import.

The localization tables are shared by every project, so a deploy of some projects or entities
(`--only`, `--only-projects`) or of the backend only leaves them to `twaco localization push`.

`--backend-only` leaves out `[bundle] ui_collections`, so backend work does not overwrite a
designer's mashups. `twaco bundle` builds the document without sending it.

`twaco entity push <Entity>` imports a single entity, with the same conflict check.

## Taking in a designer's or another backend developer's work

Give a collaborator the exact package they start from, then keep that package as the comparison
base. A twaco-built package records itself while it is written; a package built another way can
be recorded directly.

```sh
twaco bundle --handoff acme-orders --apply
twaco handoff record backend.xml --name acme-orders --apply
twaco handoff list
twaco adopt returned-export.xml                 # review the three-way plan
twaco adopt returned-export.xml --apply         # take safe collaborator changes
```

`adopt` compares Composer's canonical form, so formatting and platform noise do not count. It
uses the named `--base <file|handoff|git-rev>`, otherwise the most similar recorded handoff; with
no base it uses git history to spot stale content and calls the rest unknown. The first report line
names the selected base. Handoffs are local derived files under `.twaco/handoffs/`; pass a package
with `--base` on another machine.

A base type that changed on something both sides have (a mashup parameter, a binding, a service
parameter, a field) is listed under its own heading, with the old and new type, whatever else
changed: whoever passes the old type now sends a value the other side does not expect. `--json`
and MCP `adopt_report` return them as `type_changes`.

| State | Meaning | Apply |
| --- | --- | --- |
| same | both sides already agree | does nothing |
| stale | ours changed after the handoff | keeps ours |
| theirs | only the collaborator changed it | writes theirs |
| conflict | both changed differently | reports it; use `--take` |
| added | it did not exist at the handoff or here | writes theirs |
| we removed | it was removed here | reports it |
| unknown | no base can prove its origin | reports it as a conflict |

Mashups, media, themes and configured UI collections are UI; everything else is backend. Use
`--only ui` or `--only backend` to narrow a run. Backend service and entity changes that are safe
to take are folded into XML and sidecars in the apply transaction. Mashup sidecars alone may need
`twaco sync --all` afterwards. Apply writes mashups and media; another UI entity it would take is
listed as `not adopted`, to import from the export. Resolve an individual conflict with
`--take theirs:Acme.Orders.Manager.GetOrders` or keep it with
`--take ours:Acme.Orders.Manager`; a take may name an entity or one service, and taking ours for
an entity leaves it whole. `--take theirs:ui` takes every UI conflict and unknown at once, for a
designer's export adopted without a base. `[adopt]` in `twaco.toml` still lists values that differ on every
export by design.

States compare entity XML, so sidecars edited since the last `twaco sync` are protected
separately: a backend entity with such a service is merged service by service rather than
replaced, and a mashup whose sidecars changed is kept until you sync and adopt again (or
`--take theirs:` it).

## Renaming

ThingWorx has no rename: an entity's name is its identity. A rename is therefore three steps,
and twaco does the first and the last:

1. **Repoint the repository.** `twaco rename` rewrites every reference and moves the files.
2. **Create the new names on the server.** `twaco deploy --apply`.
3. **Delete the old ones.** Nothing removes them for you, and an import never does.

```sh
twaco rename entity Acme.App.Manager Acme.App.Director      # one entity
twaco rename prefix Acme.App Acme.Platform                  # a building block: a Project and everything named below it
twaco rename prefix Acme.App Acme.Platform --apply --text   # write it, docs and sql included
```

Like `adopt`, it plans by default. The plan says how many entities move and how many references
change in the entity files, the sidecars, `twaco.toml`, the localization tables and the other text
files, and lists what it left for a person. `--apply` writes. A prefix rename changes the token names
in the localization tables (their name cells only) with the `[[token]]` references to them, always. `--text` also changes the other
text files (docs, `sql/`); without it they are counted and left alone. `--detail` lists every finding.

**What counts as a reference.** A name matches as a whole token: it is not part of a longer
name (`Acme.App-2`, `Acme.App_TS`, and for an entity not `Acme.App.Manager.Child`). In
`twaco.toml` and the other text files, an entity followed by one of its members
(`Acme.App.Manager.GetOrders`, as `[validate] inherited_overrides` writes it) is a reference and
is renamed; followed by anything else that is not an entity, it is left for a person. The same name
is found in an attribute, a text node, a script or a mashup's JSON, a URL
(`/Thingworx/MediaEntities/Acme.App.Icon_MD`), a localization token (`[[Acme.App.Save]]`), and in
the ids a mashup derives from an entity (`DynamicThingShapes_Acme.App.Management_TS`). A **short
name** such as `T` is only rewritten where it is the entire value or a whole quoted
string; the same name inside longer text (`T/T1/A1`) is reported as a
**review** finding and left alone. Nothing is rewritten that the rule does not name.

**What it writes.** Entity files and sidecar folders move to the new name; a file repository's
`filerepository/<name>/` folder moves with its Thing; `.twaco/baseline.json` forgets the old
entities (the new ones start with no baseline, as new entities do). It refuses before touching
anything if the new name already exists, nests with the old (`A` to `A.B`), is not a plain name,
or if `twaco check` already fails (`--skip-checks` overrides that). The files are written
through temporary files and, if any step fails, restored; a file someone saved after the plan
was made is refused rather than overwritten. Before the first write it applies the rename to a
throwaway copy of the workspace and refuses, writing nothing, if that would leave an entity's
sidecars out of step with its XML (a workspace over 1 GiB is not copied, and the output says the
rename was not verified first). Afterwards it checks that the sidecars still match
the XML and the gates still pass, and exits 2 if they do not.

**The ledger.** `.twaco/renames.json` records every rename: date, old, new, and each entity. It
is the only record of which old entities still exist on a server, so commit it.

**What a rename cannot carry** is server state, and the output says so each time: a Thing's
persisted property values, a Stream's or ValueStream's data, DataTable rows (a renamed DataTable
starts empty), group memberships, files already in a repository, and any row in an external
database that names an entity. Plan these before you delete the old entities.

**Renaming a field.** A configuration table's columns are the fields of its DataShape, so one
command covers both:

```sh
twaco rename field Acme.App.Settings_CT Period PeriodKey --apply
```

It renames the field in the DataShape and its `fields.json` sidecar, and in every configuration
table of that shape on every Thing, ThingTemplate and ThingShape: the inline copy of the shape
and each row's element (`<Period>` becomes `<PeriodKey>`, start and end tag together). Tables of
another shape that happen to have a field of the same name are not touched. The same field is
renamed in the InfoTable values of every property typed by the shape (a Thing's value is typed by
its template's property, so the inheritance chain is followed), and in a DataTable's accumulated
shape and indexes, with `datatable.json` regenerated. Scripts and mashup bindings that read the
field by name are not changed: every remaining whole-word mention is listed as a review finding.
The rows on a server keep their values only if you deploy with `--overwrite-tables`; a DataTable's
rows live on the server and the renamed column starts empty; and a field stored through
DBConnection needs its database column renamed first.

**Renaming a service.** `twaco rename service Acme.App.Management_TS GetPeriods GetTimePeriods`
renames the definition, the implementation and the `services/<name>/` sidecar folder, and
follows the callers: `me.X(`, `this.X(`, `Things["T"].X(` and a variable assigned from
`Things["T"]` in a script; the `Services`, bindings and events of every mashup that calls the
entity; and the `post_import`, `deploy_service` and `inherited_overrides` entries of
`twaco.toml`. The service must be declared on the entity you name: if it is inherited, twaco
says where it lives. Entities that inherit it and override it are renamed too, or the override
would stop overriding. Any other mention (a log message, a call on a receiver twaco cannot
resolve, a bare name in `inherited_overrides`) is listed as a **review** finding, never changed.
Scripts and tools outside the entities (a documentation generator, a test) are not rewritten.

**Renaming a service's input.** `twaco rename param Acme.App.Management_TS GetData cardUid cardId`
renames the input in the definition, as the variable it is inside the service's own script (not
after a dot, not an object key, not inside a string or comment), as a key of an object-literal
argument at the call sites twaco can resolve, and in the mashups that call the service. It refuses
a new name the script already uses (`n = Number(n)` would capture it), a JavaScript keyword, or a
name every ThingWorx script has (`me`, `result`, `logger`, `Things`...). A shorthand property
(`{ cardUid }`) or a redeclared name leaves that whole script as a review finding. Inputs of an
SQL service are named inside the query as `[[name]]` or `<<name>>`; those are listed for you to
change.

**Renaming a property.** `twaco rename property Acme.App.Machine_TS Level Height` renames a
property of a Thing, template or shape where it is declared, and follows it through the entities that
inherit it: the definition, the value element under `ThingProperties` (a Thing's value is typed by
its template's property), alert configurations, property bindings (including a binding elsewhere
that reads it as its source), a subscription's data-change event, a run-time permission keyed by the
name, and the reads and writes in scripts (`me.Level`, `this.Level`, `me["Level"]`,
`Things["T"].Level`, and a variable assigned from `Things["T"]`). `me.Level(` is a service call and is
left alone. A property declared on an ancestor is refused and the declaring entity named. Another
receiver, a quoted string and the mashups that bind the property are listed as review findings. A
server keeps the old property's persisted value and a ValueStream's logged data under the old name.

**Renaming a configuration table.** `twaco rename table Acme.App.Manager_TT Limits_CT Bounds_CT`
renames the table's definition on the entity that declares it and every instance on the Things
and templates that inherit it, and the `tableName: "Limits_CT"` calls in the scripts of that
entity's whole family (itself, what inherits it, and the shapes it implements). Any other mention
of the name, such as `const TABLE = "Limits_CT"`, is a review finding. The table's DataShape is a
separate entity: rename it with `rename entity` if it is named for the table. On a server the new
table's rows come from the entity XML, so deploy with `--overwrite-tables` to load them.

Files and folders whose *name* contains the old name (`docs/Acme.App.notes.md`) are reported,
not renamed. Renaming back (`rename prefix Acme.Platform Acme.App --apply --text`) restores the
repository byte for byte, apart from the ledger, provided the new name did not already
occur in it.

## Renaming with DBConnection tables

A solution that stores data through DBConnection names its tables in the manager's `GetDBInfo`
service. DBConnection makes a table from the lowercased short name of a DataShape and a column
from the lowercased field name, and `DeployComponent` creates tables but never renames one. So a
rename that touches such a DataShape cannot be left to the import: the database must change first.
`rename entity`, `rename prefix` and `rename field` therefore **refuse** while a DBConnection table
is involved until you choose:

```sh
twaco rename field Acme.App.Dashboards UserName OwnerName --sql --apply
twaco db run sql/2026-10-02-rename-field-Dashboards-UserName-to-OwnerName.sql --apply   # before deploying
twaco deploy --apply
```

`--sql` (or `--sql-dir <folder>`) writes the migration into `sql/` and `--no-sql` says the tables
are not in use. The script is one guarded, re-runnable transaction: the column (or table) rename,
the index, key and sequence names DBConnection derived from it, and, for an entity or prefix
rename, a plain substring replace of the old name in every text column of the DBConnection tables,
because rows name entities (a card stores the mashup it renders). A field rename also edits the
field's literals in `GetDBInfo` (its own `fields`, `indexedFields` and foreign keys, and the foreign
keys of other tables that point at it). Run the script before the import, with `twaco db run` or
psql, and back the tables up first; the header says how. SQL files outside the sources, such as a
migration an earlier rename wrote, are listed but never changed by `--text`.

## Deleting old entities

After a rename and a deploy, the old entities are still on the server, and nothing but you removes
them.

```sh
twaco entity delete --renamed                 # a plan: what would go, in what order, why it may not
twaco entity delete --renamed --apply
twaco entity delete Things/Acme.Old.Thing DataShapes/Acme.Old.Shape --apply
```

`--renamed` takes the old names from `.twaco/renames.json`; marks them deleted when done. For
each entity twaco asks the server what depends on it (`GetIncomingDependencies`) and refuses
while something outside the set still does, deletes dependents before what they depend on, and
confirms each entity is gone afterwards. An entity that the repository still defines is refused,
since the next deploy would create it again. Pass `--allow-repository-defined` to accept that
specific condition, or `--allow-outside-dependents` to accept structural dependents outside the
set. A disabled Thing cannot answer the dependency question, so its dependents are unknown: the
plan refuses it until you enable it or pass `--allow-outside-dependents`, and then warns. Deleting a FileRepository Thing deletes all its files and is refused until
`--allow-file-repository-data-loss` acknowledges that loss; when acknowledged, the plan still
warns about it. `--force` is deprecated: it means the first two acknowledgements and never the
FileRepository data-loss acknowledgement. The dependency answer sees structural dependents only,
never a name inside a script or a mashup, which the rename's own review findings cover.

## Moving and copying services and properties

Refactoring often means a service or property belongs somewhere else: lifted out of a Thing into
its template, or out of a template into a shape several things implement.

```sh
twaco copy service Acme.Reports_TS Acme.Common_TS Format --apply            # the source keeps it
twaco move service Acme.Manager_TT Acme.Common_TS Format --as FormatValue   # new name on the way
twaco move service Acme.Common_TS Acme.Singleton Format --leave-delegate    # callers keep working
twaco move property Acme.Manager_TT Acme.Common_TS Level
```

It plans by default. The definition (and a service's implementation and sidecar folder) is lifted
out of one entity's XML and put into the other's byte for byte, re-indented to where it lands;
a script inside its CDATA section is never re-indented, and twaco refuses to write if the script
would not arrive unchanged. It refuses a name the target already has, or that anything the target
inherits or anything inheriting it has, and a service that is declared on an ancestor of the source.

**What moving breaks.** Moving up into a template or shape the source inherits changes nothing for
callers: `me.Format` and `Things["X"].Format` still resolve. Moving anywhere else removes the member
from the source's instances, so the plan lists the references that stop resolving (scripts, mashup
bindings, `twaco.toml`: the same findings a rename of the member makes). `--leave-delegate` keeps
the service on the source with a body that calls the moved one on a Thing target, built from the
service's signature. Property values stored on Things are not moved. The next `deploy` makes the
server follow.

## Creating a building block

```sh
twaco new building-block Acme.Orders --description "Order handling" --base-extension PTC.Base:10.1.0
twaco new building-block Acme.Base --type abstract --apply
twaco new building-block Acme.Orders.Plant --type implementation --parent Acme.Base --apply
```

Plans by default. It writes what the building-block framework's Create New Building Block builds on a
server: the project, an `EntryPoint` template and Thing naming the block, a `Management_TS` shape, a
`Manager_TT` template and `Manager` Thing (none for an abstract block), a default and an admin group,
and an organization, in the repository's layout, and appends the project to `twaco.toml`. The text
matches what the framework itself produced (the tests hold it against server exports). Nothing that
exists is overwritten. Not created: the component permission helper (the framework builds it on the
server), and the `ui` and `test` types. Then `twaco check`, commit and `twaco deploy`.

## Creating entities

`twaco new` creates the common 10.1 entity exports in a project and plans unless `--apply` is
given. Every kind accepts `--project`, one-line `--description`, and repeated
`--visibility Organization/name` or `OrganizationalUnit/org:unit`.

```sh
twaco new thing Acme.Orders.Manager --template Acme.Orders.Manager_TT --shape Acme.Orders.Audit_TS
twaco new thing-template Acme.Orders.Manager_TT --base GenericThing --shape Acme.Orders.Audit_TS
twaco new thing-shape Acme.Orders.Audit_TS
twaco new data-shape Acme.Orders.Order_DS --field id:STRING:primary --field created:DATETIME --apply
twaco new timer Acme.Orders.Refresh_TM --every 30s --run-as Administrator --then "me.Refresh();" --apply
twaco new scheduler Acme.Orders.Nightly_SC --cron "0 0 2 * * ?" --run-as Administrator --apply
twaco new data-table Acme.Orders.Rows_DT --data-shape Acme.Orders.Row_DS --index byId=id --apply
twaco new stream Acme.Orders.Events_ST --data-shape Acme.Orders.Event_DS --apply
twaco new value-stream Acme.Orders.Values_VS --apply
twaco new media Acme.Orders.Logo_MD --file logo.svg --apply
twaco new style Acme.Orders.Good_SD --background '#66cc33' --foreground '#000000' --bold --apply
twaco new state Acme.Orders.Speed_SD --type numeric --state '<2000=Normal;background:#66cc33' --state '*=Overload;background:#ff0000' --apply
twaco new menu Acme.Orders.Main_MN --item Home=Acme.Orders.Home_MU --default --apply
twaco new theme Acme.Orders.Theme_TH --prop e-button-primary-fontcolor=#ffffff --apply
```

Things default to `GenericThing`; `--template`, `--shape`, template `--base`, and DataShape
`--base` must resolve in the solution (or the platform cache for templates and shapes). A Thing
can set `--value-stream`. DataShapes require one or more ordered `--field name:TYPE[:primary]`
arguments and write the matching `fields.json` sidecar. Timers accept `--every` as milliseconds,
`ms`, `s`, `m`, or `h` (60 seconds by default); schedulers accept a six- or seven-field Quartz
`--cron`. Both require `--run-as`, accept `--disabled`, and use either `--then <code>` or
`--script <file.js>` to put a Script subscription directly in their entity XML. The entity XML and
all managed sidecars are created together in one transaction.

DataTables and Streams require a DataShape; DataTables write `datatable.json` and validate every
indexed field. A DataTable's DataShape needs a primary key among its own fields: the server ignores
an inherited one, and refuses the import. ValueStreams, Streams, and DataTables always set the persistence provider. Media
accepts PNG, JPEG, GIF, or SVG content up to 10 MB. Styles use `#rrggbb` or `rgba(r,g,b,a)` colors.
States take ordered comparator specifications; menus require one or more `Title=Mashup` items; and
themes may copy a repository StyleTheme with `--from` before applying `--prop name=value` overrides.

On `sync`, a DataTable or Stream's `accumulatedDataShape` follows its DataShape exactly as the
server does on import. DataTables update both their XML and `datatable.json`; Streams update XML.
Do not edit `accumulatedDataShape` by hand.

## Asset hierarchies and networks

ThingWorx joins the Things of an asset hierarchy (enterprise, site, area, line, machine; a fleet of
vehicles) in a Network entity. A Network is a forest: a Thing appears in it once, under one parent,
and a connection whose `from` is empty is a root. The Network XML in the repository is the
declarative truth: deploying it replaces the server's whole connection list.

```sh
twaco network list
twaco network show Acme.Plant                  # the tree, with each Thing's template
twaco network show Acme.Plant --server         # the hierarchy currently on the server
twaco network show Acme.Plant Acme.Area2 --depth 1
twaco network diff Acme.Plant                  # repository versus server connections
twaco network pull Acme.Plant                  # plan copying the server hierarchy into XML
twaco network pull Acme.Plant --apply
twaco new network Acme.Plant --project Acme --apply
twaco network add Acme.Plant Acme.Site --as-root --apply
twaco network add Acme.Plant Acme.Line7 --under Acme.Site --type Child --apply
twaco network move Acme.Plant Acme.Line7 --under Acme.Area2 --apply
twaco network remove Acme.Plant Acme.Area2 --subtree --apply
```

`network show` prints each Thing with its ThingTemplate, or "outside the repository" for a Thing
only the server holds, then how many nodes each template has and anything wrong with the
hierarchy; a Thing name after the Network shows only its subtree, and `--depth` cuts it. `--json`
gives the tree, the flat connection list and the problems. `add` refuses a Thing already in the
network (`move` it instead), a parent that is not in it, and a Thing the repository does not hold
unless `--outside` says it exists on the server. `move` keeps the Thing's subtree and refuses a
parent inside it; `move` with `--type` under the same parent changes only the connection type. No
edit may leave the network with a cycle, a second parent or another problem it did not have. `remove` takes the connection, never the Thing, and refuses a Thing with
children unless `--subtree`. Each edit plans until `--apply` and changes only the connections it
names, in the file's own layout. The `networks` gate of `twaco check` reports a Thing with two
parents, a cycle, a parent that is not itself in the network, and a connection listed twice.

`network show --server` reads `GetNetworkConnectionsWithTemplate`, so the templates are the
ones the server reports. `network diff` lists connections only on either side and Things whose
parent or connection type differs; it exits 1 on drift. `network pull` reads the server's full
connection list and changes only the connections that differ, in the file's own layout, so a
hierarchy built at run time becomes declarative. It says how many server Things remain outside the
repository, and writes only with `--apply`. A Network import replaces the server list, so the
deploy plan says how many connections each import adds, moves and **REMOVES**. A server whose
connections changed since the last deploy is refused like any other server change (`--force`
overrides); pull first to keep them.

`network import` brings an asset export into that same hierarchy. Its `--file` is UTF-8 RFC 4180
CSV (a BOM, CRLF, quoted commas, doubled quotes and quoted newlines are accepted), with `name` and
`parent` headers and optional `type`, `template`, `description` and `project`. An empty `parent`
makes a root. Parents are applied before their children even if the CSV puts the child first. A
row without a `type` keeps an existing connection's type (a new one is `Contains`); a row that
gives a different parent or type changes the connection only with `--move`.

```csv
name,parent,type,template,description,project
Acme.Site,,Contains,GenericThing,Acme site,Acme
Acme.Line,Acme.Site,Contains,Acme.Line_TT,"Line, one",Acme
```

```sh
twaco network import Acme.Plant --file assets.csv --create-things
twaco network import Acme.Plant --file assets.csv --create-things --apply
```

An already-correct row is a no-op; an existing Thing under another parent needs `--move`.
`--create-things` requires a template on every missing Thing and creates it from a repository or
platform template (such as `GenericThing`), with the row project or the Network project. Without
it, missing Things need `--outside`. Refusals write nothing; an applied import creates all Things
and changes the Network in one transaction. MCP exposes the same read operations as `network` and
the edits and import as `network_write` (`dry_run` defaults to true).

## Changing entities

Each command plans until `--apply` and changes only the element it names.

```sh
twaco thing property add Acme.Orders.Machine_TT Speed:NUMBER --default 12.5 --persistent --apply
twaco thing property set Acme.Orders.Machine_TT Speed --no-logged --category Motion --apply
twaco thing property remove Acme.Orders.Machine_TT Speed --apply
twaco thing service add Acme.Orders.Machine_TT ReadSpeed --returns NUMBER --then "result = me.Speed;" --apply
twaco thing service remove Acme.Orders.Machine_TT ReadSpeed --apply
twaco thing subscription add Acme.Orders.Press OnSpeed --event DataChange --source-property Speed --then "me.Check();" --apply
twaco thing shape add Acme.Orders.Machine_TT Acme.Orders.Audit_TS --apply
twaco timer set Acme.Orders.Refresh_TM --every 5m --enabled --apply
twaco scheduler set Acme.Orders.Nightly_SC --cron "0 0 3 * * ?" --apply
twaco style set Acme.Orders.Good_SD --foreground '#000000' --bold --apply
twaco media set Acme.Orders.Logo_MD --file logo.svg --apply
twaco menu item add Acme.Orders.Main_MN Orders=Acme.Orders.Orders_MU --before Home --apply
twaco state set Acme.Orders.Speed_SD --type numeric --state '<2000=Normal' --state '*=Overload' --apply
```

A property name must be new to the entity, to what it inherits, and to everything that inherits
it (the server refuses that clash too). A persistent property has a stored value in every Thing
that has it, and the server writes that value into each Thing's export. So adding, removing,
retyping or implementing one also rewrites those Things' `ThingProperties`. Removing or retyping
a property that a mashup binding, another entity's script or a `me.<name>` read still uses is
refused, and so is stopping implementing a shape whose members are still used that way; `--force`
does it anyway and lists the users. A persistent INFOTABLE property is
refused: a 10.1 import re-stamps a stored infotable with the import time, so the repository could
never match the server. MCP: `entity_edit`.

`thing service add` writes the exported service signature and script sidecars, then folds them
into the entity through the same synchronizer as sidecar changes. Parameters are repeated
`--param name:TYPE[:DataShape]`; `--returns` defaults to `NOTHING`. `thing service remove` removes
an own script service, its run-time permission and sidecar folder. Both refuse inherited clashes;
remove also refuses callers unless `--force` lists and accepts them. MCP exposes the same work as
`entity_edit` actions `service_add` and `service_remove`.

## Creating mashups

```sh
twaco mashup new Acme.App.Orders_MU --project Acme.App                     # plan
twaco mashup new Acme.App.Orders_MU --project Acme.App --apply
twaco mashup new Acme.App.Main_MM --master --visibility Organization/Acme.App.Org --apply
twaco mashup new Acme.App.Navigation_MU --template sidebar --primary-menu data:Acme.App.Nav.Manager.GetPrimaryMenu --apply
```

`mashup new` writes a ThingWorx 10.2 responsive mashup the way Composer makes one: the entity file
under the project's `Mashups` folder, and its `content.json` and `custom.css` sidecars, written by the
same sync as every other mashup, so a new mashup is already in sync. `--template` selects a named
layout (and `--layout` remains a compatibility alias); `twaco mashup templates` lists the names,
aliases and slots. Slots are responsive container names usable with `mashup add --in <slot>`.

| Template | Slots | For |
| --- | --- | --- |
| blank | center | A free-form page |
| sidebar | sidebar, center | Navigation beside content |
| header-footer | header, center, footer | Framed application page |
| header-sidebar | header, body, sidebar, center | Navigation page with a header |
| sidebar-header | sidebar, main, header, center | Application shell: the menu the full height, a header above the page (the shape of the stock ThingWorx master templates) |
| form-dialog | title, fields, actions | Form editing dialog |
| confirm-dialog | close, message, actions | Confirmation dialog |
| list-page | title, toolbar, body | List and toolbar page |
| card | header, title, actions, body | Card content |
| master-detail | list, detail | Master and detail panes |
| kpi-tile | title, value | A compact KPI display |
| filter-panel | filters, content | Filters beside results |
| tabs | title, tabs | A tab set whose tabs each show a mashup (`--tab Label=Mashup`) |
| dashboard | title, tiles, tile-1..tile-4, panels, side, main | Four KPI tiles above a side panel and a main panel (1:3) |

`--template tabs` takes `--tab "Label=Mashup"` once per tab (up to 16; a label may be a `[[token]]`, and a
bare `Label` leaves that tab empty). Each tab gets the container Composer gives it, holding the mashup in a
contained mashup with its parameters cached, so `mashup bind` can reach them; without `--tab` the tab set
has Composer's two empty tabs. A tabs page cannot be a master.

`sidebar`, `header-sidebar` and `sidebar-header` require `--primary-menu`; their optional `--secondary-menu` values
are INFOTABLE-returning services. Their menu-bar data bindings and page-loaded service events are
exactly those `mashup bind` and `mashup on` would create. The menu bar shows a tree of `MenuEntry`
rows: one root row (`linkType` `Menu`, a `menuId` such as `topMenu`) and items whose `parentMenuId` is
that id, with `linkType` and `linkTarget` `Mashup` and the mashup in `linkDestination`. Rows without
the root show nothing, and twaco cannot check that from the service definition. `--master` makes a master mashup instead,
with a page container filling the template's master slot for the pages it shows. The theme is ThingWorx's default unless `--theme` names a
StyleTheme in the solution; twaco never creates one. Nothing is granted visibility unless
`--visibility` names an `Organization/<name>` or an `OrganizationalUnit/<organization>:<unit>`.
`--project` may be left out when the solution has one project. A name that is already a mashup, or
files already in the way, are refused.

### Contained mashups and parameters

A contained mashup receives only the parameters its container declares. Add one from a mashup in
the repository with `twaco mashup add-contained Acme.App.Board_MU Acme.App.Card_MU --set id=7
--apply`. It uses the same `--in`, `--right-of`, `--left-of`, `--above` and `--below` placement
rules as `mashup add`; without one it goes in `center`. Its visible child parameters become
container properties, so bind them as `ui:mashupcontainer-7.id`. IN parameters are binding
targets, OUT parameters are sources, and BOTH parameters support both directions. `Name` is the
MASHUPNAME target that selects the displayed child. A child cache that lacks a parameter is
refreshed as part of a binding plan and the plan names the refreshed container.

Composer stores one
parameter in four places: the mashup entity's `ParameterDefinitions`, the content root's
`_currentParameterDefs`, the base type on every binding property map that names it, and the
`MashupParameters` cache on every `mashupcontainer` showing that mashup. A missing cache entry
silently drops a binding.

Use `mashup param add`, `mashup param set`, and `mashup param remove` to keep those stores in
step. They plan by default, refresh every parent container in one transaction on `--apply`, and
refuse removals that still have bindings unless `--force` removes those bindings too. New mashups
may declare scalar parameters with repeatable `--param name:TYPE[:IN|OUT|BOTH]`; INFOTABLE
parameters need a DataShape and are added with `mashup param add --data-shape`.

### Adding widgets

```sh
twaco mashup add Acme.App.Orders_MU grid --apply
twaco mashup add Acme.App.Orders_MU grid --right-of ptcschartbar-3 --apply
twaco mashup add Acme.App.Orders_MU button --below ptcsgrid-7 --label Save --apply
```

`mashup add` plans a catalogue widget insertion until `--apply`. Without a placement it adds to
`center`; `--in` names a responsive container, and the four beside flags name the widget to place
next to. A matching row or column accepts the sibling directly. When its direction differs, twaco
makes a row or column and gives both widgets their own containers; it also wraps existing leaf
siblings where that is needed to preserve responsive layout. A widget that can fill fills only
when alone in its container. `--label` selects the first available LabelText, Label, Text or Title
property; `--prop Name=value` changes a simple catalogue default property with its expected type.
Managed mashups update `content.json` and the entity in one transaction; unmanaged mashups update
only their entity file. If sidecars and entity content disagree, resolve it first with `sync` or
`extract`.

### Binding

```sh
twaco mashup bind Acme.App.Orders_MU --from data:Acme.App.Orders.Manager.GetOrders --to ui:ptcsgrid-3.Data --apply
twaco mashup bind Acme.App.Orders_MU --from ui:ptcstextfield-4.Text --to data:Acme.App.Orders.Manager.SaveOrder.note --apply
twaco mashup bind Acme.App.Orders_MU --from mashup:OrderId --to data:Acme.App.Orders.Manager.SaveOrder.id --apply
```

`mashup bind` adds one binding until `--apply`. Addresses are the ones `mashup describe` shows:
`ui:<widget-id>.<Property>`, `mashup:<Parameter>`, and `data:<Entity>.<Service>` for a result or
`data:<Entity>.<Service>.<field-or-parameter>`. A mashup call with its own name is
`data:<Entity>.<Service>[as <alias>]`, and a selected row field is
`data:<Entity>.<Service>[SelectedRows].<field>` (`[as <alias>][SelectedRows]` for both). When the
mashup calls a service more than once, the address must name the call with `[as <alias>]`; an
alias another service's call already has is refused. Widgets are named by id, display name or a
type word only one widget matches, never by a loose catalogue search. The command checks property
direction and base types before it writes, and warns when the target already has another source. A missing call for a Thing is added to a `Things_<Entity>` data source;
templates and shapes need the runtime entity binding Composer creates, so twaco asks you to add
that call there once. The entity and managed content sidecar change in one transaction.

### Events

```sh
twaco mashup on Acme.App.Orders_MU --event ui:ptcsbutton-6.Clicked --invoke data:Acme.App.Orders.Manager.SaveOrder --apply
twaco mashup on Acme.App.Orders_MU --event mashup:Loaded --invoke data:Acme.App.Orders.Manager.GetOrders --apply
twaco mashup on Acme.App.Orders_MU --event data:Acme.App.Orders.Manager.SaveOrder.ServiceInvokeCompleted --invoke data:Acme.App.Orders.Manager.GetOrders --apply
```

`mashup on` makes one event run a service, until `--apply` only a plan. The event is a widget's
(`ui:<widget>.<Event>`, one its catalogue contract lists, such as a button's `Clicked`), the
page's `mashup:Loaded`, or a call's `ServiceInvokeCompleted`, `AllDataChanged` or
`SelectedRowsChanged` for a call the mashup already has. It runs a data service, added as a call
when a Thing's service is not called yet, or a widget's own service such as a grid's `Reset`.
Widgets whose contract the catalogue does not know are refused. `mashup describe` lists the
result as `trigger -> handler`.

### Events from a contained mashup

```sh
twaco mashup event add Acme.App.OrderCard_MU saved --when ui:ptcsbutton-6.Clicked --apply
twaco mashup on Acme.App.Orders_MU --event ui:mashupcontainer-7.saved --invoke data:Acme.App.Orders.Manager.GetOrders --apply
```

ThingWorx gives a contained mashup no events of its own, so real mashups signal with a value:
`mashup event add` adds a NUMBER parameter (BOTH, refreshed in every parent's copy) that an
expression sets to the time whenever `--when` fires. A parent reacts with `mashup on --event
ui:<container>.<name>`, which feeds the container's parameter into an events router (reused by a
second handler) whose `Changed` runs the handler. Any BOTH or OUT parameter of the mashup can be
reacted to the same way; an IN parameter cannot.

### Function widgets

```sh
twaco mashup add Acme.App.Orders_MU expression --input note:STRING --prop "Expression=Output = note === '';" --prop OutputBaseType=BOOLEAN --apply
twaco mashup bind Acme.App.Orders_MU --from ui:ptcstextfield-3.Text --to ui:expression2-7.note --apply
twaco mashup bind Acme.App.Orders_MU --from ui:expression2-7.Output --to ui:ptcsbutton-6.Disabled --apply
twaco mashup on Acme.App.Orders_MU --event ui:ptcstextfield-3.Changed --invoke ui:expression2-7.Evaluate --apply
```

Expressions, validators, events routers, navigation functions, status messages and auto-refresh
draw nothing: `mashup add` puts them beside the layout, under the mashup itself, and refuses a place
in it. An expression's or validator's inputs are declared with `--input name:TYPE` and become its
bindable targets; an expression's `Output` has the base type its `OutputBaseType` says. An events
router's inputs are `Input1` to `Input<NumberOfInputs>`, of its `DataType`. A navigation function's
bindable properties include the parameters of the mashup it opens. The legacy `expression` and
`validator` stay readable in existing mashups; adding either adds `expression2` or `validator2`.

### Recipes

```sh
twaco mashup recipe enable-when Acme.App.Edit_MU ptcsbutton-6 --if "note !== ''" --input note=ui:ptcstextfield-3.Text --apply
twaco mashup recipe popup Acme.App.Orders_MU ptcsbutton-6 --mashup Acme.App.EditOrder_MU --pass orderId=data:Acme.App.Orders.Manager.GetOrders[SelectedRows].id --reload data:Acme.App.Orders.Manager.GetOrders --apply
twaco mashup recipe close-after Acme.App.EditOrder_MU --after data:Acme.App.Orders.Manager.SaveOrder --apply
twaco mashup recipe validate Acme.App.Edit_MU ptcsbutton-6 --if "note !== ''" --input note=ui:ptcstextfield-3.Text --then data:Acme.App.Orders.Manager.SaveOrder --else-say "A note is required" --apply
```

`mashup recipe` writes, in one edit, the wiring real mashups repeat, from the same pieces `mashup add`,
`bind` and `on` use:

| Recipe | What it writes |
|---|---|
| enable-when, show-when | an expression of `--if`, fed by each `--input name=address`, into the widget's `Disabled` (negated) or `Visible`, evaluated on load and whenever an input changes |
| text | a STRING expression (`--expr`) into a label's text |
| message | a status message (`--say` or bound `--from`; `--kind info|warning|error`) when the `--after` call completes |
| confirm | a confirmation (`--title`, `--say`) the click opens; its action runs `--then` |
| popup | a navigation function opening `--mashup` (`--window`, `--size WxH`, `--title`), each `--pass name=address` into its parameters, `--reload` run when it closes |
| close-after | the mashup closes itself (when it is a popup) once the `--after` call completes |
| auto-refresh | `--then` run every `--every` seconds, or at a bound `--interval` |
| validate | a validator of `--if` the click evaluates: true runs `--then`, false shows `--else-say` |
| chart | `--data` bound into the chart and its category field `--x` named (a pie: `--x` label and `--value` fields); every other numeric column of the result becomes a series; `--kind` bar/column or linechart/areachart |
| columns | the grid's `ColumnFormat`: each `--column field[:Title[:width]]` in order, rendered by the field's base type; `--data` also bound |
| picker | a dropdown or list filled from `--data`, showing `--x`, valued by `--value`, its selection bound from `--selected` |

### Validating mashups

```sh
twaco mashup validate Acme.App.Orders_MU
twaco mashup validate --all --json
```

`mashup validate` reads the repository and reports what ThingWorx would load but not honour, or
what the next edit would trip over: content that is not a mashup twaco can edit, a container holding
both containers and widgets, bindings and events naming a widget, data source or call the mashup no
longer has (Composer leaves these behind when a call is removed), a binding against a catalogue
widget's contract (the wrong direction, a property it lacks, such as a misspelt expression input),
an event or service a widget lacks, a binding with several property maps, and the contained-mashup
drift `twaco check` also reports. What it cannot judge, extension and legacy widgets, properties and
events a widget makes per configured action, a collection's cell parameters, it leaves alone. It
exits 1 when it finds anything.

### Finding widgets

```sh
twaco widgets --search chart
twaco widgets grid
```

`widgets` reads the embedded ThingWorx 10.2 catalogue, so it needs neither a solution nor a
server. Search by type, Composer label or alias; name one widget to see the properties that can
bind, their base types and directions, and its events and services. `--json` gives the same data
to automation. A container with no binding contract is marked so it can be placed without being
mistaken for a bindable widget.

### Reading a mashup's wiring

```sh
twaco mashup describe Acme.App.Orders_MU
twaco mashup describe Acme.App.Orders_MU --json
```

`mashup describe` answers "which services drive this page, and is anything left unset" without the
mashup's XML: the widget tree with each container's direction, the mashup's parameters, every
service it calls with what sets each declared parameter (a binding's source, `static`, or
`unbound`, which falls back to the service's default) and what runs it, then every binding and
event. It reads the sidecar `content.json` when there is one. Addresses are written as
`ui:<widget>.<Property>`, `data:<Entity>.<Service>[.<Parameter>]` and `mashup:<Parameter>`; a call
the mashup names differently from its service is `data:<Entity>.<Service>[as <name>]`, a source of
selected rows `data:<Entity>.<Service>[SelectedRows].<field>`, and the entity a shape's data source
runs on `data:<Entity>[EntityName]`. A property map after a binding's first is listed as ignored,
since the runtime applies only the first.

## Moving entities to another project

```sh
twaco reproject Acme.App.Report_DS Acme.App.Reports --to Acme.Reporting           # plan
twaco reproject Acme.App.Report_DS Acme.App.Reports --to Acme.Reporting --apply
```

`reproject` changes each entity's `projectName` and, when the projects keep their files in
different folders, moves the file to the same collection folder under the other project's root.
Sidecars are filed by entity name and stay where they are. Every file changes in one transaction,
or none does. The plan also names each reference twaco resolves (templates, shapes, DataShapes,
script references it can read, mashup bindings, `twaco.toml`) that the move puts outside the deploy
order: an entity of a project that does not depend on the new one using a moved entity, or a moved
entity using one of a project the new one does not depend on, each with the `depends_on` that
allows it; a mere mention of the name is not checked. A misfiled entity, or a collection the new
project does not keep, is refused. On the server, the next deploy's import gives the entities their
new project; until then `entity status` reports them `local-changed`, which that deploy records.

## Changing a template

Pointing a Thing at a different template, a template at a different base, or adding or removing an
implemented shape changes what the entity inherits, and the platform says nothing about what it
leaves behind. `retemplate` works it out first:

```sh
twaco retemplate Acme.Boiler01 --to Acme.Boiler_TT                  # a plan
twaco retemplate Acme.Boiler_TT --to Acme.Equipment_TT --apply
twaco retemplate Acme.Boiler01 --add-shapes Acme.Alarmed_TS --remove-shapes Acme.Legacy_TS
```

The plan lists the entity and everything inheriting it, the properties, services and configuration
tables each gains and loses, the stored property values and configuration-table rows that would be
left with no definition, and the scripts, bindings, alerts and mashups that still refer to a lost
member. A loss that holds data or is still referenced is refused unless `--accept-loss`. The only
edits are the `thingTemplate` or `baseThingTemplate` attribute and the `ImplementedShape` elements,
made in place. A template that is not in the solution (a platform template) is accepted, and the plan
says its own members are not known.

## Backups before something cannot be undone

A delete is gone for good, and a forced push or deploy replaces changes the repository has never
seen, so twaco saves the server's copy first. `entity delete --apply` exports every entity it is
about to delete, and `entity push --force` and `deploy --force` export the ones they would overwrite
that hold such changes, into `.twaco/backups/<date-time>/<Collection>/<Name>.xml` with a
`backup.json` saying why. A backup that cannot be taken stops the command before anything changes;
`--no-backup` goes ahead without one. The newest 20 sets are kept.

```sh
twaco entity restore                          # the sets there are
twaco entity restore 20261002-164541          # a plan: what would be imported
twaco entity restore 20261002-164541 --apply  # import it all, or name entities after the set
```

A set is the Exporter's XML of each entity, which the server imports as it is: definitions,
configuration and permissions. It does not hold persisted property values, DataTable rows, stream
data or a FileRepository's files. A delete backup is therefore not a backup of a FileRepository's
files, which is why deleting one needs its own explicit acknowledgement. Keep `.twaco/backups/`
and `.twaco/transactions/` out of git; `twaco init --agents` adds both to `.gitignore`.

## Carrying permissions to the new entities

A rename creates new entities, and permissions set on the server at run time (who may invoke a
service, edit an entity, see it) are not in the XML twaco imports. `entity carry` copies them from
the old entity to the new one before you delete the old one:

```sh
twaco entity carry --renamed                  # a plan: which permission sets differ, nothing written
twaco entity carry --renamed --apply
twaco entity carry Things/Acme.Old.Thing Things/Acme.New.Thing --apply
```

It reads run-time, design-time and visibility permissions through the platform's
`Get...PermissionsAsJSON` services, maps every principal through the rename ledger (so a group
renamed by a prefix rename is granted under its new name, including `Organization:Group` forms),
writes only the sets that differ, reads each back, and marks the ledger entry carried. A set that
reads back different is reported as a failure, and the rest are still attempted (exit 2).
`--detail` also shows the platform's own difference count. Entities missing on either side are
reported and skipped.

## Permissions the import leaves behind

An import only adds permissions. A grant removed from the entity XML stays on the server after the
next deploy, and a principal the server already lists keeps its own allow or deny whatever the XML
says, so a deny written in the repository can be silently ignored. Deploy reports such an entity as
not kept, and says when its permissions are the only difference.

```sh
twaco permissions diff --all                 # what differs, set by set; exit 1 when anything does
twaco permissions push Acme.App.Manager      # a plan
twaco permissions push Acme.App.Manager --apply
```

`diff` compares each set the entity XML declares with the server's, through the platform's
`Get...PermissionsAsJSON` services, and names every grant the server alone has, every grant the
repository alone has, and every allow/deny that differs. Order is not a difference. `push --apply`
writes each differing set whole through `Set...PermissionsAsJSON`, which replaces the set, reads it
back, and records the baseline of every pushed entity that then matches the server, so the next
deploy needs no `--force`. A set the XML has no block for is never compared or written. A
ThingShape's `InstanceRunTimePermissions`, and a ThingTemplate's three `Instance...Permissions`
(what its Things get), are sets of their own and are compared and pushed the same way.

## A permission policy

Write who may use what once, in the project's `permissions.toml`
([Configuration](CONFIGURATION.md#permissionstoml-who-may-use-a-project)), and let twaco check
the entity XML against it. For a project that already has permissions, start from a draft:

```sh
twaco permissions init                  # print a draft for each project without a policy
twaco permissions init --apply          # write them; `permissions apply` then has nothing to do,
                                        # except what the drafts' notes name
twaco permissions init --from-helper    # the grants of the helper's tables, not the entity XML
```

A draft names each rule's roles outright and lists resources one by one; `includes` and
patterns such as `Get*` make it shorter. Roles come from the permission helper when the project
has one. An entity whose run-time block holds a deny or a principal that is not a group, or whose
visibility block denies a role's unit, is left `unmanaged`, with a note. When the helper's
tables disagree with the entity XML, a note says `apply` will rewrite them. `--from-helper` is how a matrix someone edited in the helper's mashup
comes back into the repository.

```sh
twaco permissions audit                 # every project with a policy; exit 1 on any error
twaco permissions audit --detail        # and every grant behind a finding
```

Without a server, the audit reports:

- **errors:** run-time and visibility blocks that differ from the policy; services of a `strict`
  entity that no rule names; a group or user in a visibility block (the server answers HTTP
  500); an Organization in run-time permissions; a principal `[visibility] remove` names.
- **warnings:** principals under one of the solution's projects that no entity defines, such as
  an organizational unit an Organization does not declare; rules and patterns that match nothing.
- **notes:** explicit denies.

With `--server` (and `--profile`), the audit also reads the server, and writes nothing:

- each entity's permission sets against the repository's, as `permissions diff` compares them;
- in helper mode, the helper's three tables on the server against the repository's;
- each `[[platform]]` grant and membership: what the Solution Framework's `DeployComponent` does
  on entities the project does not own, which an import cannot carry. An entry whose `requires`
  project is not on the server is skipped, with a note;
- each role's organizational unit: it must exist and hold the role's group, or the role sees
  nothing.

```sh
twaco permissions audit --server --profile production
twaco permissions push --platform                # what is missing
twaco permissions push --platform --apply        # add it, read it back; nothing is removed
```

`push --platform` adds the `[[platform]]` grants and memberships the server lacks, one at a time
(`AddRunTimePermission`, `AddMember`), as `DeployComponent` does. Those entities are shared by
every block on the server, so it never removes anything.

`permissions apply` writes the policy into the entity XML: the run-time block of each Thing, the
instance run-time block of each ThingShape and ThingTemplate, and the role principals of each
visibility block. Only blocks whose grants differ are rewritten, in the export's layout and the
file's line ending; what a block keeps stays in its order, and new principals follow in the
order of the roles. Every changed file is written in one transaction. It is refused while a
strict entity has a service no rule names.

```sh
twaco permissions apply                 # the plan: files, grants added and removed
twaco permissions apply --apply
twaco deploy --apply                    # an import adds grants ...
twaco permissions push --all --apply    # ... and only a push removes them on the server
```

A project with a Solution Framework permission helper Thing is in helper mode. The helper's
template ships in `PTCDTS.Base`, so a server with only the common blocks can have one; a project
without one is in plain mode. Nothing needs the Solution Framework itself. In helper mode the
audit also compares the helper's three tables and the columns of its two DataShapes with the
policy, and `apply` writes them, so the helper's mashup shows what the entity XML grants. A
change made in that mashup shows up in the audit; carry it into `permissions.toml`.

## Testing services as users

A permission policy says who may call what; `twaco test` checks that the server agrees. The test
users are declared in `twaco.toml`, each with its groups ([Configuration](CONFIGURATION.md#test-and-testuser-test-users)),
and share one password, the profile's `test_password`. A profile must say
`allow_test_users = true` before twaco creates an account on its server or runs a case there.

```sh
twaco users list                 # each test user: present, signs in, in its groups
twaco users seed --apply         # create what is missing; nothing that exists is changed
twaco test                       # read and check tests/*.toml, list the calls
twaco test --apply               # run them; exit 1 when one fails
twaco test dashboards --apply    # only the cases whose name, service or file says dashboards
twaco users delete --apply       # remove the test users
```

A case names a service, the test users or groups it runs as, and the status it expects: 200 for
a call the role may make, 401 for one it may not, 404 for a user who cannot see the entity at all.
A new user in no group is answered 404 by most of the platform's resources too, `EntityServices`
and `CurrentSessionInfo` included; a wrong name or password is always 401.

```toml
[[case]]
name = "a viewer reads the dashboards"
service = "Acme.App.Manager.GetDashboards"   # Thing.Service, or Collection/Name.Service
as = ["Acme.App.Viewers_UG", "nobody"]       # a group means each test user in it
expect = 200
params = { limit = 10 }
rows = 2                                      # optional, with expect = 200
first = { name = "Overview" }                 # optional: these fields of the first row
```

A case calls its service as written: one that creates something creates it, as the user it runs
as. The cases run in file and case order, so a case can clean up after the one before it. `seed`
needs the groups on the server, so deploy first; it never resets a password, and `users list`
reports a user that cannot sign in with `test_password`.

For one call by hand, `twaco call <target> <service> [<json>] --as <test user>` signs in as that
`[[test.user]]` with `test_password` (the profile needs `allow_test_users`). A user in no group
that can see the entity gets 404; a wrong password gets 401. No other account can be named.

Add `--lang <code>` to make that call in another language: `--lang de`, or a preference list
`--lang fr,de` (the first language with a localization table wins; one without falls back to
Default). ThingWorx ignores `Accept-Language` and takes a call's language only from the calling
user's stored preference, so twaco reads the test user's preference with the profile's own
account, sets it, makes the call, and puts the old one back, even when the call fails. It works
only with `--as`, so your own account's language never changes. If putting it back fails, the
error says what it was and how to restore it. MCP: `call` with `as` and `lang`.

## Application keys

An application key lets a browser, a script or another tool call the server as a user without
its password (`?appKey=` on a Runtime URL, or an `appKey` header).

```sh
twaco appkey create Acme.Kiosk_AK --user kiosk --expires 90d --project Acme --save default --apply
twaco appkey show Acme.Kiosk_AK --show
twaco appkey delete Acme.Kiosk_AK --apply
```

`create` plans until `--apply`. It always sends an expiration, `30d` unless `--expires` says
otherwise (`12h`, a date at midnight UTC, or a timestamp): without one the server expires a key
after a day. `--whitelist 10.0.0.0/8,127.0.0.1` limits where it may be used from. The key itself
is a credential and twaco prints it only with `--show`; `--save <profile>` writes it as that
profile file's `app_key` instead, replacing an old one and keeping the file's other lines and
permissions, and the profile file must already exist. A key that was made but not saved is not
lost: `appkey show <name> --save <profile>` reads it again. Re-importing a key's XML never changes
its expiration, so twaco creates keys through the server rather than as repository entities.
`appkey delete` is `entity delete ApplicationKeys/<name>`, with its plan and backup. twaco itself
signs in with the profile's username and password either way, since the Importer refuses app
keys. MCP: `appkey` (create, show); delete with `entity_delete`.

## Copying DataTable rows

Renaming a DataTable creates a new, empty one: its rows live on the server, not in the XML.
`datatable copy` moves them:

```sh
twaco datatable copy Acme.Old_DT Acme.New_DT                  # a plan: the field mapping, the row count
twaco datatable copy Acme.Old_DT Acme.New_DT --map label=title --apply
```

Fields are matched by name, then by the field renames in `.twaco/renames.json`, then by `--map
old=new,...`. A source field with no target is refused (`--drop-unmapped` leaves it behind), as is
a type change, a target that already has rows (`--append` allows it) and a table over `--max-rows`
(default 100000). After the write the target's row count and rows are read back and compared. Each
row's source, tags and timestamp are not carried: the platform stamps the caller and the time.

## SQL through a Thing

A DBConnection table, or any database work a rename needs, can be run through the platform itself,
which is the only route on an instance where you have no psql:

```sh
twaco db run migration.sql                    # a plan: the Thing, the JDBC URL, the SQL
twaco db run migration.sql --apply            # one atomic script
twaco db query -q "SELECT count(*) FROM dashboards"
```

`db run --apply` imports a throwaway Thing on the built-in `Database` template with a `SQLCommand`
service, copies the project's connection, sets its password (encrypted through the platform) from
the profile's `database_password`, runs the script, and deletes the Thing whatever happens. The
script is one transaction: a failing statement undoes the whole script. Use `--no-transaction`
for a statement that cannot run in one (`CREATE DATABASE`). `db query` runs a read-only
`SQLQuery`, with the database itself set read-only (PostgreSQL), so it cannot change anything.
The password is never printed.

If a run is killed before it can delete its Thing (power loss, a closed terminal), the Thing stays
on the server. `twaco db clean` lists such leftovers and `--apply` deletes them; it touches only
names twaco generates (`ZZ.Twaco.Sql.` and eight hex digits), only on `Database` Things, and
confirms each is gone.

## Running and observing

- **`twaco call <Thing> <Service> '<json>'`** calls a service now. A service can write, and
  twaco cannot tell which do, so know what it does first. `--with-logs` then shows what the
  call wrote to ScriptLog and ApplicationLog, waiting up to 3 seconds for them. A target
  other than a Thing is written `ThingTemplates/<Name>` or `Resources/<Name>`.
- **`twaco logs ScriptLog --since 10m --level WARN`** reads a log, newest first, in local
  time. `--grep`, `--user` and `--thread` narrow it.
- **`twaco logs level ScriptLog DEBUG --apply`** changes a log's level for the whole server.
  The command prints the undo; run it afterwards.
- **`twaco config-table <Thing> <Table> --backup before.json`** saves a configuration table
  before a test that writes it. `--diff` compares it with the repository, and
  `--restore before.json --apply` puts it back. `SetConfigurationTableRows` replaces whole
  rows, so a backup is the only undo.
- **`twaco settings --search <words>`** finds a subsystem setting by name or description.
  It is read-only, and password values are never shown.

## Server content

- **File repositories:** `twaco repo ls`, `get`, `status`, `put`, `mkdir`, `rm`, `mv`, `push`
  and `pull` mirror Composer's Repository page. `filerepository/<repo>/` in the solution
  holds a repository's tree. `push` and `pull` never delete, and a file that differs on both
  sides needs `--overwrite`.

- **Extensions:** `twaco ext list`, `show`, `import` and `remove`. `import` without `--apply`
  only has the server validate the package; nothing is installed.
- **Finding entities:** `twaco search <text>` asks the server what Composer's Spotlight box
  asks it, for text anywhere in a name or a description (`*` makes it a pattern: `*_DS`).
  `--type` narrows to entity types, given either way (`Mashup` or `Mashups`), and `--project`
  to one project. Each result is `Collection/Name`, which `twaco entity get` prints and
  `twaco export entity` writes to a file, whether or not the entity is in the repository.
  The list stops at `--limit` (100) and says when more match.
- **Exports and imports:** `twaco export` and `twaco import` do what Composer's Import/Export
  dialog does, for an entity, a collection, a project or a source-control tree. An import
  without `--apply` lists what it would add and what it would replace.

### Localization tables

A solution keeps its tokens of the shared localization tables under `localization/`, one file
per table: `localization/<Project>/LocalizationTable.xml` for `Default` and
`LocalizationTable_<table>.xml` for each language, holding only the project's tokens. Other
names and flat layouts are read too, by the table name inside each file. twaco edits these files
in place, so section comments and row order survive. A file holding several tables, as an
extension ships them, is read table by table: `status` compares it and `push` imports each of its
tables alone (the other tables cut out of the bytes it sends); `pull`, `set` and `remove` refuse to
edit it and say to split it into one file per table.

```sh
twaco localization status --detail                 # each token: same, differs, local only, server only
twaco localization pull --apply                    # the server's tokens into the files
twaco localization push --apply                    # import the tables that differ, Default first; read back
twaco localization push --prune --apply            # also delete server tokens no file has
twaco localization new de --language-common German --language-native Deutsch --apply
twaco localization set Acme.App.Title --value Title --apply
twaco localization set Acme.App.Title --value Titel --table de --apply
twaco localization remove Acme.App.Title --apply   # from every table
```

A project's tokens are those named under its prefixes: the project name followed by `.`, or
`[project.localization] prefixes`. Every command but `status` plans until `--apply`. An import
never removes a token, which is why deleting needs `--prune`. A language token must also be in
Default: the server's token services refuse one that is not, although an import takes it, so
twaco refuses to push one, and a token pruned from Default is pruned from every language table
that has it. `status` exits 1 for such a token, a duplicate or an unreadable file, since each
blocks a push. Files twaco creates carry no
`projectName`: importing one with a `projectName` would put the shared table into that project.

## Releases

`twaco package` builds release artifacts from the repository, offline:

```sh
twaco package bundle --out dist/acme.xml                    # one importable XML
twaco package bundle --project Acme.Core --backend-only --out dist/core-backend.xml
twaco package source-control --out dist/acme-sc.zip         # <Project>/<Collection>/<Name>.xml
twaco package extension --project Acme.Core --out dist/acme-core.zip
twaco package extension --out dist/acme-extensions.zip      # every project, a zip of zips
```

An extension package is a ThingWorx extension zip: `metadata.xml` from `[package]` and each
project's `depends_on`, then every entity. With `--editable`, its entities stay editable once
installed; without it they are locked, as a shipped extension's are. Entity files go in byte
for byte except that one attribute. `twaco ext import <zip>` has a server validate a project's
package before anyone installs it.

## Updating twaco

Once a day, a command run in a terminal checks for a newer twaco release, with a two-second
timeout. When one exists, a line on stderr says so after the command's own output. These never
check:

- `twaco mcp`, `twaco update` and `twaco --version`.
- A run with `CI` set, or with stderr redirected.
- Any run with `TWACO_NO_UPDATE_CHECK=1`.

The result is cached in twaco's cache directory, so the network is asked at most once a day.

`twaco update` compares this binary with the latest release. `twaco update --apply` does this:

1. It verifies the minisign signature of the release manifest, `updater.json`.
2. It downloads the release archive for this platform.
3. It verifies the archive's signature. The signature names the archive, so the archive of
   another release cannot pass as this one.
4. It replaces the running binary.

`twaco update --apply` installs a release only when it is newer than the running binary.

The cache also records the highest version that a signed manifest showed. An old copy of the
manifest can still have a valid signature, so twaco uses that record:

- The daily notice keeps naming the newer release.
- `twaco update` refuses a manifest older than the recorded version. Try again later, or
  download the release by hand.

On Windows, a failed replacement puts the old binary back. When that also fails, the error
names the copy of the old binary, `.twaco-backup-<number>.exe`. Rename it to `twaco.exe`.

When the binary is in a directory you cannot write, such as `/usr/local/bin`, run
`sudo twaco update --apply`. A `.deb` install and an AppImage are not replaced: install the next
release's `.deb` or AppImage instead.

## Working together

- **One writer at a time.** A command that writes twaco's managed files takes a lock in
  `.twaco/`, so an agent and a person, or two agents, cannot interleave their writes: extract,
  sync, fmt, types, bundle, an applied deploy, push, adopt or `repo pull`, and `entity status
  --record`. A file you name yourself (`export`, `package`, `entity get --out`, a
  `config-table` backup) is written without it. A lock left by a crashed process is detected
  and cleared.
- **Agents.** `twaco mcp` serves most of this work to an AI agent ([MCP server](MCP_SERVER.md)),
  and `twaco guide` gives it the knowledge it needs ([Knowledge](KNOWLEDGE.md)). Write what
  you learn about the solution in its AGENTS.md or `docs/`: `guide` searches them, and the next
  agent starts from them.
