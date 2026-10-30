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
  `new building-block`, `move`, `copy`, `adopt`, every `rename`, `repo pull` and the
  `localization` file edits use it (and `extract`, for each entity's sidecars): the next command to take the
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
| `extract` | best-effort batch | best-effort batch | sidecars, declarations | Each entity's sidecars are written together or not at all, and the next command that takes the workspace lock finishes or undoes one a crash interrupted; earlier entities remain extracted. Retry failed entities. | `core/commands/extract.rs: execute` |
| `extract --all` | best-effort batch | best-effort batch | sidecars, declarations | Each entity's sidecars are written together or not at all, and the next command that takes the workspace lock finishes or undoes one a crash interrupted; earlier entities remain extracted. Retry failed entities. | `core/commands/extract.rs: execute` |
| `sync` | best-effort batch | best-effort batch | entity XML, declarations, `datatable.json` of a DataTable whose DataShape changed | Each entity file is written once, atomically, with every kind of sidecar that synced (a kind that is refused is reported, and the others are still written); a DataTable's `datatable.json` is written after its XML, and a crash between the two is put right by the next sync. Earlier entities remain synced. Retry failed entities. | `core/commands/sync.rs: execute` |
| `sync --all` | best-effort batch | best-effort batch | entity XML, declarations, `datatable.json` of a DataTable whose DataShape changed | Each entity file is written once, atomically, with every kind of sidecar that synced (a kind that is refused is reported, and the others are still written); a DataTable's `datatable.json` is written after its XML, and a crash between the two is put right by the next sync. Earlier entities remain synced. Retry failed entities. | `core/commands/sync.rs: execute` |
| `fmt` | best-effort batch | best-effort batch | script sidecars | Earlier scripts remain formatted; retry reported scripts. | `core/commands/fmt.rs: execute` |
| `check` | read-only | read-only | none | Retry freely. | `core/check.rs: run` |
| `requirements init` | read-only | single-file atomic | new `requirements.toml` | Creation is atomic and refuses an existing file; inspect it and use requirements check. | `core/commands/requirements.rs: execute_init` |
| `requirements check` | read-only | read-only | none; optional server reads | Retry freely. Server read errors block. | `core/commands/requirements.rs: execute_check` |
| `bundle` | read-only | single-file atomic | generated bundle and optional handoff | The generated bundle is old or new; an applied handoff is available as a base. Retry freely. | `core/commands/bundle.rs: execute` |
| `deploy` | read-only | server-partial | server, baseline, backups, localization tables | Read back reported imports and baseline; use the plan and backups before retrying An agent imported first without allowances its namespaces drop is put back when its bundle fails to import. A localization failure after the imports says so; `localization push --apply` retries the tables. | `core/commands/deploy.rs: execute` |
| `call` | server-partial | server-partial | service effects | Inspect the service's effects and logs before retrying. | `core/commands/call.rs: execute` |
| `logs` | read-only | read-only | none | Retry freely. | `cli/data.rs: logs_cmd` |
| `logs level` | read-only | server-partial | server log level | Read the current level and use the printed undo before retrying. | `core/commands/logs.rs: execute` |
| `config-table` | read-only | server-partial | server table, optional backup | Read the table back and use the backup before retrying a restore. | `core/commands/config_table.rs: execute` |
| `entity get` | read-only | single-file atomic | named local output | The output is old or new; retry freely. | `cli/entity.rs: entity_get` |
| `db run` | read-only | server-partial | SQL, temporary server Thing | Inspect SQL effects and run `db clean` if needed before retrying. | `core/commands/db.rs: execute` |
| `db query` | server-partial | server-partial | temporary server Thing | A cleanup failure can leave the temporary Thing; run `db clean` before retrying. | `core/commands/db.rs: execute` |
| `datatable copy` | read-only | server-partial | target DataTable | Read the target back; retry only after reconciling copied rows. | `core/commands/datatable_copy.rs: execute` |
| `db clean` | read-only | server-partial | temporary server Things | List remaining temporary Things before retrying. | `core/commands/db.rs: execute` |
| `entity status` | read-only | single-file atomic | baseline | The baseline is old or new; retry freely after fixing a read failure. | `core/commands/status.rs: execute` |
| `entity pull` | read-only | multi-file atomic | entity XML, sidecars, baseline | A crash is recovered by the next workspace writer; rerun after resolving any reported sidecar failure. | `core/commands/pull.rs: execute` |
| `rename entity` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename prefix` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename field` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename service` | read-only | multi-file atomic | XML, sidecars, ledger | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename param` | read-only | multi-file atomic | XML, sidecars, ledger | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename table` | read-only | multi-file atomic | XML, sidecars, ledger | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `rename property` | read-only | multi-file atomic | XML, sidecars, ledger | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/rename.rs: execute` |
| `mcp` | read-only | read-only | none | Retry freely. | `main.rs: main` |
| `mcp configure` | read-only | single-file atomic | the client's MCP config (a backup beside it) | The file is old or new, and the old one is saved beside it; retry freely. | `cli/live.rs: mcp_configure_cmd` |
| `doctor` | read-only | read-only | none | Retry freely. | `main.rs: doctor` |
| `update` | read-only | single-file atomic | the twaco executable | The executable is old or new; retry freely. | `core/update.rs: install` |
| `completions` | read-only | read-only | none | Retry freely. | `main.rs: completions_cmd` |
| `search` | read-only | read-only | none | Retry freely. | `cli/content.rs: search_cmd` |
| `entity inspect` | read-only | read-only | none | Retry freely. | `cli/live.rs: entity_inspect_cmd` |
| `model resolve` | read-only | read-only | none | Retry freely. | `cli/live.rs: model_cmd` |
| `access explain` | read-only | read-only | none | Retry freely; nothing is invoked. | `cli/live.rs: access_cmd` |
| `export entity` | single-file atomic | single-file atomic | named export file | The output is old or new; retry freely. | `cli/content.rs: export_cmd` |
| `export collection` | single-file atomic | single-file atomic | named export file | The output is old or new; retry freely. | `cli/content.rs: export_cmd` |
| `export project` | single-file atomic | single-file atomic | named export file | The output is old or new; retry freely. | `cli/content.rs: export_cmd` |
| `export source-control` | read-only | server-partial | server repository | Re-plan and inspect the repository before retrying. | `core/export.rs: source_control` |
| `package bundle` | single-file atomic | single-file atomic | named package file | The output is old or new; retry freely. | `cli/content.rs: package_cmd` |
| `package source-control` | single-file atomic | single-file atomic | named package file | The output is old or new; retry freely. | `cli/content.rs: package_cmd` |
| `package extension` | single-file atomic | single-file atomic | named package file | The output is old or new; retry freely. | `cli/content.rs: package_cmd` |
| `import` | read-only | server-partial | server entities | Compare again with the server before retrying. | `cli/content.rs: import_cmd` |
| `import source-control` | read-only | server-partial | server entities | Read the post-import diff before retrying. | `core/imports.rs: import_source_control` |
| `settings` | read-only | read-only | none | Retry freely. | `cli/info.rs: settings_cmd` |
| `catalog` | read-only | read-only | none | Retry freely. | `cli/info.rs: catalog_cmd` |
| `contract diff` | read-only | read-only | none | Retry freely. | `core/contract.rs: diff` |
| `impact` | read-only | read-only | none | Retry freely. | `core/impact.rs: run` |
| `trace` | read-only | read-only | none | Retry freely. | `core/trace.rs: run` |
| `unused` | read-only | read-only | none | Retry freely. | `core/unused.rs: run` |
| `docs` | read-only | single-file atomic | named output file | The output is old or new; retry freely. | `cli/info.rs: docs_cmd` |
| `block list` | read-only | read-only | none | Reads the repository and, with --server, the server; retry freely. | `cli/block.rs: block_cmd` |
| `block show` | read-only | read-only | none | Reads the repository and, with --server, the server; retry freely. | `cli/block.rs: block_cmd` |
| `block version` | read-only | multi-file atomic | the Project and entry point files | Writes both files or neither, refused if either changed since it was read; retry. | `core/commands/block.rs: version` |
| `ext list` | read-only | read-only | none | Retry freely. | `cli/content.rs: ext_cmd` |
| `ext show` | read-only | read-only | none | Retry freely. | `cli/content.rs: ext_cmd` |
| `ext check` | read-only | read-only | none | Reads an extension project's files only; retry freely. | `cli/content.rs: ext_check_cmd` |
| `ext build` | best-effort batch | best-effort batch | the project's build output, node_modules | Runs the project's own build, which writes build/ or zip/ (and installs its dependencies the first time); retry. | `core/ext_build.rs: build` |
| `ext new` | read-only | multi-file atomic | extension project files | Writes the new project's files into a missing or empty folder and removes them again on a failure (in-process only). Retry. | `core/ext_new.rs: Plan::apply` |
| `ext add` | read-only | multi-file atomic | extension project files | Writes the new class or widget files, then replaces metadata.xml whole, refused if it changed since it was read; a failure removes the new files again (in-process only). Retry. | `core/ext_new.rs: Plan::apply` |
| `ext deploy` | read-only | server-partial | extension project files, server extension | Runs the build and, with --bump, rewrites packageVersion, put back if the build fails or the install fails and the package list lacks the new version (kept when the server may have it); the install is one upload. Check `ext list` before retrying. | `core/ext_deploy.rs: deploy` |
| `ext intellij` | read-only | best-effort batch | IntelliJ configuration files | Writes two files into the IntelliJ configuration folder, one after the other; retry. | `core/intellij.rs: Install::apply` |
| `ext import` | read-only | server-partial | server extension | Check installed extensions before retrying. | `cli/content.rs: ext_cmd` |
| `ext remove` | read-only | server-partial | server extension | Check installed extensions before retrying. | `cli/content.rs: ext_cmd` |
| `repo list` | read-only | read-only | none | Retry freely. | `cli/content.rs: repo_cmd` |
| `repo ls` | read-only | read-only | none | Retry freely. | `cli/content.rs: repo_cmd` |
| `repo get` | read-only | single-file atomic | optional named local output | The output is old or new; retry freely. | `cli/content.rs: repo_cmd` |
| `repo status` | read-only | read-only | none | Retry freely. | `cli/content.rs: repo_cmd` |
| `repo put` | read-only | server-partial | server repository | Read the path back before retrying. | `cli/content.rs: repo_change` |
| `repo mkdir` | read-only | server-partial | server repository | List the path before retrying. | `cli/content.rs: repo_change` |
| `repo rm` | read-only | server-partial | server repository | List the path before retrying. | `cli/content.rs: repo_change` |
| `repo mv` | read-only | server-partial | server repository | List source and destination before retrying. | `cli/content.rs: repo_change` |
| `repo push` | read-only | server-partial | server repository | Inspect copied paths before retrying. | `core/repo.rs: sync` |
| `repo pull` | read-only | multi-file atomic | local repository files | Every file is downloaded before any is written, then all are written in one journaled transaction: a failed download or write copies nothing (an error that says the pull was left part-written names what the journal holds). The repository folder must be inside the solution. Retry after fixing the cause. | `core/commands/repo.rs: execute` |
| `localization status` | read-only | read-only | none | Retry freely. | `core/commands/localization.rs: execute` |
| `localization pull` | read-only | multi-file atomic | localization files | Every file is written in one journaled transaction, or none is; a file edited since the plan read it is refused (an error that says files were left part-written names what the journal holds). Retry after fixing the cause. | `core/commands/localization.rs: execute` |
| `localization push` | read-only | server-partial | localization tables | Read tables back before retrying. | `core/commands/localization.rs: execute` |
| `localization new` | read-only | single-file atomic | localization file | The file is old or new; retry freely. | `core/commands/localization.rs: execute` |
| `localization set` | read-only | single-file atomic | localization file | The file is old or new; retry freely. | `core/commands/localization.rs: execute` |
| `localization remove` | read-only | multi-file atomic | localization files | Every file is written in one journaled transaction, or none is; a file edited since the plan read it is refused (an error that says files were left part-written names what the journal holds). Retry after fixing the cause. | `core/commands/localization.rs: execute` |
| `users` | read-only | server-partial | server users, group memberships, test project | `list` writes nothing. `seed` and `delete` make one server call per user, membership or project: `users list` shows what exists before retrying, and a seed never changes what exists. | `core/commands/users.rs: execute` |
| `appkey` | read-only | server-partial | server application key, a profile file | `create` makes the key, then reads it back and, with `--save`, writes it into the profile; a key made but not saved is read again with `appkey show <name> --save <profile>`, never made twice (a second create is refused). `show` writes only the profile. `delete` is `entity delete`. | `core/commands/appkey.rs: execute` |
| `test` | read-only | server-partial | whatever the cases' services change | Without `--apply` it reads and checks the case files only. Applied, each case calls its service as written, one after another; a failing case does not stop the rest. Rerunning repeats every call. | `core/commands/service_tests.rs: execute` |
| `guide` | read-only | read-only | none | Retry freely. | `cli/info.rs: guide_cmd` |
| `help search` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `core/help.rs: cached_file` |
| `help page` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `core/help.rs: cached_file` |
| `javadoc search` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `core/javadoc.rs: cached` |
| `javadoc class` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `core/javadoc.rs: cached` |
| `javadoc service` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `core/javadoc.rs: read_service` |
| `init` | read-only | best-effort batch | config and optional guide files | Inspect files already created; retry only missing work. | `main.rs: main; core/init.rs: write_agent_files` |
| `init --from-server` | read-only | best-effort batch | entity files, twaco.toml (new, or appended to when adding projects), agent files and .gitignore lines (new repository only); sidecars; baseline; localization table files | Entity files and config are one journaled transaction, or none; sidecars follow per entity, then the baseline, then the localization pull, all under one lock. After a later failure the files stay and a rerun refuses: `twaco extract --all`, then `twaco entity status --all --record`; a failed localization pull is reported and left to `twaco localization pull`. | `core/init_server.rs: execute` |
| `adopt` | read-only | multi-file atomic | entities, service/mashup sidecars, declarations | Three-way apply writes only selected UI and backend changes in one transaction; every entity and sidecar write happened or none did. Declarations are regenerated afterwards, and a failure there is a warning. Retry. | `core/commands/adopt.rs: execute` |
| `handoff` | read-only | best-effort batch | `.twaco/handoffs` copies and metadata | Listing retries freely; a record plan writes nothing, and an applied record can be retried with a new name. | `core/commands/handoff.rs: execute` |
| `entity push` | read-only | server-partial | server entity, baseline, backup | Read the entity back and check the baseline before retrying. | `core/commands/push.rs: execute` |
| `entity delete` | read-only | server-partial | server entities, ledger, backups | Inspect confirmed deletions and ledger entries before retrying. | `core/commands/delete.rs: execute` |
| `move service` | read-only | multi-file atomic | XML, sidecars | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/relocate.rs: execute` |
| `move property` | read-only | multi-file atomic | XML, sidecars | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/relocate.rs: execute` |
| `copy service` | read-only | multi-file atomic | XML, sidecars | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/relocate.rs: execute` |
| `copy property` | read-only | multi-file atomic | XML, sidecars | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/relocate.rs: execute` |
| `new building-block` | read-only | multi-file atomic | project files, config | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/newblock.rs: execute` |
| `new project` | read-only | multi-file atomic | entity XML, config | The Project entity and a new `[[project]]` entry (a new `twaco.toml` where there is none) are created together; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new group` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new thing` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new thing-template` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new thing-shape` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new data-shape` | read-only | multi-file atomic | entity XML, fields.json | The entity XML and sidecar are created in one journaled transaction, or none is; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new timer` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new scheduler` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new data-table` | read-only | multi-file atomic | entity XML, datatable.json | The entity XML and sidecar are created in one journaled transaction, or none is. Retry. | `core/commands/entity_new.rs: execute` |
| `new stream` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new value-stream` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new media` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new style` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new state` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new menu` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new theme` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new network` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new mcp-namespace` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `new ai-agent` | read-only | multi-file atomic | entity XML | The entity XML is created in a journaled transaction; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `network list` | read-only | read-only | none | Retry freely. | `core/network.rs: parse` |
| `network show` | read-only | read-only | none | Retry freely. | `core/network.rs: show` |
| `network diff` | read-only | read-only | server | Retry freely. | `core/commands/network.rs: server_diff` |
| `network pull` | read-only | single-file atomic | Network XML | Applying reads the server again while holding the workspace lock, then replaces only the Network XML through a transaction. Retry after a stale plan. | `core/commands/network.rs: pull_execute` |
| `network add` | read-only | multi-file atomic | Network XML | The Network XML is changed in a journaled transaction; a file edited since the plan is refused. Retry. | `core/commands/network.rs: execute` |
| `network move` | read-only | multi-file atomic | Network XML | The Network XML is changed in a journaled transaction; a file edited since the plan is refused. Retry. | `core/commands/network.rs: execute` |
| `network remove` | read-only | multi-file atomic | Network XML | The Network XML is changed in a journaled transaction; a file edited since the plan is refused. Retry. | `core/commands/network.rs: execute` |
| `network import` | read-only | multi-file atomic | Network XML, new Thing XML | Every new Thing and the Network XML are written in one journaled transaction, or none is; a changed file is refused. Retry. | `core/commands/network.rs: import_execute` |
| `thing property` | read-only | multi-file atomic | entity XML, Things inheriting it | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `thing service` | read-only | multi-file atomic | entity XML, service sidecars | The entity and service sidecars are created or deleted in one journaled transaction; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `thing subscription` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `thing shape` | read-only | multi-file atomic | entity XML, Things inheriting it | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `timer set` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `scheduler set` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `style set` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `media set` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `menu item` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `mcp-namespace add` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `mcp-namespace remove` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `agent allow` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `agent disallow` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `agent taxonomy` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `agent skill` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `agent set` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `state set` | read-only | multi-file atomic | entity XML | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `retemplate` | read-only | single-file atomic | one entity XML | The entity is old or new; retry freely. | `core/commands/retemplate.rs: execute` |
| `mashup describe` | read-only | read-only | none | Retry freely. | `core/mashup_describe.rs: describe` |
| `mashup validate` | read-only | read-only | none | Retry freely. | `core/mashup_validate.rs: validate` |
| `widgets` | read-only | read-only | none | Retry freely. | `core/widgets_view.rs: list` |
| `mashup templates` | read-only | read-only | none | Retry freely. | `core/mashup_templates_view.rs: list` |
| `mashup new` | read-only | multi-file atomic | new entity file, mashup sidecars | The entity file and its content.json and custom.css are created in one journaled transaction, or none is; a file that appeared since the plan is refused. Retry. | `core/commands/mashup.rs: new` |
| `mashup add` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: add` |
| `mashup add-contained` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: add_contained` |
| `mashup bind` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: bind` |
| `mashup recipe` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: recipe` |
| `mashup on` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: on` |
| `mashup remove` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; cascade only removes proven bindings and events. Retry. | `core/commands/mashup.rs: remove` |
| `mashup unbind` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: unbind` |
| `mashup off` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: off` |
| `mashup param` | read-only | multi-file atomic | affected mashup entity files, content sidecars | Every child and parent container is replaced through one journaled transaction, or none is; a file edited since planning is refused. Retry. | `core/commands/mashup.rs: parameter` |
| `mashup event` | read-only | multi-file atomic | affected mashup entity files, content sidecars | Every child and parent container is replaced through one journaled transaction, or none is; a file edited since planning is refused. Retry. | `core/commands/mashup.rs: event` |
| `reproject` | read-only | multi-file atomic | entity files | Every moved file is rewritten and moved in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/reproject.rs: execute` |
| `entity restore` | read-only | server-partial | server entities | Read restored entities back before retrying. | `core/commands/restore.rs: execute` |
| `entity carry` | read-only | server-partial | server permissions, ledger | Read permissions and ledger before retrying. | `core/commands/carry.rs: execute` |
| `permissions init` | read-only | multi-file atomic | new permissions.toml files | One transaction of new files; an existing policy is never touched. | `core/commands/permissions.rs: execute_init` |
| `permissions audit` | read-only | read-only | none | Retry freely. | `core/permissions/audit.rs: audit` |
| `permissions apply` | read-only | multi-file atomic | entity XML | One transaction; run `permissions audit` before retrying. | `core/commands/permissions.rs: execute_apply` |
| `permissions diff` | read-only | read-only | none | Retry freely. | `core/commands/permissions.rs: execute` |
| `permissions push` | read-only | server-partial | server permissions, baseline | Each set is written whole and read back; run `permissions diff` before retrying. | `core/commands/permissions.rs: execute` |
| `permissions push --platform` | read-only | server-partial | grants and memberships on shared server entities | Adds one item at a time and reads it back, never removes; run `permissions audit --server` before retrying. | `core/commands/permissions.rs: execute_platform` |

## MCP tools

| Tool | Default | Applied | Writes | After a failure | Evidence |
| --- | --- | --- | --- | --- | --- |
| `projects` | read-only | read-only | none | Retry freely. | `mcp/mod.rs: projects` |
| `types` | best-effort batch | best-effort batch | declarations or platform cache | Every action can regenerate declarations; inspect completed files and retry. | `core/commands/types.rs: execute` |
| `check` | read-only | read-only | none | Retry freely. | `mcp/source.rs: check_tool` |
| `status` | read-only | single-file atomic | baseline | `record: true` replaces one baseline atomically; retry freely. | `core/commands/status.rs: execute` |
| `entity_pull` | read-only | multi-file atomic | entity XML, sidecars, baseline | An applied pull takes one workspace lock; rerun after resolving a reported sidecar failure. | `core/commands/pull.rs: execute` |
| `sync` | best-effort batch | best-effort batch | entity XML, declarations | Each entity file is written once, atomically, with every kind of sidecar that synced (a kind that is refused is reported, and the others are still written); earlier entities remain synced. Retry failed entities. | `core/commands/sync.rs: execute` |
| `extract` | best-effort batch | best-effort batch | sidecars, declarations | Each entity's sidecars are written together or not at all, and the next command that takes the workspace lock finishes or undoes one a crash interrupted; earlier entities remain extracted. Retry failed entities. | `core/commands/extract.rs: execute` |
| `fmt` | best-effort batch | best-effort batch | script sidecars | Earlier scripts remain formatted; retry reported scripts. | `core/commands/fmt.rs: execute` |
| `push` | read-only | server-partial | server entity, baseline, backup | Read the entity and baseline before retrying. | `core/commands/push.rs: execute` |
| `entity_delete` | read-only | server-partial | server entities, ledger, backups | Inspect confirmed deletions and ledger entries before retrying. | `core/commands/delete.rs: execute` |
| `entity_restore` | read-only | server-partial | server entities | Read restored entities back before retrying. | `core/commands/restore.rs: execute` |
| `entity_carry` | read-only | server-partial | server permissions, ledger | Read permissions and ledger before retrying. | `core/commands/carry.rs: execute` |
| `permissions_init` | read-only | multi-file atomic | new permissions.toml files | One transaction of new files; an existing policy is never touched. | `core/commands/permissions.rs: execute_init` |
| `requirements_init` | read-only | single-file atomic | new `requirements.toml` | Atomic creation refuses an existing contract; inspect it and use requirements_check. | `core/commands/requirements.rs: execute_init` |
| `requirements_check` | read-only | read-only | none; optional server reads | Retry freely. Server read errors block. | `core/commands/requirements.rs: execute_check` |
| `permissions_audit` | read-only | read-only | none | Retry freely. | `core/permissions/audit.rs: audit` |
| `permissions_apply` | read-only | multi-file atomic | entity XML | One transaction; run `permissions_audit` before retrying. | `core/commands/permissions.rs: execute_apply` |
| `permissions` | read-only | read-only | none | Retry freely. | `core/commands/permissions.rs: execute` |
| `permissions_push` | read-only | server-partial | server permissions, baseline; with `platform`, grants and memberships on shared entities | Each set is written whole and read back; `platform` adds one item at a time and never removes. Run `permissions` or `permissions_audit` with `server` before retrying. | `core/commands/permissions.rs: execute` |
| `localization` | read-only | read-only | none | Retry freely. | `core/commands/localization.rs: execute` |
| `localization_write` | read-only | server-partial | localization files or tables | Inspect the listed changes and read tables back before retrying a push. | `core/commands/localization.rs: execute` |
| `users` | read-only | read-only | none | Retry freely. | `core/commands/users.rs: execute` |
| `users_write` | read-only | server-partial | server users, group memberships, test project | `users` shows what exists before retrying; nothing that exists is changed by a seed. | `core/commands/users.rs: execute` |
| `appkey` | read-only | server-partial | server application key, a profile file | `create` with `dry_run: false` makes the key, reads it back and, with `save`, writes the profile; a key made but not saved is read again with `show` and `save`, never made twice. `show` writes only the profile. | `core/commands/appkey.rs: execute` |
| `test` | read-only | server-partial | whatever the cases' services change | A dry run reads and checks the case files only; run, each case calls its service as written. Rerunning repeats every call. | `core/commands/service_tests.rs: execute` |
| `db_run` | read-only | server-partial | SQL, temporary server Thing | Inspect SQL effects and clean temporary Things before retrying. | `core/commands/db.rs: execute` |
| `db_query` | server-partial | server-partial | temporary server Thing | Run `db_clean` if temporary cleanup failed. | `core/commands/db.rs: execute` |
| `datatable_copy` | read-only | server-partial | target DataTable | Read the target back before retrying. | `core/commands/datatable_copy.rs: execute` |
| `db_clean` | read-only | server-partial | temporary server Things | List remaining temporary Things before retrying. | `core/commands/db.rs: execute` |
| `deploy` | read-only | server-partial | server, baseline, backups, localization tables | Read back imports and baseline; use plan and backups before retrying An agent imported first without allowances its namespaces drop is put back when its bundle fails to import. A localization failure after the imports says so; `localization_write` push retries the tables. | `core/commands/deploy.rs: execute` |
| `adopt_report` | read-only | read-only | none | Retry freely. | `mcp/refactor.rs: adopt_tool` |
| `adopt_apply` | multi-file atomic | multi-file atomic | entities, sidecars, declarations | Unlike the CLI form, this tool applies directly. Every entity and sidecar write happened or none did; a crash is finished or undone by the next command that takes the workspace lock. Retry. | `core/commands/adopt.rs: execute` |
| `handoff` | read-only | best-effort batch | `.twaco/handoffs` copies and metadata | List retries freely; a record plan writes nothing, and an applied record can be retried with a new name. | `core/commands/handoff.rs: execute` |
| `rename` | read-only | multi-file atomic | XML, sidecars, ledger, SQL | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry the reviewed plan. | `core/commands/rename.rs: execute` |
| `move_member` | read-only | multi-file atomic | XML, sidecars | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/relocate.rs: execute` |
| `new_building_block` | read-only | multi-file atomic | project files, config | A crash is finished or undone by the next command that takes the workspace lock; if it refuses, follow its message. Otherwise retry. | `core/commands/newblock.rs: execute` |
| `init_from_server` | read-only | best-effort batch | as `init --from-server`, in the MCP root | Plans unless `apply`; applied, it behaves as `init --from-server --apply`: follow its message after a failure. | `core/commands/init_server.rs: init_from_server` |
| `entity_new` | read-only | multi-file atomic | entity XML, DataShape sidecar | The entity XML and every managed sidecar are created in one journaled transaction, or none is; a file that appeared since the plan is refused. Retry. | `core/commands/entity_new.rs: execute` |
| `network` | read-only | read-only | none | Retry freely. | `core/network.rs: show` |
| `network_write` | read-only | multi-file atomic | Network XML, new Thing XML | A dry run is default; apply writes every imported Thing and the Network XML in one journaled transaction; pull replaces only the Network XML after a fresh server read. Retry after a stale plan. | `core/commands/network.rs: import_execute` |
| `entity_edit` | read-only | multi-file atomic | entity XML, service sidecars, Things inheriting it | Every file is rewritten in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/entity_edit.rs: execute` |
| `retemplate` | read-only | single-file atomic | one entity XML | The entity is old or new; retry freely. | `core/commands/retemplate.rs: execute` |
| `mashup_describe` | read-only | read-only | none | Retry freely. | `core/mashup_describe.rs: describe` |
| `mashup_validate` | read-only | read-only | none | Retry freely. | `core/mashup_validate.rs: validate` |
| `widgets` | read-only | read-only | none | Retry freely. | `core/widgets_view.rs: list` |
| `mashup_templates` | read-only | read-only | none | Retry freely. | `core/mashup_templates_view.rs: list` |
| `mashup_new` | read-only | multi-file atomic | new entity file, mashup sidecars | The entity file and its content.json and custom.css are created in one journaled transaction, or none is; a file that appeared since the plan is refused. Retry. | `core/commands/mashup.rs: new` |
| `mashup_add` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: add` |
| `mashup_add_contained` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: add_contained` |
| `mashup_bind` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: bind` |
| `mashup_recipe` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: recipe` |
| `mashup_on` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: on` |
| `mashup_remove` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; cascade only removes proven bindings and events. Retry. | `core/commands/mashup.rs: remove` |
| `mashup_unbind` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: unbind` |
| `mashup_off` | read-only | multi-file atomic | mashup entity file, content sidecar | The entity file and managed content.json are replaced in one journaled transaction, or neither is; a file edited since the plan read it is refused. Retry. | `core/commands/mashup.rs: off` |
| `mashup_param` | read-only | multi-file atomic | affected mashup entity files, content sidecars | Every child and parent container is replaced through one journaled transaction, or none is; a file edited since planning is refused. Retry. | `core/commands/mashup.rs: parameter` |
| `mashup_event` | read-only | multi-file atomic | affected mashup entity files, content sidecars | Every child and parent container is replaced through one journaled transaction, or none is; a file edited since planning is refused. Retry. | `core/commands/mashup.rs: event` |
| `reproject` | read-only | multi-file atomic | entity files | Every moved file is rewritten and moved in one journaled transaction, or none is; a file edited since the plan read it is refused. Retry. | `core/commands/reproject.rs: execute` |
| `config_table` | read-only | server-partial | server table, a named backup file | A backup is old or new; read the table back before retrying restore. | `core/commands/config_table.rs: execute` |
| `logs` | read-only | read-only | none | Retry freely. | `mcp/data.rs: logs_tool` |
| `log_level` | read-only | server-partial | server log level | Read the current level and use the undo before retrying. | `core/commands/logs.rs: execute` |
| `repo` | read-only | read-only | none | Retry freely. | `mcp/content.rs: repo_tool` |
| `repo_write` | read-only | server-partial | server repository or local pull | Inspect copied paths before retrying. | `core/commands/repo.rs: execute` |
| `block` | read-only | read-only | none | Reads the repository and, with server, the server; retry freely. | `mcp/block.rs: block_tool` |
| `block_version` | read-only | multi-file atomic | the Project and entry point files | Writes both files or neither, refused if either changed since it was read; retry. | `core/commands/block.rs: version` |
| `extensions` | read-only | read-only | none | Retry freely. | `mcp/content.rs: extensions_tool` |
| `ext_check` | read-only | read-only | none | Reads an extension project's files only; retry freely. | `mcp/content.rs: ext_check_tool` |
| `ext_build` | best-effort batch | best-effort batch | the project's build output, node_modules | Runs the project's own build, which writes build/ or zip/ (and installs its dependencies the first time); retry. | `core/ext_build.rs: build` |
| `ext_new` | read-only | multi-file atomic | extension project files | Writes the new project's files into a missing or empty folder and removes them again on a failure (in-process only). Retry. | `core/ext_new.rs: Plan::apply` |
| `ext_add` | read-only | multi-file atomic | extension project files | Writes the new class or widget files, then replaces metadata.xml whole, refused if it changed since it was read; a failure removes the new files again (in-process only). Retry. | `core/ext_new.rs: Plan::apply` |
| `ext_deploy` | read-only | server-partial | extension project files, server extension | Runs the build and, with bump, rewrites packageVersion, put back if the build or install fails; the install is one upload. Check `extensions` before retrying. | `core/ext_deploy.rs: deploy` |
| `extension_write` | read-only | server-partial | server extension | Check installed extensions before retrying. | `core/commands/extensions.rs: execute` |
| `bundle` | read-only | single-file atomic | generated bundle | The generated bundle is old or new; retry freely. | `core/commands/bundle.rs: execute` |
| `search` | read-only | read-only | none | Retry freely. | `mcp/content.rs: search_tool` |
| `model_resolve` | read-only | read-only | none | Retry freely. | `mcp/content.rs: model_resolve_tool` |
| `entity_inspect` | read-only | read-only | none | Retry freely. | `mcp/content.rs: entity_inspect_tool` |
| `access_explain` | read-only | read-only | none | Retry freely; nothing is invoked. | `mcp/content.rs: access_explain_tool` |
| `entity_get` | read-only | read-only | none | Retry freely. | `mcp/content.rs: entity_get_tool` |
| `export` | read-only | server-partial | local export or server repository | Local export is atomic; source-control apply needs a new plan and inspection. | `core/commands/export.rs: execute` |
| `package` | single-file atomic | single-file atomic | named package file | The output is old or new; retry freely. | `core/commands/package.rs: execute` |
| `import` | read-only | server-partial | server entities | Compare again with the server before retrying. | `core/commands/imports.rs: execute` |
| `settings` | read-only | read-only | none | Retry freely. | `mcp/info.rs: settings_tool` |
| `catalog` | read-only | read-only | none | Retry freely. | `mcp/info.rs: catalog_tool` |
| `contract_diff` | read-only | read-only | none | Retry freely. | `core/contract.rs: diff` |
| `impact` | read-only | read-only | none | Retry freely. | `mcp/info.rs: impact_tool` |
| `trace` | read-only | read-only | none | Retry freely. | `mcp/info.rs: trace_tool` |
| `unused` | read-only | read-only | none | Retry freely. | `mcp/info.rs: unused_tool` |
| `docs` | read-only | read-only | none | Retry freely. | `mcp/info.rs: docs_tool` |
| `doctor` | read-only | read-only | none | Retry freely. | `mcp/info.rs: doctor_tool` |
| `guide` | read-only | read-only | none | Retry freely. | `mcp/info.rs: guide_tool` |
| `help_search` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `mcp/info.rs: help_search_tool` |
| `help_page` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `mcp/info.rs: help_page_tool` |
| `javadoc` | single-file atomic | single-file atomic | user cache file | The cache file is old or new; retry freely. | `mcp/info.rs: javadoc_tool` |
| `call` | read-only | server-partial | service effects | `dry_run` differs from CLI `call`; inspect service effects before retrying. | `core/commands/call.rs: execute` |
