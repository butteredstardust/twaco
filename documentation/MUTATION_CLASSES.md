# Mutation classes

This document states what may have changed when a command or tool reports a failure.  Its
classes are ordered from weakest to strongest: `server-partial`, `best-effort batch`,
`multi-file atomic`, `single-file atomic`, and `read-only`.

- **read-only** writes nothing locally and sends nothing that changes the server. After a
  failure, retry freely.
- **single-file atomic** writes at most one local file, replacing it atomically. After a
  failure, that file is entirely its old content or entirely its new content, so retry freely.
- **multi-file atomic** writes several local files and undoes files already written if a later
  write fails. That rollback is in-process only: a killed process or power loss between writes
  cannot run it. A crash-recoverable journal now exists (see [TRANSACTIONS.md](TRANSACTIONS.md)) and
  `new building-block`, `move`, `copy` and every `rename` use it: the next command to take the
  workspace lock finishes or undoes an interrupted run. The other multi-file commands still have
  the in-process rollback only, so inspect the workspace after such an interruption before
  retrying.
- **best-effort batch** handles items one after another with no rollback, so a failure can leave
  earlier items changed. Some commands continue past a failing item and some stop at the first
  one; either way the report names what was done and what failed. Retry only the failed or
  remaining items after inspecting the completed ones.
- **server-partial** sends one or more server-changing requests, sometimes followed by local
  bookkeeping such as a baseline or rename ledger. A failure can leave the server changed while
  the local record is not, or can leave only part of the server work complete; use the command's
  plan, read-back checks, and backups before retrying.

`Default` is the class with no apply or write flag. `Applied` is the weakest class available
when the command is asked to write. A local output named by an option is included in `Applied`.

## CLI commands

| Command | Default | Applied | Writes | After a failure | Evidence |
| --- | --- | --- | --- | --- | --- |
| `projects` | read-only | read-only | none | Retry freely. | `main.rs: projects` |
| `types` | best-effort batch | best-effort batch | generated declarations | Inspect files already regenerated; retry failed generation. | `core/commands/types.rs: execute` |
| `types --check` | best-effort batch | best-effort batch | generated declarations, check project | Inspect generated files and retry the compiler after fixing its failure. | `core/commands/types.rs: execute` |
| `types --platform` | best-effort batch | best-effort batch | platform cache, declarations | Inspect generated files and retry after the reported failure. | `core/commands/types.rs: execute` |
| `extract` | best-effort batch | best-effort batch | sidecars, declarations | Earlier sidecars can exist; retry the reported entity. | `core/commands/extract.rs: execute` |
| `extract --all` | best-effort batch | best-effort batch | sidecars, declarations | Earlier entities remain extracted; retry failed entities. | `core/commands/extract.rs: execute` |
| `sync` | best-effort batch | best-effort batch | entity XML, declarations | Inspect the entity and generated declarations, then retry the reported item. | `core/commands/sync.rs: execute` |
| `sync --all` | best-effort batch | best-effort batch | entity XML, declarations | Earlier entities remain synced; retry failed entities. | `core/commands/sync.rs: execute` |
| `fmt` | best-effort batch | best-effort batch | script sidecars | Earlier scripts remain formatted; retry reported scripts. | `core/commands/fmt.rs: execute` |
| `check` | read-only | read-only | none | Retry freely. | `core/check.rs: run` |
| `bundle` | single-file atomic | single-file atomic | generated bundle | The generated bundle is old or new; retry freely. | `core/commands/bundle.rs: execute` |
| `deploy` | read-only | server-partial | server, baseline, backups | Read back reported imports and baseline; use the plan and backups before retrying. | `core/commands/deploy.rs: execute` |
| `call` | server-partial | server-partial | service effects | Inspect the service's effects and logs before retrying. | `core/commands/call.rs: execute` |
| `logs` | read-only | read-only | none | Retry freely. | `main.rs: logs_cmd` |
| `logs level` | read-only | server-partial | server log level | Read the current level and use the printed undo before retrying. | `core/commands/logs.rs: execute` |
| `config-table` | read-only | server-partial | server table, optional backup | Read the table back and use the backup before retrying a restore. | `core/commands/config_table.rs: execute` |
| `entity get` | read-only | single-file atomic | named local output | The output is old or new; retry freely. | `main.rs: entity_get` |
| `db run` | read-only | server-partial | SQL, temporary server Thing | Inspect SQL effects and run `db clean` if needed before retrying. | `core/commands/db.rs: execute` |
| `db query` | server-partial | server-partial | temporary server Thing | A cleanup failure can leave the temporary Thing; run `db clean` before retrying. | `core/commands/db.rs: execute` |
| `datatable copy` | read-only | server-partial | target DataTable | Read the target back; retry only after reconciling copied rows. | `core/commands/datatable_copy.rs: execute` |
| `db clean` | read-only | server-partial | temporary server Things | List remaining temporary Things before retrying. | `core/commands/db.rs: execute` |
| `entity status` | read-only | single-file atomic | baseline | The baseline is old or new; retry freely after fixing a read failure. | `core/commands/status.rs: execute` |
| `rename entity` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename prefix` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename field` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename service` | read-only | multi-file atomic | XML, sidecars, ledger | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename param` | read-only | multi-file atomic | XML, sidecars, ledger | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename table` | read-only | multi-file atomic | XML, sidecars, ledger | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename property` | read-only | multi-file atomic | XML, sidecars, ledger | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `mcp` | read-only | read-only | none | Retry freely. | `main.rs: main` |
| `doctor` | read-only | read-only | none | Retry freely. | `main.rs: doctor` |
| `export entity` | single-file atomic | single-file atomic | named export file | The output is old or new; retry freely. | `main.rs: export_cmd` |
| `export collection` | single-file atomic | single-file atomic | named export file | The output is old or new; retry freely. | `main.rs: export_cmd` |
| `export project` | single-file atomic | single-file atomic | named export file | The output is old or new; retry freely. | `main.rs: export_cmd` |
| `export source-control` | read-only | server-partial | server repository | Re-plan and inspect the repository before retrying. | `core/export.rs: source_control` |
| `package bundle` | single-file atomic | single-file atomic | named package file | The output is old or new; retry freely. | `main.rs: package_cmd` |
| `package source-control` | single-file atomic | single-file atomic | named package file | The output is old or new; retry freely. | `main.rs: package_cmd` |
| `package extension` | single-file atomic | single-file atomic | named package file | The output is old or new; retry freely. | `main.rs: package_cmd` |
| `import` | read-only | server-partial | server entities | Compare again with the server before retrying. | `main.rs: import_cmd` |
| `import source-control` | read-only | server-partial | server entities | Read the post-import diff before retrying. | `core/imports.rs: import_source_control` |
| `settings` | read-only | read-only | none | Retry freely. | `main.rs: settings_cmd` |
| `catalog` | read-only | read-only | none | Retry freely. | `main.rs: catalog_cmd` |
| `impact` | read-only | read-only | none | Retry freely. | `core/impact.rs: run` |
| `unused` | read-only | read-only | none | Retry freely. | `core/unused.rs: run` |
| `docs` | read-only | single-file atomic | named output file | The output is old or new; retry freely. | `main.rs: docs_cmd` |
| `ext list` | read-only | read-only | none | Retry freely. | `main.rs: ext_cmd` |
| `ext show` | read-only | read-only | none | Retry freely. | `main.rs: ext_cmd` |
| `ext import` | read-only | server-partial | server extension | Check installed extensions before retrying. | `main.rs: ext_cmd` |
| `ext remove` | read-only | server-partial | server extension | Check installed extensions before retrying. | `main.rs: ext_cmd` |
| `repo list` | read-only | read-only | none | Retry freely. | `main.rs: repo_cmd` |
| `repo ls` | read-only | read-only | none | Retry freely. | `main.rs: repo_cmd` |
| `repo get` | read-only | single-file atomic | optional named local output | The output is old or new; retry freely. | `main.rs: repo_cmd` |
| `repo status` | read-only | read-only | none | Retry freely. | `main.rs: repo_cmd` |
| `repo put` | read-only | server-partial | server repository | Read the path back before retrying. | `main.rs: repo_change` |
| `repo mkdir` | read-only | server-partial | server repository | List the path before retrying. | `main.rs: repo_change` |
| `repo rm` | read-only | server-partial | server repository | List the path before retrying. | `main.rs: repo_change` |
| `repo mv` | read-only | server-partial | server repository | List source and destination before retrying. | `main.rs: repo_change` |
| `repo push` | read-only | server-partial | server repository | Inspect copied paths before retrying. | `core/repo.rs: sync` |
| `repo pull` | read-only | best-effort batch | local repository files | Stops at the first failed file; the files already copied stay and are listed in the error. Retry after fixing the cause; copied files are then unchanged. | `core/repo.rs: sync` |
| `guide` | read-only | read-only | none | Retry freely. | `main.rs: guide_cmd` |
| `help search` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `core/help.rs: cached_file` |
| `help page` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `core/help.rs: cached_file` |
| `javadoc search` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `core/javadoc.rs: cached` |
| `javadoc class` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `core/javadoc.rs: cached` |
| `init` | read-only | best-effort batch | config and optional guide files | Inspect files already created; retry only missing work. | `main.rs: main; core/init.rs: write_agent_files` |
| `adopt` | read-only | best-effort batch | entities, sidecars, declarations | Earlier writes remain; retry the reported export after inspection. | `core/commands/adopt.rs: execute` |
| `entity push` | read-only | server-partial | server entity, baseline, backup | Read the entity back and check the baseline before retrying. | `core/commands/push.rs: execute` |
| `entity delete` | read-only | server-partial | server entities, ledger, backups | Inspect confirmed deletions and ledger entries before retrying. | `core/commands/delete.rs: execute` |
| `move service` | read-only | multi-file atomic | XML, sidecars | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/relocate.rs: execute` |
| `move property` | read-only | multi-file atomic | XML, sidecars | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/relocate.rs: execute` |
| `copy service` | read-only | multi-file atomic | XML, sidecars | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/relocate.rs: execute` |
| `copy property` | read-only | multi-file atomic | XML, sidecars | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/relocate.rs: execute` |
| `new building-block` | read-only | multi-file atomic | project files, config | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/newblock.rs: execute` |
| `retemplate` | read-only | single-file atomic | one entity XML | The entity is old or new; retry freely. | `core/commands/retemplate.rs: execute` |
| `entity restore` | read-only | server-partial | server entities | Read restored entities back before retrying. | `core/commands/restore.rs: execute` |
| `entity carry` | read-only | server-partial | server permissions, ledger | Read permissions and ledger before retrying. | `core/commands/carry.rs: execute` |

## MCP tools

| Tool | Default | Applied | Writes | After a failure | Evidence |
| --- | --- | --- | --- | --- | --- |
| `projects` | read-only | read-only | none | Retry freely. | `mcp.rs: projects` |
| `types` | best-effort batch | best-effort batch | declarations or platform cache | Every action can regenerate declarations; inspect completed files and retry. | `core/commands/types.rs: execute` |
| `check` | read-only | read-only | none | Retry freely. | `mcp.rs: check_tool` |
| `status` | read-only | single-file atomic | baseline | `record: true` replaces one baseline atomically; retry freely. | `core/commands/status.rs: execute` |
| `sync` | best-effort batch | best-effort batch | entity XML, declarations | Earlier entities remain synced; retry failed entities. | `core/commands/sync.rs: execute` |
| `extract` | best-effort batch | best-effort batch | sidecars, declarations | Earlier entities remain extracted; retry failed entities. | `core/commands/extract.rs: execute` |
| `fmt` | best-effort batch | best-effort batch | script sidecars | Earlier scripts remain formatted; retry reported scripts. | `core/commands/fmt.rs: execute` |
| `push` | read-only | server-partial | server entity, baseline, backup | Read the entity and baseline before retrying. | `core/commands/push.rs: execute` |
| `entity_delete` | read-only | server-partial | server entities, ledger, backups | Inspect confirmed deletions and ledger entries before retrying. | `core/commands/delete.rs: execute` |
| `entity_restore` | read-only | server-partial | server entities | Read restored entities back before retrying. | `core/commands/restore.rs: execute` |
| `entity_carry` | read-only | server-partial | server permissions, ledger | Read permissions and ledger before retrying. | `core/commands/carry.rs: execute` |
| `db_run` | read-only | server-partial | SQL, temporary server Thing | Inspect SQL effects and clean temporary Things before retrying. | `core/commands/db.rs: execute` |
| `db_query` | server-partial | server-partial | temporary server Thing | Run `db_clean` if temporary cleanup failed. | `core/commands/db.rs: execute` |
| `datatable_copy` | read-only | server-partial | target DataTable | Read the target back before retrying. | `core/commands/datatable_copy.rs: execute` |
| `db_clean` | read-only | server-partial | temporary server Things | List remaining temporary Things before retrying. | `core/commands/db.rs: execute` |
| `deploy` | read-only | server-partial | server, baseline, backups | Read back imports and baseline; use plan and backups before retrying. | `core/commands/deploy.rs: execute` |
| `adopt_report` | read-only | read-only | none | Retry freely. | `mcp.rs: adopt_tool` |
| `adopt_apply` | best-effort batch | best-effort batch | entities, sidecars, declarations | Unlike the CLI form, this tool applies directly; inspect completed writes and retry failures. | `core/commands/adopt.rs: execute` |
| `rename` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry the reviewed plan. | `core/commands/rename.rs: execute` |
| `move_member` | read-only | multi-file atomic | XML, sidecars | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/relocate.rs: execute` |
| `new_building_block` | read-only | multi-file atomic | project files, config | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/newblock.rs: execute` |
| `retemplate` | read-only | single-file atomic | one entity XML | The entity is old or new; retry freely. | `core/commands/retemplate.rs: execute` |
| `config_table` | read-only | server-partial | server table | Read the table back before retrying restore. | `core/commands/config_table.rs: execute` |
| `logs` | read-only | read-only | none | Retry freely. | `mcp.rs: logs_tool` |
| `log_level` | read-only | server-partial | server log level | Read the current level and use the undo before retrying. | `core/commands/logs.rs: execute` |
| `repo` | read-only | read-only | none | Retry freely. | `mcp.rs: repo_tool` |
| `repo_write` | read-only | server-partial | server repository or local pull | Inspect copied paths before retrying. | `mcp.rs: repo_write_tool` |
| `extensions` | read-only | read-only | none | Retry freely. | `mcp.rs: extensions_tool` |
| `extension_write` | read-only | server-partial | server extension | Check installed extensions before retrying. | `mcp.rs: extension_write_tool` |
| `export` | read-only | server-partial | local export or server repository | Local export is atomic; source-control apply needs a new plan and inspection. | `mcp.rs: export_tool` |
| `package` | single-file atomic | single-file atomic | named package file | The output is old or new; retry freely. | `mcp.rs: package_tool` |
| `import` | read-only | server-partial | server entities | Compare again with the server before retrying. | `mcp.rs: import_tool` |
| `settings` | read-only | read-only | none | Retry freely. | `mcp.rs: settings_tool` |
| `catalog` | read-only | read-only | none | Retry freely. | `mcp.rs: catalog_tool` |
| `impact` | read-only | read-only | none | Retry freely. | `mcp.rs: impact_tool` |
| `unused` | read-only | read-only | none | Retry freely. | `mcp.rs: unused_tool` |
| `docs` | read-only | read-only | none | Retry freely. | `mcp.rs: docs_tool` |
| `guide` | read-only | read-only | none | Retry freely. | `mcp.rs: guide_tool` |
| `help_search` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `mcp.rs: help_search_tool` |
| `help_page` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `mcp.rs: help_page_tool` |
| `javadoc` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `mcp.rs: javadoc_tool` |
| `call` | read-only | server-partial | service effects | `dry_run` differs from CLI `call`; inspect service effects before retrying. | `core/commands/call.rs: execute` |
