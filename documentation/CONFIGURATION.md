# Configuration

twaco reads these configuration files:

- **`twaco.toml`** describes the solution. It is committed. twaco looks for it in the working
  directory and then upwards, so any command works from any folder of the solution.
- **Server profiles** say how to reach a ThingWorx server. They hold credentials and are
  never committed.
- **`requirements.toml`** is an optional, committed contract for the dependencies the solution
  needs from its server and does not ship itself.

`twaco init` proposes a `twaco.toml` from the entities themselves; most solutions need little
beyond what it writes.

## `requirements.toml`

An optional reviewed contract at the solution root, committed with the source. Draft it with
`twaco requirements init`, then write it with `--apply`. Every section is optional. Unknown
fields and unknown `[entities]` collection names are refused with the file and key to fix.

```toml
[platform]
minimum = "10.1"

[[extension]]
name = "Acme.Base"
minimum = "2.1.0"

[[project]]
name = "Acme.Shared"

[entities]
ThingTemplates = ["GenericThing", "Acme.ExternalTemplate"]
ThingShapes = ["Acme.ExternalShape"]
DataShapes = ["Acme.ExternalRows"]
Things = ["Acme.ExternalEntryPoint"]
Organizations = ["Acme.Shared_OR"]
Groups = ["Acme.Operators"]
Widgets = ["Acme.ExtensionWidget"]

[optional.entities]
Things = ["Acme.OptionalEntryPoint"]
```

Collection keys use ThingWorx collection names; only names the repository does not define
belong here. The draft records a count of referring entities and the first sorted referrer per
name. Extension minima come from Project `dependsOn` declarations, taking the highest version;
external project names come from the same declarations. The platform minimum comes first from
the highest non-empty Project `minPlatformVersion`, otherwise `[package] minimum_thingworx`,
otherwise the highest export `Entities` major/minor version. Its comment records the source.
Versions compare dotted numeric parts as integers, missing parts as zero, ignoring the first
`-` and everything after it: `10.1.0-b47` equals `10.1`. Non-numeric parts are refused.

`requirements check` checks the file offline. It blocks on undeclared dependencies or versions
below the source declarations, and reports unused declarations as stale notes. Keep intentional
runtime dependencies even when marked stale. `--server` additionally checks the platform,
extension package versions, external projects and entities; unreadable server results block.
The built-in `requirements` gate runs offline when the file exists; `[gates] advisory =
["requirements"]` makes its findings advisory (a gate that cannot run still fails closed).

Use `[optional.entities]` for a dependency that some target servers intentionally do not have.
It has the same collection keys as `[entities]`, counts as declared offline, and is a non-blocking
note when absent from `requirements check --server` or deploy; do not list one name in both tables.

## `twaco.toml`

A complete example. At least one `[[project]]` table is required, with its `name`. Every
other table is optional, but one you declare needs its own required keys: a `[[check]]` its
`name` and `command`, a `[[project.deploy.post_import]]` its `thing` and `service`. A key or
table twaco does not know is refused, with its name, so a misspelling never quietly runs fewer
gates; only free-form tables such as `deploy_parameters` take any key.

```toml
[solution]
name = "Acme"                 # used for package metadata and AGENTS.md
src = "src"                   # where sidecars live (default "src")
dist = "dist"                 # where bundles are written (default "dist")

[[extension]]
root = "extensions/AcmeKafka"  # Java extension checkout, relative to this solution
name = "Acme Kafka"            # optional label in catalog and reports

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

[format.script]
indent_width = 4              # 2 to 8
line_width = 100000           # 40 to 100000
quote_style = "double"        # "double" or "single"

[lint]
extends = ["house-v1"]        # checks-v1, traps-only-v1, or house-v1; later presets win

[lint.rules."traps.unbounded-loop"]
severity = "warn"             # "error", "warn", or "off"

[[lint.override]]
entity = "Things/Acme.Legacy.*"
service = "Import*"           # optional; otherwise every service
[lint.override.rules."order.main-code-below-a-helper"]
severity = "off"

[gates]
live = false                  # true: every `check` also has the server parse every script
advisory = []                 # built-in gates that report without failing, e.g. ["code order"]
structure_xsd = ""            # optional XSD, Eclipse plugin JAR/zip, or directory; also TWACO_STRUCTURE_XSD

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

[localization]
root = "localization"          # localization table exports (default)

[project.localization]
prefixes = ["Acme.App."]       # token prefixes this project owns (default: project name + .)

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

### `[[extension]]`

One optional read-only Java extension checkout. `root` is required, relative to the solution root,
and may sit beside it (`root = "../Acme_Extension"`), since twaco only reads it; prefer a relative
path so the setting works in every clone. `name` is the optional human label shown by `catalog`. twaco reads `configfiles/metadata.xml` (or
root `metadata.xml`), the matching Java source and DataShapes under `Entities/`. Its ThingTemplates,
ThingShapes, Resources and declared Things join the offline model for `impact`, `catalog`, types and
the mashup bindings gate. They are never discovered as workspace entities, so sync, rename, deploy,
format and every other writing command cannot edit the extension. An absent root, metadata file or
unreadable Java source is reported as skipped input; unrelated commands still run.

| Key | Default | Meaning |
| --- | --- | --- |
| `root` | required | Extension project directory, relative to the solution root. |
| `name` | `root` | Label shown as `[extension <name>]`. |

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

`framework = true` makes the project a Solution Framework building block whose deploy step runs as
the framework's own deploy (`PTC.Base.Manager.InitializeSolution`) runs it for one block, after
the project imports. `entry_point_thing` defaults to `<project>.EntryPoint` and the service is its
`DeployComponent`; `deploy_parameters` holds only `deploymentConfig`, the block's deploy
parameters. When the entry point's `componentDeploymentState` is `Configured` the step is
skipped (`deploy --redeploy-blocks` runs it anyway). Otherwise the parameters
`GetComponentDeployConfigurationParameters` marks required must be there, the entry point's
`ValidateComponentDeployConfigParams` must accept them, and `DeployComponent` must return true;
then the state becomes `Configured`. A `DeployComponent` that throws leaves it `Error`, one that
returns false leaves it as it was, and both fail the deploy. A plan reads the state and calls
none of these services. `InitializeSolution` itself is never called: it deploys every block on the
server that is not configured.

```toml
[project.deploy]
framework = true
deploy_parameters = { deploymentConfig = { databaseUser = "${profile:database_user}" } }
```

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

### `[[server_owned]]`

Declare per-server state once, so comparisons omit it and imports preserve it:

```toml
[[server_owned]]
entities = ["Groups/Acme.Admin_UG", "Groups/Acme.*_UG"]
parts = ["members", "run-time-permissions"]

[[server_owned]]
entities = ["Things/Acme.Database"]
parts = ["configuration-table:ConnectionInfo.password"]

[[server_owned]]
entities = ["Things/Acme.Manager"]
parts = ["configuration-table:DatabaseConnectionConfiguration"]

[[server_owned]]
entities = ["Things/Acme.Sample*"]
parts = ["property-values:speed,temperature"]
```

twaco adds one rule of its own: each project's `Things/<project>.EntryPoint` owns
`property-values:componentDeploymentState,deployComponentStatus,migrateComponentStatus,upgradeComponentStatus`,
a Solution Framework building block's lifecycle state, which the framework writes on the server.

Both lists are required and nonempty. Entity patterns use a known collection and a name;
`*` matches within the name, and other glob syntax is refused. Matching rules are combined.
Unknown kinds, malformed selectors and unknown keys stop configuration loading.

| Part | Owned content |
| --- | --- |
| `members` | A Group's direct `Members`, or the `Members` of an Organization's units. |
| `run-time-permissions` | The entity's direct `RunTimePermissions`. |
| `visibility-permissions` | The entity's direct `VisibilityPermissions`. |
| `property-values` | All direct `ThingProperties`. |
| `property-values:a,b` | Just the named property entries in that block. |
| `configuration-table:Table` | The named direct `ConfigurationTables` table, including its definition and rows. |
| `configuration-table:Table.column` | That column's cells in every row; its definition remains compared. |

Table, column and property selectors are exact, case-sensitive names, without whitespace or
selector separators (`:`, `,`, `.`, `/`, `*`, `\`). Unkeyed repeated rows correspond by position;
rows with a known unowned key use that key, as adopt does. Keep row and Organization unit
identities in source when owning only their cells or members. A transfer that would change
unowned structure is refused before import, including with `--force`; own the whole table if
the server also owns its row structure.

Status (including `--record`), guards and read-back compare both sides without these parts.
Applicable parts are fingerprinted in the hash: a policy change makes that entity's old
baseline absent; entities without rules keep their existing hashes. `entity status --detail`
names the parts; `doctor` warns when an entire rule matches no solution entity, and when it
names `members` but matches no Group or Organization, or property values but matches no Thing
or ThingTemplate.

For an existing server entity, push, deploy and `import <file.xml>` fetch its Exporter form and
splice each owned part into the outgoing copy: replace, insert, or drop it according to the
server. Server values
stay in memory, except when an existing backup option saves them. Deploy prepares all owned
copies before importing its project bundles. Plans and MCP results list these entities and
parts (`--json` on CLI push and deploy). Repository files are unchanged. A fresh server receives
the repository's initial values. Archive and source-control imports cannot be spliced: an archive
that would replace owned parts is refused; source-control import warns that it cannot preserve
them. A bundle imported by hand in Composer does not preserve them.
Adopt neither reports nor takes owned content from a designer's export.

An encrypted value cannot be carried: the Importer encrypts the Exporter's ciphertext again and
the Thing is left with a wrong password. So an owned `PASSWORD` column, a whole owned table with
one, or an owned `PASSWORD` property the entity defines is still left out of comparisons, but an
import sends the repository's value and clears the server's. `entity push` refuses such an
entity unless `--force`; set the value again after the import, as a deploy hook that supplies
the credentials does.

### Per-profile overrides

Committed profile overrides belong under `[profiles.<name>]`; credentials remain in the normal
untracked `.twaco/profiles/<name>.toml` file. Their `server_owned` entries are added after the
top-level list for that profile. A patch changes a dotted property in a Mashup's parsed
`mashupContent` JSON only in the copy sent to that profile:

```toml
[profiles.qa]
[[profiles.qa.server_owned]]
entities = ["Things/P.Database*"]
parts = ["configuration-table:ConnectionInfo"]

[[profiles.qa.patches]]
entity = "Mashups/P.Main"
path = "UI.Properties.Master"
value = "P.QaMaster"
```

Patches apply to Mashups only. Status for that profile compares the patched copy, and push and
deploy send it; the file on disk never changes. Intermediate patch objects must already exist;
the final key may be created. `entity pull` puts the working copy's value back (or removes the key
if it is absent there), so a pull from one profile never writes that profile's patch into the
repository. Deploy refuses, and `doctor --profile` fails, when a patch names no Mashup in the
workspace.

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

`[format.script]` controls the formatter for every service script. `indent_width` is an integer
from 2 through 8 (default `4`), and `line_width` is an integer from 40 through 100000 (default
`100000`). `quote_style` is `"double"` by default or `"single"`; `"single"` asks dprint to
prefer single quotes while still avoiding escapes when the other quote is cleaner.

`init` proposes `indent_width` and `quote_style` from the scripts it finds. `doctor` and the
formatting gate say when the configured setting differs from most scripts, before a format run
silently rewrites them.

Trailing commas stay disabled because Rhino rejects them in a call. Semicolons and braces stay
fixed so script traps and code order read a stable layout. Scripts still have no final newline so
`fmt` remains a fixed point of extraction.

Changing one of these values makes the next `twaco fmt` rewrite every script it affects. Because
whitespace inside a script is part of its content, every reformatted service then differs from the
server until it is deployed.

### `[lint]`

`[lint]` configures the formatting, script-trap, code-order and comment rules individually.
Without the table, twaco uses `checks-v1`: every formatting, script-trap and code-order rule, as
the gates have always run, and no comment rule. `house-v1`, which `twaco init` writes, adds the
comment header, budget and prose rules below. `traps-only-v1` enables only script traps.
`extends` is optional and defaults to `checks-v1`; when present it cannot be empty, and applies
presets in order so later presets win.

Each rule has severity `"error"`, `"warn"`, or `"off"`. `error` is a finding, `warn` is reported
but does not block, and `off` omits it. Rules may be changed in `[lint.rules]`, then in matching
`[[lint.override]]` entries in file order. An override requires an `entity` glob over
`Collection/Name`; `service` is optional and globs the service name. They use the same glob dialect
as the other selectors: `*` stays within one path component and `**` crosses components. Rule
options belong only in `[lint.rules]`; an override may set only severity. `*_extend` appends
case-insensitively deduplicated phrases or sets, while `*_replace` replaces the built-in list;
the two forms cannot be used together.

#### Suppressing one finding

Use `// twaco-lint-disable-next-line traps.for-in: verified legacy behaviour` to suppress named
rules on the next non-blank, non-`//` line, or `// twaco-lint-disable traps.for-in: verified legacy
behaviour` through `// twaco-lint-enable traps.for-in`. Disable directives need a non-blank reason,
and `format.*` and `comments.*` rules cannot be suppressed inline; use `[lint.rules]` or
`[[lint.override]]` instead. Comment rules are a house comment style: reword the comment when one
reports.
A malformed directive, or one that suppresses nothing, is itself reported.

| Rule | Gate |
| --- | --- |
| `format.unformatted` | formatting |
| `format.will-not-parse` | formatting |
| `traps.const-in-loop` | script traps |
| `traps.const-redeclared` | script traps |
| `traps.for-in` | script traps |
| `traps.indexed-rows` | script traps |
| `traps.nan-risk` | script traps |
| `traps.read-before-assign` | script traps |
| `traps.unbounded-loop` | script traps |
| `traps.suppression` | script traps |
| `order.main-code-below-a-helper` | code order |
| `order.declaration-between-helpers` | code order |
| `order.suppression` | code order |
| `comments.budget.jsdoc-prose` | comment budget |
| `comments.budget.line-run` | comment budget |
| `comments.budget.ratio` | comment budget |
| `comments.header.contract` | comment header |
| `comments.header.first-statement` | comment header |
| `comments.header.wrap` | comment header |
| `comments.prose.em-dash` | comment prose |
| `comments.prose.history` | comment prose |
| `comments.prose.length` | comment prose |
| `comments.prose.bundled` | comment prose |
| `comments.prose.hedge` | comment prose |
| `comments.prose.condition` | comment prose |
| `comments.prose.passive` | comment prose |
| `comments.prose.nouns` | comment prose |
| `comments.prose.synonym` | comment prose |

In `house-v1`, the three header rules, every comment budget rule and `em-dash`, `history`,
`length`, `bundled`, and `hedge` are errors; `condition`, `passive`, `nouns`, and `synonym` are
warnings.

#### Service headers

A service script starts with a JSDoc header that its `definition.xml` holds it to: one
`@function` naming the service, one `@param` per input in `ordinal` order (the XML lists inputs
alphabetically, so the order is the attribute's), and one `@returns`, which names the full
DataShape when the result has one. The first statement follows the header directly.

When the contract rule is on, `entity edit ... service add` writes the header for a new service from
its definition: the description as prose, then `@function`, `@param {type} name - description` lines
and `@returns`. `twaco lint fix-headers` plans the same repair for existing scripts and `--apply`
writes it under the workspace lock. It sets `@function` to the service's name and otherwise moves
each existing tag whole, with its text as written, generates only what is missing, removes a
`@param` that names no input and a second `@returns`, and never rewrites prose; a comment above the
first statement or a line too long is reported for a person to fix. Services whose contract rule
is off are left alone.
Run `twaco sync` afterwards to write the scripts into the entity XML.

| Rule option | Type | Default | Meaning |
| --- | --- | --- | --- |
| `comments.budget.jsdoc-prose.max_lines` | integer ≥ 0 | `2` | Header prose lines allowed before tags. |
| `comments.budget.line-run.max_lines` | integer ≥ 1 | `2` | `//` lines allowed in one run; a blank line does not end a run, code does. |
| `comments.budget.ratio.max_ratio` | float 0–1 | `0.40` | Largest comment share. |
| `comments.budget.ratio.min_code_lines` | integer ≥ 0 | `15` | Code lines needed before judging the ratio. |
| `comments.prose.length.instruction_max_words` | integer ≥ 1 | `20` | Word cap for imperative sentences. |
| `comments.prose.length.description_max_words` | integer ≥ 1 | `25` | Word cap for other sentences. |
| `comments.prose.length.imperatives_extend` / `_replace` | string array | built-in list | Add or replace imperative words. |
| `comments.prose.history.phrases_extend` / `_replace` | string array | built-in list | Add or replace history phrases. |
| `comments.prose.hedge.phrases_extend` / `_replace` | string array | built-in list | Add or replace hedge phrases. |
| `comments.prose.synonym.sets_extend` / `_replace` | string-array array | built-in sets | Add or replace equivalent-word sets. |
| `comments.prose.nouns.max_run` | integer ≥ 1 | `3` | Nouns allowed before the next one reports. |
| `comments.header.contract.require_result_shape` | boolean | `true` | `@returns` names the result DataShape when there is one. |
| `comments.header.wrap.max_columns` | integer ≥ 40 | `100` | Longest header line; generated headers wrap at it. |

```toml
[lint.rules."comments.prose.length"]
description_max_words = 30
imperatives_extend = ["deploy"]

[lint.rules."comments.prose.hedge"]
phrases_replace = ["basically", "simply"]
```

`[gates].advisory` still makes its whole gate report-only, whatever the rule severities say.
Run `twaco lint --print-config` to see each rule's resolved severity and where it came from.
With a `[lint]` table, a `[[check]]` hook cannot use the name of a lint gate that the table
enables (`formatting`, `script traps`, `code order`, `comment budget` or `comment prose`); rename
or remove that hook, since twaco then runs that gate itself. Without one, every hook name that
loaded before still loads. The comment gates appear in `check` only when `[lint]` enables one of
their rules.

### `[gates]`

`live = true` makes every `twaco check` also send every script to the server's parser, as
`check --live` does. That gate fails closed: without a reachable server, `check` fails.
`deploy` always parses on the server, whatever this says.

`structure_xsd = "path"` enables the offline, advisory `structure` gate. The path is relative
to the solution root unless `TWACO_STRUCTURE_XSD` overrides it. It may name an XSD, the Eclipse
plugin JAR (whose nested platform-common JAR holds it), its distribution zip, or a directory
holding any of them. twaco reads the schema at check time;
it does not copy it into the repository.

`advisory` names built-in gates whose findings are reported (as `warn`) without failing
`check` or blocking `deploy`: `line endings`, `sidecars`, `formatting`, `script traps`,
`code order`, `project`, `project dependencies`, `configuration tables`, `mashup bindings`, `mashup content`, `contained mashups`, `localization tokens`,
`result shapes`, `networks`, `requirements`, `ai references`, `structure`, and the comment gates
`comment budget`, `comment prose` and `comment header` when `[lint]` enables them. The live parse is not among them: `deploy` always has the server
parse every script, and stops on a script it refuses. It is for a solution adopting twaco over code written
before those gates existed; take a gate off the list once its findings are fixed. A gate that
cannot run still fails, and a name that is not a gate is refused.

### `[validate]`

A service implemented with no definition of its own is legitimate when its definition comes from a
shape or template. `check`, comment headers and `types` resolve that definition on a template or
shape of the solution or of a configured `[[extension]]` the entity inherits from, or in the
platform cache that `twaco types --platform` fills from the server (the Solution Framework's
`DeployComponent` on `PTC.Base` templates, for example). An incomplete chain or a missing cache
leaves the override unresolved; `inherited_overrides` lists those exceptions as `Entity.Service`
names, and a bare `Service` allows it on every entity. It never suppresses an ancestry that is
fully known but no longer defines the service: revert that override or restore the parent's
definition.

### `[unused]`

What `twaco unused` treats as in use without anything in the repository referring to it:

- **`keep`:** names or `Collection/Name` patterns, as globs (see [Globs](#globs)). A
  database connection Thing that the platform calls, an entity a REST client or a connected
  system uses, anything reached from outside the repository, belongs here. A pattern that matches
  no entity is reported, so a typo does not silently keep nothing.

Mashups, things a deploy runs (`[project.deploy]`) and anything that runs on events
(subscriptions, Timers, Schedulers) are entry points already and need no entry.

### `[call]`

MCP `call` without `dry_run` runs a service known to only read and plans any other. twaco knows
the platform's reads (InfoTableFunctions, SearchFunctions, `GetPropertyValues`,
`QueryDataTableEntries` and the like, from the ThingWorx Javadoc), but never trusts that list for
a service name the repository defines or overrides (or while any repository file cannot be read).
It goes by name, so a server-only entity outside the repository that defines a service of a
listed name is trusted too. List your own reads here:

```toml
[call]
read_only = ["Things/P.Orders.GetOrders", "Things/P.*.GetSummary"]
```

Each entry is `Collection/Name.Service`; the name may be a glob as in `[[server_owned]]`. A result
that ran this way says `read_only: "platform"` or `"twaco.toml"`. `dry_run: true` still plans and
`dry_run: false` still calls; a call with `lang` always needs `dry_run: false`, because it sets the
test user's language preference. The CLI's `twaco call` always calls.

### `[adopt]`

How `twaco adopt` reads a designer's export:

- **`ignore_paths`:** node paths never reported as a change, written
  `Collection/Entity/node/path`, as globs (see [Globs](#globs)). Use it
  for values that differ on every export by design, such as live property values or a
  password encrypted with the designer's key.
- **`generated_services`:** `Entity.Service` names whose body this repository generates, so
  the repository wins.

### Globs

Every pattern in `twaco.toml` (`[unused] keep`, `[adopt] ignore_paths`, `[[server_owned]]`,
`[call] read_only`, permission policies, bundle exclusions) is one dialect. It matches the whole
text. `*` stands for any run of characters within one `/` step, and `**` crosses steps. Every other
character is literal, brackets included, so `ConfigurationTable[ConnectionInfo]` means itself.

### `[types]`, `[help]`, `[repositories]`, `[localization]`, `[project.localization]`, `[package]`, `[knowledge]`

| Key | Meaning |
| --- | --- |
| `types.tsc` | The TypeScript compiler command for `types --check`, before twaco's own `-p` arguments. Default: `tsc` (`tsc.cmd` on Windows) on `PATH`. It receives no credentials, has a 600-second timeout, each output stream is capped at 16 MiB, and must report twaco's known type-error canary. TypeScript 7 reports fewer JavaScript findings than 5.x, such as a value that may be null. |
| `help.version` | The help center release `twaco help` reads, such as `10.1`. Default: the server's version, else the newest. |
| `repositories.root` | The folder holding one subfolder per FileRepository, for `repo status`, `push` and `pull`. Default `filerepository`. A pull writes only inside the solution. |
| `localization.root` | The folder holding localization table files. Default `localization`. Edits write only inside the solution. |
| `project.localization.prefixes` | Token-name prefixes this project owns. Default the project name followed by `.`. |
| `package.*` | Extension metadata for `twaco package extension`: `version` (1.0.0), `group` (the solution's name), `vendor`, `minimum_thingworx` (9.0.0). Each project's `artifactId` is its name in lower case. |
| `knowledge.paths` | Markdown files or folders `twaco guide` reads besides its built-in topics. Each must exist and stay inside the solution. Default: AGENTS.md, CLAUDE.md and `docs/`, where they exist. |

### `[test]` and `[[test.user]]`: test users

The accounts `twaco users seed` creates on a test server, and the cases `twaco test` signs in as.

```toml
[test]
project = "Acme.App.TestUsers"   # optional: the project the users are created in
cases = "tests"                  # optional: the folder of case files (*.toml); default tests

[[test.user]]
name = "vera.viewer"
groups = ["Acme.App.Viewers_UG"] # the groups it belongs to: its role

[[test.user]]
name = "ed.editor"
groups = ["Acme.App.Viewers_UG", "Acme.App.Editors_UG"]
```

Every test user has the one password the profile holds as `test_password`; it is never in this
file, and never printed. A profile must also say `allow_test_users = true` before `users seed`,
`users delete` or `twaco test` act on its server: a test account is a way in, and has no place on
a customer's server.

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
app_key = "..."                           # optional; alone, it replaces username and password
database_password = "..."                 # any other key, for ${profile:key} placeholders
allow_test_users = true                   # a test server: users seed and twaco test may act on it
test_password = "..."                     # the password of every [[test.user]]
sdk = "C:/ThingWorx/Extension-SDK"          # optional: Extension SDK Javadoc for `twaco javadoc`
```

Commands use the profile `default` unless given `--profile <name>`, or `profile` over MCP.
`twaco javadoc` uses a profile's optional `sdk` only after an explicit `--sdk` (or MCP `sdk`)
and `TWACO_SDK`; it may name the SDK download zip, an extracted SDK folder, or the
`thingworx-javadocs-*.jar` itself. The jar is read in place and is never copied into twaco's
cache or the repository.
Environment variables override a profile's fields, or replace the file entirely:
`TWACO_URL`, `TWACO_USERNAME`, `TWACO_PASSWORD` and `TWACO_APP_KEY` (or the `TWX_` spellings).

twaco signs in with the username and password (Basic). A profile with an `app_key` and neither
a username nor a password signs in with the key instead, sent as an `appKey` header; the
server's `AllowApplicationKeyInURL` setting does not apply to a header. A profile with both keeps
Basic, so `twaco appkey create ... --save <profile>` never changes who an existing profile acts
as. A key acts as its user, with that user's permissions, until it expires. On ThingWorx 10.2
the Importer, and so every deploy, accepts a key; older servers have been seen to refuse one
with 401 or 403, and a profile with a username and password is the answer there. A profile is only read by a command that talks to a server,
so offline commands work without one. Profile values are never printed; `twaco doctor`
shows which file or variables a profile resolved from.

Keep profiles, and the backups a delete or a forced push saves, out of git: a backup holds the server's
copy of an entity as it is, a database Thing's password included. A solution's `.gitignore` should
hold the lines below. `twaco init --write` and `twaco init --agents` add the missing ones, and
`twaco doctor` warns when any is missing. An ignore line does not untrack a file committed before it, so
`twaco doctor` also asks git whether it tracks anything under `.twaco/profiles`, `.twaco/backups` or
`.twaco/transactions` or `.twaco/handoffs`: a tracked backup is a warning, a tracked profile a failure (history keeps it,
so change the password it holds).

```gitignore
.twaco/profiles/
.twaco/lock
.twaco/lock.holder
.twaco/backups/
.twaco/handoffs/
.twaco/transactions/
.twaco/types/
.twaco/platform.json
**/services/*/jsconfig.json
**/services/*/twaco-globals.d.ts
```

Commit `.twaco/baselines/`. It holds one file per server profile (`<profile>.json`), each
recording every entity's state on that server at the last deploy or push (and what
`entity status --record` adopted), so a later deploy can tell a teammate's server-side change from
yours. The former `.twaco/baseline.json` is read as the `default` profile's until that profile next
records; other profiles begin without a baseline, and `entity status --all --record --profile
<name>` gives one to every entity that already matches that server.

Each recorded hash carries the version of the comparison form it was made with (`v7:...`). When a
new twaco compares differently, an entry from the old version counts as unrecorded rather than
as a change on both sides: an entity whose two sides match reads `no-baseline-same` and is
recorded again by its next deploy, and `twaco doctor` counts the old entries.
`twaco entity status --all --record` records again, at once, every entity that matches the server.
