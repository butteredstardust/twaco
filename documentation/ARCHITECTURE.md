# Architecture

How twaco is built, for anyone changing it. It is one Rust crate: a library (`src/lib.rs`,
`src/core/`) with two thin front ends, the command line (`src/main.rs`) and the MCP server
(`src/mcp.rs`). Every behaviour lives in `core`, so the two front ends stay thin and are meant to agree
wherever they share a feature.

## Principles

These decide most design questions. A change that breaks one needs a very good reason.

1. **Never re-serialise an entity.** ThingWorx exports are compared, diffed and reviewed as
   bytes. twaco tokenises XML into byte spans (`scan`) and replaces only the span it means to
   change (`splice`), so a no-op edit is the identity function, byte for byte. No XML library
   round trip, which would change attribute order, whitespace, empty-element style and CDATA
   boundaries. Scripts follow the same rule: an ECMAScript parser (swc) reads them for byte
   spans only, and its output is never printed back.
2. **The server is someone else's.** Every command that writes to a server plans unless told
   to act; CLI `call` is the exception, since twaco cannot know whether a service writes. Deletes
   are dependency-guarded and confirmed absent. Conflicts are refused rather than resolved. A gate that cannot run fails closed.
3. **Summaries first.** Results, on the command line and over MCP, lead with counts and the
   first few items; detail is asked for. An agent's context is a budget.
4. **Offline unless it must not be.** Profiles are read only by commands that talk to a
   server; everything about the repository works without one.
5. **Measured, not assumed.** Platform behaviour twaco relies on was verified against a live
   server, and the knowledge topics record what was found.
6. **Failure guarantees are explicit.** Every command and MCP tool has a checked mutation class
   in [Mutation classes](MUTATION_CLASSES.md), including its safe retry rule.

## Layers

```text
main.rs  mcp.rs                 front ends: parse arguments, call core, print or return JSON
   \      /
  core::{commands, workflow}    orchestration shared by both: command policy; sync, extract, fmt, types refresh
       |
  core::{deploy, push, adopt, status, check, package, catalog, guide, ...}   features
       |
  core::{scan, splice, sidecar, entity, workspace, config, profile, server, lock}  foundations
```

### Foundations

| Module | Role |
| --- | --- |
| `scan` | Tokenises XML into spans (start, end, empty, text, CDATA, comment), validating UTF-8 and refusing markup it would have to guess at. Finds an attribute's span in a tag. |
| `splice` | Applies non-overlapping span replacements to a document. |
| `script` | Parses one service script as ECMAScript and returns facts with byte spans: identifiers and their roles, member accesses and calls, variables bound to a Thing, strings, comments. A script the parser refuses yields no facts, and the rename passes leave it for review. |
| `entity` | Reads which entity a document is: collection, name, project. |
| `entity_key` | Validates entity and service-call addresses, preserving whether a service target was a bare Thing or a qualified entity. |
| `index` | The solution as one immutable graph (petgraph) of what depends on what: entities and projects as nodes, references as edges that say how sure they are (structural, resolved, review). Answers dependents, inheritance, cycles and reachability, and lists what it could not read so a command can say what it left out. |
| `workspace` | Finds a solution's entity files, reports unreadable ones and links, and reads and writes sidecars atomically; `atomic_replace` is the one way any file that may exist is replaced. |
| `sidecar`, `datashape`, `datatable`, `mashup` | Extract and sync each sidecar kind. |
| `normalise` | The comparison form of an entity: what ThingWorx changes on its own is removed, so the repository and server versions compare by content. |
| `config`, `profile` | `twaco.toml` and server profiles. |
| `codes` | Stable error categories shared by front-end adapters. |
| `server` | The HTTP client (ureq): REST entity reads, services, Importer, Exporter, file repositories, extension uploads. Credentials are redacted from every `Debug` and error. |
| `lock` | One writer per workspace, with stale-lock detection; sweeps every hidden temporary a crashed write left. |
| `parallel` | Bounded parallel map, for server calls. |

### Features

| Module | Command |
| --- | --- |
| `check`, `lint`, `fmt`, `order`, `validate` | `check`, `fmt`: the gates, script traps, formatting (dprint's TypeScript formatter), code order, project validation |
| `types` (+ `types_base.d.ts`) | `types`: declarations from the entities and the server's metadata; `--check` runs TypeScript once over every service |
| `baseline`, `status`, `push` | `entity status`, `entity push`: the three-way comparison and conflict refusal |
| `bundle`, `deploy` | `bundle`, `deploy`: ordered bundles, live parse, import, read-back, deploy services, baseline |
| `adopt` | `adopt`: a designer's export against the repository |
| `config_table` | `config-table` |
| `refs`, `rename/` (`identity`, `field`, `table`, `member`, `run`, `apply`), `rename_scan`, `rename_property`, `rename_sql`, `dbinfo` | `rename entity`, `prefix`, `field`, `service`, `param`, `table`, `property`: one matching rule (`refs`), one planner per rename family, a transaction with rollback (`apply`), a scratch-copy check before the first write (`run`), and the database half from parsed `GetDBInfo` literals (`dbinfo`, `rename_sql`) |
| `relocate` | `move`, `copy`: a member's definition and implementation spliced from one entity's XML into another's, re-indented, with the sidecars and the callers it breaks |
| `retemplate` | `retemplate`: the entity's inheritance changed in place, with the effective members compared before and after |
| `backup` | the backup sets taken before a delete or a forced overwrite, and `entity restore` |
| `ledger`, `entity_delete`, `entity_carry`, `datatable_copy` | `entity delete`, `entity carry`, `datatable copy`: what happens on the server after a rename, driven by the typed rename ledger |
| `db` | `db run`, `db query`, `db clean`: SQL through a throwaway Database Thing |
| `logs`, `settings` | `logs`, `logs level`, `settings` |
| `repo`, `extensions`, `export`, `imports` | `repo`, `ext`, `export`, `import` |
| `package` | `package`: bundles, source-control zips, extension packages |
| `commands` | One module per state-changing command that holds its policy (lock, profile, plan versus apply, backup) as a request, a typed outcome and an executor; the CLI and MCP adapters only parse and project. See [COMMAND_FACADE.md](COMMAND_FACADE.md). |
| `transaction` | Local file changes that survive a crash: a journal written before the first change, stages and backups beside each file, and recovery on the next workspace lock that finishes the operation, undoes it, or refuses and names every path when a person has edited something. Used by `new building-block`, `move`, `copy` and `rename`; the other multi-file commands follow. See [TRANSACTIONS.md](TRANSACTIONS.md). |
| `docs` | `twaco docs`: the solution written down as deterministic Markdown or JSON, from the index, the catalog and the DataShape model; it says what it does not cover (permissions, run-time references). |
| `catalog` | `catalog`, on the same model `types` builds |
| `guide`, `help`, `javadoc` | `guide` (topics in `knowledge/`, compiled in), `help`, `javadoc` (fetched and cached) |
| `init`, `doctor` | `init`, `doctor` |

## Testing

- **Unit tests** sit beside the code they test. A server is replaced by a fake behind a small
  trait (`Remote`) or a local TCP listener, so no test needs ThingWorx.
- **`tests/offline.rs` and `tests/call.rs`** run the built binary end to end.
- **`tests/corpus.rs`** asserts the round-trip properties over real repositories: tokens tile
  every document; a no-op edit is the identity; extraction then sync changes nothing; the
  committed sidecars are reproduced byte for byte. Point `TWACO_CORPUS` at one or more real
  ThingWorx repositories (separated as `PATH` is) to run them; without it they skip. These
  are the tests to run after touching `scan`, `splice` or any sidecar module.

See [Testing](TESTING.md) for commands.

## Adding a command

1. Put the behaviour in a `core` module, with its server access behind a trait so tests can
   fake it.
2. Add the CLI route: the command table and dispatch in `main.rs`, and its lines in the usage
   text.
3. Add the MCP tool in `mcp.rs`: its schema in `tool_definitions`, its route, and its name in
   the `tools/list` test. Writes take `dry_run` defaulting to `true`; results lead with a
   summary.
4. If it writes the workspace, take the lock (`writes_workspace` in `main.rs`; the MCP route
   does it per tool).
5. Regenerate [Commands](COMMANDS.md) with `python scripts/commands_doc.py`, and document
   the command wherever it belongs in the user guide.
6. Classify the command in [Mutation classes](MUTATION_CLASSES.md); its coverage test fails
   until the CLI command and MCP tool each have a row.
