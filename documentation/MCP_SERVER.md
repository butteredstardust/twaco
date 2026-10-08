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

Server tools use the same [profiles](CONFIGURATION.md#server-profiles) as the command line;
pass `profile` to choose one.

## How the tools behave

- **Dry runs by default.** Every tool that changes a server takes `dry_run`, which defaults to
  `true`: the result is the plan. The agent passes `dry_run: false` to act. That includes
  `call`, because twaco cannot know whether a service writes.
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

## Errors and codes

An errored tool result has `{"error":"the unchanged message","code":"stable_code"}` in its
text and structured content. The code identifies the next action.

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
| `check` | no | Every gate; `live: true` also parses every script on the server |
| `types` | workspace | Generate editor declarations, type-check every service, or cache the platform's |
| `extract` | workspace | Entity XML to sidecars |
| `sync` | workspace | Sidecars to entity XML |
| `fmt` | workspace | Format service scripts; `check: true` only reports |
| `status` | baseline | Compare entities with the server and the baseline |
| `push` | server | Import one entity, refusing a conflict |
| `deploy` | server | The full deploy, in project order, with live parse and conflict checks |
| `call` | server | Call a service; `with_logs` adds what it logged |
| `adopt_report` | no | What a designer's Composer export really changes |
| `adopt_apply` | workspace | Write the mechanical half of a designer's export |
| `entity_restore` | server | List backup sets, or import one back (all of it or named entities), plan by default |
| `entity_carry` | server; workspace for the ledger | Copy run-time, design-time and visibility permissions from renamed entities to the new ones, plan by default |
| `entity_delete` | server; workspace for the ledger | Delete old entities in dependency order, guarded, plan by default |
| `db_run` | server | Run one atomic SQL script through a throwaway Database Thing; plan by default |
| `datatable_copy` | server | Copy a DataTable's rows into the table that replaced it, mapped by name, ledger or `map`, plan by default |
| `db_clean` | server | Find and delete leftover temporary `ZZ.Twaco.Sql.*` Database Things, plan by default |
| `db_query` | server | Run read-only SQL through a throwaway Database Thing |
| `rename` | workspace | Rename an entity, a building block (a name prefix), a DataShape field, a service or a property across the repository; `dry_run` defaults to true; the result's `plan_digest`, passed back with `dry_run: false`, applies exactly the plan that was reviewed |
| `move_member` | workspace | Move or copy a service or property between Things, templates and shapes; reports the callers it breaks; plan by default |
| `new_building_block` | workspace | Create a building block as files and register its project, plan by default |
| `retemplate` | workspace | Change a template, base template or implemented shapes, with a report of what is gained and lost; plan by default |
| `config_table` | server | Read, diff or restore one Thing's configuration table |
| `logs` | no | Read a server log |
| `log_level` | server | Read or change a log's level |
| `settings` | no | The server's subsystem settings |
| `repo` | no | List and read file repositories, compare with the solution |
| `repo_write` | server; workspace for pull | Put, mkdir, rm, mv, push and pull for file repositories |
| `extensions` | no | The server's extension packages |
| `extension_write` | server | Validate, install or remove an extension package |
| `bundle` | workspace, with `dry_run: false` | Whether the configured bundle is current, or rebuild it |
| `search` | no | Find server entities by text in a name or description, type and project |
| `entity_get` | no | One entity's XML as the server has it, returned rather than written |
| `export` | workspace* | Export from the server into a file of the solution |
| `import` | server | Import a file of the solution, or a source-control tree |
| `package` | workspace | Bundles, a source-control zip or extension packages, offline |
| `catalog` | no | Every service in the repository, with its signature and origin |
| `impact` | no | What changing an entity, or one service, property or field of it, would reach, each dependent at the strength of its weakest reference |
| `unused` | no | Entities no entry point reaches (advisory; deletes nothing), with what still names them |
| `docs` | no | The solution written down: projects and deploy order, inheritance, services, DataShapes and the references to review, as JSON and Markdown |
| `doctor` | no | What resolved, what is reachable and what is missing |
| `guide` | no | twaco's workflow, the platform's quirks and the solution's own documents |
| `help_search` | no | Search the ThingWorx Platform help center |
| `help_page` | no | Read a help page as Markdown |
| `javadoc` | no | Search or read the ThingWorx Platform Java API |

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
  backup) or reads (`db_run`, `import`) is a plain path inside the solution, and an existing
  file is replaced only with `overwrite`. The command line takes any path.
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
