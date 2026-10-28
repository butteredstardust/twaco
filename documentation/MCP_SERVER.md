# MCP server

`twaco mcp` serves twaco to an AI agent over the [Model Context Protocol](https://modelcontextprotocol.io),
on standard input and output, as tools covering most of the command line's work: the
repository, deploys, the server and the knowledge. `init` and `update` are command-line only;
[Where the tools differ from the command line](#where-the-tools-differ-from-the-command-line)
says why, and what else differs on purpose.

## Set up

The server works on the solution in its working directory, or in `TWACO_ROOT` when set. It
looks for `twaco.toml` on every call, so an edit to it applies without a restart.

**Claude Code**, from the solution's root:

```sh
claude mcp add twaco -- twaco mcp
```

**Codex** (`~/.codex/config.toml`):

```toml
[mcp_servers.twaco]
command = "twaco"
args = ["mcp"]
env = { TWACO_ROOT = "/path/to/solution" }
```

**Any other client** (Cursor, Claude Desktop, VS Code and others) takes the same three
things: the command `twaco`, the argument `mcp`, and `TWACO_ROOT` when the client does not
start it in the solution's folder.

**Logs.** An MCP client often hides the server's stderr, so send logs to a file. Add
`--log-file` to the server arguments, or set `TWACO_LOG_FILE` in its environment:

```sh
claude mcp add twaco -- twaco mcp --log-file /tmp/twaco-mcp.log
```

```toml
[mcp_servers.twaco]
command = "twaco"
args = ["mcp", "--log-file", "/tmp/twaco-mcp.log"]
```

Without these options the server writes no log. Stdout carries only JSON-RPC. The log has one
line per message and one per server request. It never holds a tool's `arguments`. See
[Diagnostic logs](CONFIGURATION.md#diagnostic-logs).

Server tools use the same [profiles](CONFIGURATION.md#server-profiles) as the command line;
pass `profile` to choose one.

**Fewer tools.** A client keeps every listed tool's definition in the agent's context on every
turn, and all 88 come to about 160 KB with their output schemas (about 104 KB for a client on a
revision before `2025-06-18`, which gets none). `--tools` (or `TWACO_MCP_TOOLS`) lists and runs only
the groups named, comma-separated; any other tool is unknown to that server. `core` alone is about
35 KB (20 KB without output schemas).

| Group | Tools |
| --- | --- |
| `core` | projects, types, check, status, sync, extract, fmt, push, deploy, bundle, catalog, impact, contract_diff, trace, unused, search, entity_get, docs, doctor, guide |
| `entity` | entity_pull, entity_delete, entity_restore, entity_carry, entity_new, network, network_write, entity_edit, new_building_block, block, block_version, export, import, package |
| `refactor` | rename, move_member, reproject, retemplate, adopt_report, adopt_apply, handoff |
| `mashup` | mashup_new, mashup_templates, mashup_add, mashup_add_contained, mashup_bind, mashup_on, widgets, mashup_param, mashup_recipe, mashup_event, mashup_validate, mashup_describe |
| `permissions` | permissions, requirements_init, requirements_check, permissions_init, permissions_audit, permissions_apply, permissions_push |
| `data` | db_run, db_query, datatable_copy, db_clean, config_table, call, test |
| `server` | init_from_server, logs, log_level, repo, repo_write, localization, localization_write, users, users_write, appkey, extensions, ext_check, ext_build, ext_new, ext_add, ext_deploy, extension_write, settings |
| `reference` | help_search, help_page, javadoc |

```sh
claude mcp add twaco -- twaco mcp --tools core,mashup
```

## How the tools behave

- **Dry runs by default.** Every tool that changes a server takes `dry_run`, which defaults to
  `true`: the result is the plan. The agent passes `dry_run: false` to act. That includes
  `call`, because twaco cannot know whether a service writes. `db_query` has none: it runs
  read-only SQL through a temporary Thing it deletes again, and `db_clean` removes one a crash
  left behind.
- **Summaries first.** A result is a compact summary: counts, the first few items, what to ask
  for next. `detail: true` returns everything. A check over a large solution costs a few
  hundred tokens, not tens of thousands.
- **Arguments are validated** against each tool's schema; an unknown or mistyped argument is
  an error, not ignored.
- **Writes to twaco's managed files take the workspace lock** (extract, sync, fmt, types, an
  applied deploy, push, adopt or repo pull, and `status` with `record`), so an agent and a
  person cannot interleave them. Files an agent names for `export` and `package` are written
  without it.
- **A failure is an error** (`isError`), with what went wrong in the message.
- **Progress for long calls.** Add `_meta.progressToken` to a `tools/call` request. Twaco then
  sends `notifications/progress` lines before the response. This works for `deploy`, `status`,
  `types`, `import`, `export`, `extension_write`, `check` (with `live`), `push`, `permissions`,
  `permissions_audit`, `permissions_apply`, `permissions_push`, `repo` and `repo_write`. Each notification has `progress` (steps
  done, always rising), `total` (when known) and `message` (a phase name or an entity name).
  Twaco sends at most about ten per second, and one at the end of each phase. Without a token,
  twaco sends none.
- **One call at a time, and cancellation.** Tool calls run in the order they arrive, one after
  another, while the server keeps answering `ping` and `tools/list`. A
  `notifications/cancelled` for a call that has not started means it never runs. A call
  already running finishes, since stopping a server write halfway would leave the server
  partly changed, but it sends no more progress and no response.
- **Limits.** At most 32 calls wait behind the running one; a call past that is answered at once
  with JSON-RPC error `-32000` (twaco is busy) and does not run. A message line longer than
  32 MiB, its line ending included, is answered with `-32000` and skipped; the messages after it
  are read as usual.

## Errors and codes

An errored tool result has `isError: true` and `{"error":"the unchanged message","code":"stable_code"}`
in its text block. It has no `structuredContent`, which is kept for results that match the tool's
`outputSchema`. The code identifies the next action.

| Code | Meaning and next action |
| --- | --- |
| `invalid_arguments` | The request is invalid; correct the arguments. |
| `unknown_entity` | The named entity is absent; inspect the repository. |
| `ambiguous` | More than one entity matches; disambiguate it. |
| `already_exists` | A target already exists; choose another target. |
| `guard_refused` | A safety guard refused; ask a person. |
| `gate_failed` | A gate failed; inspect and fix the repository. |
| `stale_plan` | State changed since planning; re-plan. |
| `workspace_locked` | Another write is active; retry later. |
| `server_conflict` | Server state conflicts with the plan; re-plan. |
| `server_unreachable` | The server could not be reached; retry later. |
| `server_error` | The server returned an error; inspect its response. |
| `not_verified` | A write could not be verified; inspect server state. |
| `rollback_failed` | Rollback was incomplete; ask a person. |
| `io_error` | A local read or write failed; inspect the repository. |
| `invalid_data` | Repository or server data is invalid; inspect it. |
| `unclassified` | No typed classification is available; inspect the message. |

## The tools

| Tool | Writes | What it does |
| --- | --- | --- |
| `projects` | no | The solution's projects, their roots and deploy order |
| `check` | no | Every gate; `live: true` also parses every script on the server; `detail: true` adds each finding and each gate's notes |
| `types` | workspace | Generate editor declarations, type-check every service, or cache the platform's |
| `extract` | workspace | Entity XML to sidecars |
| `sync` | workspace | Sidecars to entity XML |
| `fmt` | workspace | Format service scripts; `check: true` only reports |
| `status` | baseline | Compare entities with the server and the baseline |
| `push` | server | Import one entity, refusing a conflict |
| `entity_pull` | workspace | Take server-changed entities into the repository, extract their sidecars and record their baselines; plan by default |
| `init_from_server` | workspace | Start a repository, or add projects to this one, from server projects; a plan until `apply: true` |
| `deploy` | server | The full deploy, in project order, with live parse, server requirements and conflict checks; a deploy the gates stop answers `stage: "gates"` and lists each blocking gate's findings |
| `call` | server | Call a service; `with_logs` adds what it logged |
| `test` | server | The service cases of `tests/*.toml`, called as each test user; a dry run lists and checks them |
| `adopt_report` | no | What a designer's Composer export really changes, including retyped service arguments and defaults |
| `adopt_apply` | workspace | Write the mechanical half of a designer's export |
| `handoff` | workspace | List recorded collaborator handoffs, or record delivered exports as the base of a later three-way adopt; plan by default |
| `entity_restore` | server | List backup sets, or import one back (all of it or named entities), plan by default |
| `entity_carry` | server; workspace for the ledger | Copy run-time, design-time and visibility permissions from renamed entities to the new ones, plan by default |
| `entity_delete` | server; workspace for the ledger | Delete old entities in dependency order, guarded, plan by default |
| `db_run` | server | Run one atomic SQL script through a throwaway Database Thing; plan by default |
| `datatable_copy` | server | Copy a DataTable's rows into the table that replaced it, mapped by name, ledger or `map`, plan by default |
| `db_clean` | server | Find and delete leftover temporary `ZZ.Twaco.Sql.*` Database Things, plan by default |
| `db_query` | server | Run read-only SQL through a throwaway Database Thing |
| `rename` | workspace | Rename an entity, a building block (a name prefix), a DataShape field, a service or a property across the repository; `dry_run` defaults to true; the result's `plan_digest`, passed back with `dry_run: false`, applies exactly the plan that was reviewed |
| `move_member` | workspace | Move or copy a service or property between Things, templates and shapes; reports the callers it breaks; plan by default |
| `reproject` | workspace | Move entities to another project of the solution, naming each reference the move puts outside the deploy order; plan by default |
| `new_building_block` | workspace | Create a building block as files and register its project, plan by default |
| `block` | no | The solution's building blocks, or one; with server true the framework's version, state, deploy order and deploy parameters |
| `block_version` | workspace | Raise or set a block's componentVersion and Project packageVersion together, plan by default |
| `entity_new` | workspace | Create an entity as a repository XML file (Things, templates, shapes, DataShapes, DataTables, styles, menus, Networks, ...); plan by default |
| `entity_edit` | workspace | Change an entity as Composer would: properties, script services, subscriptions, shapes, timers, schedulers, styles, states, menus, media; plan by default |
| `network` | no | List Networks, show an asset hierarchy, or diff repository and server connections |
| `network_write` | workspace | Add, move or remove a Network connection, import an asset CSV, or pull the server's connections; plan by default |
| `mashup_new` | workspace | Create a responsive mashup from one of the embedded layout templates; plan by default |
| `mashup_templates` | no | The embedded layout templates, or one template's slots |
| `mashup_add` | workspace | Add a catalogue or extension-project widget to a mashup; plan by default |
| `mashup_add_contained` | workspace | Add a repository mashup as a contained mashup; plan by default |
| `mashup_bind` | workspace | Add one data binding, by the addresses `mashup_describe` prints; plan by default |
| `mashup_on` | workspace | Run a data call or widget service when a mashup event fires; plan by default |
| `mashup_param` | workspace | Add, change or remove a mashup parameter, with every container's copy; plan by default |
| `mashup_event` | workspace | Give a mashup an event its containing mashups can react to; plan by default |
| `mashup_recipe` | workspace | Write a common piece of wiring (enable-when, show-when, text, message, confirm, popup, ...) in one edit; plan by default |
| `mashup_validate` | no | Validate mashups offline: structure, bindings and events against the widget catalogue |
| `mashup_describe` | no | A mashup's widget tree, service calls, bindings and events, without its XML |
| `widgets` | no | The embedded ThingWorx 10.2 widget catalogue, or one widget's properties, events and services |
| `retemplate` | workspace | Change a template, base template or implemented shapes, with a report of what is gained and lost; plan by default |
| `permissions` | no | Compare entities' permissions in the repository with the server's |
| `permissions_init` | workspace | Draft a `permissions.toml` for each project without one, from what it grants today; plan by default |
| `permissions_audit` | no | Check each project's `permissions.toml` against the entity XML; `server: true` also against the server |
| `permissions_apply` | workspace | Write each project's policy into its entity XML; plan by default |
| `permissions_push` | server | Make the server's permission sets exactly the repository's; plan by default |
| `requirements_init` | workspace | Draft `requirements.toml` from the solution's external dependencies; plan by default |
| `requirements_check` | no | Check `requirements.toml` against the repository; `server: true` also verifies the server, read-only |
| `config_table` | server | Read, diff or restore one Thing's configuration table |
| `logs` | no | Read a server log |
| `log_level` | server | Read or change a log's level |
| `settings` | no | The server's subsystem settings |
| `repo` | no | List and read file repositories, compare with the solution |
| `repo_write` | server; workspace for pull | Put, mkdir, rm, mv, push and pull for file repositories |
| `localization` | no | Compare localization table files with the server |
| `localization_write` | server or workspace | Pull, push, create and edit localization tables; dry run by default |
| `users` | no | The declared test users on the server: present, signing in, in their groups |
| `users_write` | server | Seed or delete the test users; plan by default |
| `appkey` | server | Create an application key (plan by default) or show one |
| `extensions` | no | The server's extension packages |
| `ext_check` | no | A Java extension project's metadata.xml against its source, offline; a widget project's package.json |
| `ext_build` | workspace | Build an extension project's package zip (npm, Gradle, Ant, or packed by twaco) and check what it holds |
| `ext_new` | extension folder | Lay out a Java extension project (Gradle build and wrapper, metadata.xml, SDK jars) or, with widget, a package.json widget project; dry run by default |
| `ext_add` | extension folder | Add a template, shape, resource, widget or jar with its metadata.xml entry; dry run by default |
| `ext_deploy` | server | Build an [[extension]], validate its package, install it with a version bump; dry run by default |
| `extension_write` | server | Validate, install or remove an extension package |
| `bundle` | workspace, with `dry_run: false` | Whether the configured bundle is current, or rebuild it |
| `search` | no | Find server entities by text in a name or description, type, project and when they last changed |
| `entity_get` | no | One entity's XML as the server has it, returned rather than written |
| `export` | workspace* | Export from the server into a file of the solution |
| `import` | server | Import a file of the solution, or a source-control tree |
| `package` | workspace | Bundles, a source-control zip or extension packages, offline |
| `catalog` | no | Every service in the repository, with its signature and origin |
| `impact` | no | What changing an entity, or one service, property or field of it, would reach, each dependent at the strength of its weakest reference; with `server`, also what the server says refers to it |
| `trace` | no | How each caller uses one service: what runs the call, what sets each parameter, where each result goes |
| `contract_diff` | no | Compare the public contract with a git revision, with the current callers of what broke; offline |
| `unused` | no | Entities no entry point reaches (advisory; deletes nothing), with what still names them |
| `docs` | no | The solution written down: projects and deploy order, inheritance, services, DataShapes and the references to review, as JSON and Markdown |
| `doctor` | no | What resolved, what is reachable and what is missing |
| `guide` | no | twaco's workflow, the platform's quirks and the solution's own documents |
| `help_search` | no | Search the ThingWorx Platform help center |
| `help_page` | no | Read a help page as Markdown |
| `javadoc` | no | Search or read the ThingWorx Platform Java API, or one service as a script names it |

\* `export` writes a file into the solution; its source-control action writes into a server
file repository instead, and that one is a dry run by default.

Each tool's description and schema, from `tools/list`, tell the agent the rest.

## Where the tools differ from the command line

Every command has a tool except these two, each for a reason:

- **`init`** sets up a solution once: it writes `twaco.toml`, the `.gitignore` lines and the
  agent instruction files that configure the agent itself. A person runs it before an agent is
  connected.
- **`update`** replaces the twaco binary, and an agent must not replace the server it talks to.

These differ on purpose:

- **`call` plans by default** over MCP and acts on the command line: a person typing `twaco call`
  means it, and twaco cannot tell which services write.
- **`adopt` is two tools.** `adopt_report` compares and writes nothing; `adopt_apply` writes
  without a dry run, as one transaction. `reverts` in the report is what `--fail-on-revert` fails on.
- **A rename applies only with the plan's `plan_digest`**, so an agent cannot apply a plan the
  repository has moved on from.
- **Files stay in the solution.** A file a tool writes (`export`, `package`, a `config_table`
  backup) or reads (`db_run`, `db_query`, `import`, a `config_table` restore) is a path inside
  the solution, links resolved. An export or package replaces an existing file only with
  `overwrite`; a backup is never replaced. The command line takes any path.
- **Results are bounded.** `entity_get` cuts the XML at `max_chars` (export writes it whole);
  `repo` returns a text file's content up to a limit and no bytes for a binary one
  (`repo_write` with `pull` brings files into the solution); `logs` returns 100 entries unless
  asked for more, as the command line does.
- **SQL can be given inline** to `db_run`, so an agent needs no temporary file; the command line
  reads a file.
- **`status` returns what it could read** with `ok: false` and the unreadable files listed,
  where the command line stops; recording still refuses unless every file was read.
- **No exit codes, `--json` or update notice.** Every result is JSON with `ok`, and an error has
  a stable `code`; stdout carries only the protocol.

## The agent's instructions

On connect, the server tells the agent what twaco is, that writes are dry runs, and where to
start: `projects`, then `check`, then `status` with `all: true` or one `entity` (it needs
one or the other). It also says to search `guide` before a live
import, a hand-written binding or a configuration-table change.

For an agent to work well on a solution, it also needs what the entities cannot say: what the
solution is for, where its data lives, and what is fragile. `twaco init --agents` writes an
`AGENTS.md` for people to fill in, and a `CLAUDE.md` that points to it. `guide` searches both,
together with `docs/`.
