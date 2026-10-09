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
3. sends every script to the server's parser, and stops on any failure;
4. checks every entity for a conflict with the baseline;
5. imports each bundle, reads each entity back, and fails if the server did not keep it;
6. calls the project's deploy and post-import services, if `[project.deploy]` names them;
7. records the new baseline.

With `--only`, the project's deploy service still runs, but its `post_import` calls are
skipped: they are written for a whole project's import.

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
`twaco sync --all` afterwards. Resolve an individual conflict with
`--take theirs:Acme.Orders.Manager.GetOrders` or keep it with
`--take ours:Acme.Orders.Manager`; a take may name an entity or one service. `[adopt]` in
`twaco.toml` still lists values that differ on every export by design.

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
change in the entity files, the sidecars, `twaco.toml` and the other text files, and lists what
it left for a person. `--apply` writes. `--text` also changes the other text files (docs, `sql/`,
localization tables); without it they are counted and left alone. `--detail` lists every finding.

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
set. Deleting a FileRepository Thing deletes all its files and is refused until
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
in place, so section comments and row order survive; a file holding several tables is reported
as unreadable (keep one table per file).

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
