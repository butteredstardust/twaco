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
  cannot run it; a future crash-recoverable journal would close this known gap. Inspect the
  workspace after such an interruption before retrying.
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
| `types` | best-effort batch | best-effort batch | generated declarations | Inspect files already regenerated; retry failed generation. | `core/types.rs: write_model` |
| `types --check` | read-only | read-only | none | Retry freely. | `core/types.rs: check` |
| `types --platform` | best-effort batch | best-effort batch | platform cache, declarations | Inspect generated files and retry after the reported failure. | `core/types.rs: fetch_platform` |
| `extract` | best-effort batch | best-effort batch | sidecars, declarations | Earlier sidecars can exist; retry the reported entity. | `core/workflow.rs: extract` |
| `extract --all` | best-effort batch | best-effort batch | sidecars, declarations | Earlier entities remain extracted; retry failed entities. | `core/workflow.rs: extract` |
| `sync` | best-effort batch | best-effort batch | entity XML, declarations | Inspect the entity and generated declarations, then retry the reported item. | `core/workflow.rs: sync` |
| `sync --all` | best-effort batch | best-effort batch | entity XML, declarations | Earlier entities remain synced; retry failed entities. | `core/workflow.rs: sync` |
| `fmt` | best-effort batch | best-effort batch | script sidecars | Earlier scripts remain formatted; retry reported scripts. | `core/workflow.rs: fmt` |
| `check` | read-only | read-only | none | Retry freely. | `core/check.rs: run` |
| `bundle` | single-file atomic | single-file atomic | generated bundle | The generated bundle is old or new; retry freely. | `main.rs: bundle` |
| `deploy` | read-only | server-partial | server, baseline, backups | Read back reported imports and baseline; use the plan and backups before retrying. | `core/deploy.rs: run` |
| `call` | server-partial | server-partial | service effects | Inspect the service's effects and logs before retrying. | `main.rs: call` |
| `logs` | read-only | read-only | none | Retry freely. | `main.rs: logs_cmd` |
| `logs level` | read-only | server-partial | server log level | Read the current level and use the printed undo before retrying. | `main.rs: log_level_cmd` |
| `config-table` | read-only | server-partial | server table, optional backup | Read the table back and use the backup before retrying a restore. | `main.rs: config_table` |
| `entity get` | read-only | single-file atomic | named local output | The output is old or new; retry freely. | `main.rs: entity_get` |
| `db run` | read-only | server-partial | SQL, temporary server Thing | Inspect SQL effects and run `db clean` if needed before retrying. | `core/db.rs: run` |
| `db query` | server-partial | server-partial | temporary server Thing | A cleanup failure can leave the temporary Thing; run `db clean` before retrying. | `core/db.rs: run` |
| `datatable copy` | read-only | server-partial | target DataTable | Read the target back; retry only after reconciling copied rows. | `core/datatable_copy.rs: run` |
| `db clean` | read-only | server-partial | temporary server Things | List remaining temporary Things before retrying. | `core/db.rs: clean` |
| `entity status` | read-only | single-file atomic | baseline | The baseline is old or new; retry freely after fixing a read failure. | `main.rs: entity_status` |
| `rename entity` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | Inspect after interruption; otherwise retry after rollback. | `core/rename/apply.rs: apply` |
| `rename prefix` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | Inspect after interruption; otherwise retry after rollback. | `core/rename/apply.rs: apply` |
| `rename field` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | Inspect after interruption; otherwise retry after rollback. | `core/rename/apply.rs: apply` |
| `rename service` | read-only | multi-file atomic | XML, sidecars, ledger | Inspect after interruption; otherwise retry after rollback. | `core/rename/apply.rs: apply` |
| `rename param` | read-only | multi-file atomic | XML, sidecars, ledger | Inspect after interruption; otherwise retry after rollback. | `core/rename/apply.rs: apply` |
| `rename table` | read-only | multi-file atomic | XML, sidecars, ledger | Inspect after interruption; otherwise retry after rollback. | `core/rename/apply.rs: apply` |
| `rename property` | read-only | multi-file atomic | XML, sidecars, ledger | Inspect after interruption; otherwise retry after rollback. | `core/rename/apply.rs: apply` |
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
| `adopt` | read-only | best-effort batch | entities, sidecars, declarations | Earlier writes remain; retry the reported export after inspection. | `core/adopt.rs: apply` |
| `entity push` | read-only | server-partial | server entity, baseline, backup | Read the entity back and check the baseline before retrying. | `core/push.rs: run` |
| `entity delete` | read-only | server-partial | server entities, ledger, backups | Inspect confirmed deletions and ledger entries before retrying. | `core/entity_delete.rs: run` |
| `move service` | read-only | multi-file atomic | XML, sidecars | Inspect after interruption; otherwise retry after rollback. | `core/relocate.rs: apply` |
| `move property` | read-only | multi-file atomic | XML, sidecars | Inspect after interruption; otherwise retry after rollback. | `core/relocate.rs: apply` |
| `copy service` | read-only | multi-file atomic | XML, sidecars | Inspect after interruption; otherwise retry after rollback. | `core/relocate.rs: apply` |
| `copy property` | read-only | multi-file atomic | XML, sidecars | Inspect after interruption; otherwise retry after rollback. | `core/relocate.rs: apply` |
| `new building-block` | read-only | multi-file atomic | project files, config | Inspect after interruption; otherwise retry after rollback. | `core/newblock.rs: apply` |
| `retemplate` | read-only | single-file atomic | one entity XML | The entity is old or new; retry freely. | `core/retemplate.rs: apply` |
| `entity restore` | read-only | server-partial | server entities | Read restored entities back before retrying. | `core/backup.rs: restore` |
| `entity carry` | read-only | server-partial | server permissions, ledger | Read permissions and ledger before retrying. | `core/entity_carry.rs: run` |

## MCP tools

| Tool | Default | Applied | Writes | After a failure | Evidence |
| --- | --- | --- | --- | --- | --- |
| `projects` | read-only | read-only | none | Retry freely. | `mcp.rs: projects` |
| `types` | best-effort batch | best-effort batch | declarations or platform cache | `action: check` is read-only; otherwise inspect completed files and retry. | `mcp.rs: types_tool_with_compiler` |
| `check` | read-only | read-only | none | Retry freely. | `mcp.rs: check_tool` |
| `status` | read-only | single-file atomic | baseline | `record: true` replaces one baseline atomically; retry freely. | `mcp.rs: status_tool` |
| `sync` | best-effort batch | best-effort batch | entity XML, declarations | Earlier entities remain synced; retry failed entities. | `mcp.rs: sync_tool` |
| `extract` | best-effort batch | best-effort batch | sidecars, declarations | Earlier entities remain extracted; retry failed entities. | `mcp.rs: extract_tool` |
| `fmt` | best-effort batch | best-effort batch | script sidecars | Earlier scripts remain formatted; retry reported scripts. | `mcp.rs: fmt_tool` |
| `push` | read-only | server-partial | server entity, baseline, backup | Read the entity and baseline before retrying. | `mcp.rs: push_tool` |
| `entity_delete` | read-only | server-partial | server entities, ledger, backups | Inspect confirmed deletions and ledger entries before retrying. | `mcp.rs: entity_delete_tool` |
| `entity_restore` | read-only | server-partial | server entities | Read restored entities back before retrying. | `mcp.rs: entity_restore_tool` |
| `entity_carry` | read-only | server-partial | server permissions, ledger | Read permissions and ledger before retrying. | `mcp.rs: entity_carry_tool` |
| `db_run` | read-only | server-partial | SQL, temporary server Thing | Inspect SQL effects and clean temporary Things before retrying. | `mcp.rs: db_tool` |
| `db_query` | server-partial | server-partial | temporary server Thing | Run `db_clean` if temporary cleanup failed. | `mcp.rs: db_tool` |
| `datatable_copy` | read-only | server-partial | target DataTable | Read the target back before retrying. | `mcp.rs: datatable_copy_tool` |
| `db_clean` | read-only | server-partial | temporary server Things | List remaining temporary Things before retrying. | `mcp.rs: db_clean_tool` |
| `deploy` | read-only | server-partial | server, baseline, backups | Read back imports and baseline; use plan and backups before retrying. | `mcp.rs: deploy_tool` |
| `adopt_report` | read-only | read-only | none | Retry freely. | `mcp.rs: adopt_tool` |
| `adopt_apply` | best-effort batch | best-effort batch | entities, sidecars, declarations | Unlike the CLI form, this tool applies directly; inspect completed writes and retry failures. | `mcp.rs: adopt_apply_tool` |
| `rename` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | Inspect after interruption; otherwise retry the reviewed plan. | `mcp.rs: rename_tool` |
| `move_member` | read-only | multi-file atomic | XML, sidecars | Inspect after interruption; otherwise retry after rollback. | `mcp.rs: move_member_tool` |
| `new_building_block` | read-only | multi-file atomic | project files, config | Inspect after interruption; otherwise retry after rollback. | `mcp.rs: new_building_block_tool` |
| `retemplate` | read-only | single-file atomic | one entity XML | The entity is old or new; retry freely. | `mcp.rs: retemplate_tool` |
| `config_table` | read-only | server-partial | server table | Read the table back before retrying restore. | `mcp.rs: config_table_tool` |
| `logs` | read-only | read-only | none | Retry freely. | `mcp.rs: logs_tool` |
| `log_level` | read-only | server-partial | server log level | Read the current level and use the undo before retrying. | `mcp.rs: log_level_tool` |
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
| `guide` | read-only | read-only | none | Retry freely. | `mcp.rs: guide_tool` |
| `help_search` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `mcp.rs: help_search_tool` |
| `help_page` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `mcp.rs: help_page_tool` |
| `javadoc` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `mcp.rs: javadoc_tool` |
| `call` | read-only | server-partial | service effects | `dry_run` differs from CLI `call`; inspect service effects before retrying. | `mcp.rs: call_service_tool` |
