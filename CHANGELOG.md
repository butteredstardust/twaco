# Changelog

All notable changes to twaco are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- Rename now analyses service scripts with a real ECMAScript parser instead of a lexical scan,
  so calls split over lines or inside template literals are found, and text in strings,
  comments and regular expressions is not mistaken for code. A script the parser refuses
  (Rhino-only syntax such as `for each`) is no longer edited; its mentions are left for review.
- A variable counts as a Thing only if every declaration of it names the same `Things` entity and
  it is never reassigned or taken as a parameter. A service rename follows an `@function` tag
  only inside comments.

## [0.1.0] - 2026-10-03

The first public release.

### The repository

- `init` proposes a `twaco.toml` from the repository's entities and can write it; `projects`
  reports project roots and deploy order, and `doctor` checks resolved configuration, server
  reachability and missing requirements.
- `extract` and `sync` move service scripts, DataShape fields, mashup content and DataTable
  configuration between entity XML and sidecar files. Sync changes only the targeted byte spans,
  supports check-only and controlled add/remove modes, and can relayout script CDATA.
- `check` runs line-ending, sidecar, formatting, script-trap, code-order and project-validation
  gates plus declared hooks; optional live checking parses every script on the server and fails
  closed when it cannot run.
- `fmt` formats service scripts with the built-in TypeScript formatter or reports changes with
  `--check`.
- `types` generates editor declarations from repository entities and platform metadata, caches
  platform declarations, and can run TypeScript once across every service.
- `catalog` provides an offline, searchable catalog of services with their signatures,
  descriptions, origins and implementation status.
- `adopt` compares a Composer export with the repository after normalising platform-managed
  differences, reports the effective changes, and can apply them to selected entities.

### Refactoring

- `rename` plans and applies entity, prefix, DataShape field, service, parameter, configuration
  table and property renames across the applicable XML, sidecars, scripts, mashups and
  configuration. It checks the result in a scratch workspace, applies file changes atomically
  with rollback, records server follow-up in a typed ledger, and requires an explicit SQL or
  no-SQL choice when an entity, prefix or field rename touches DBConnection tables.
- `move` and `copy` relocate or duplicate services and properties among Things, Thing Templates
  and Thing Shapes, with inheritance conflict checks, caller reporting, optional renaming and an
  optional forwarding delegate for a moved service.
- `retemplate` changes a Thing's template, a template's base or implemented shapes and reports
  the effective members gained and lost throughout the inheritance tree; changes that orphan
  stored values or references require explicit acceptance.
- `new building-block` plans or creates standard, abstract and implementation building blocks as
  repository files, including their project, entry point, manager where applicable, groups,
  organization and `twaco.toml` project entry.

### The server

- `entity status`, `entity get` and `entity push` compare repository, server and recorded baseline
  state, fetch raw server XML, and plan or import one entity while refusing conflicts; forced
  overwrites save the server copy unless backups are disabled.
- `bundle` builds an ordered importable document offline and can exclude configured UI
  collections; `deploy` runs repository checks, bundles in project order, live-parses scripts,
  checks conflicts, imports, reads entities back, runs deploy services and records the baseline,
  with entity, project and backend-only scopes.
- `entity delete` plans dependency-guarded deletions, saves server XML in backup sets by default,
  deletes in dependency order and confirms absence; `entity restore` lists backup sets and plans
  or imports all or selected entities back, confirming each result.
- `entity carry` plans or copies run-time, design-time and visibility permissions from renamed
  entities to their replacements, maps principals through the rename ledger, reads changes back
  and records completed work.
- `datatable copy` plans or copies rows from a replaced DataTable, mapping fields by identical
  name, the rename ledger or explicit mappings, with controls for unmapped fields and populated
  targets.
- `db run`, `db query` and `db clean` execute SQL commands in a transaction by default, run
  read-only queries and clean up temporary Database Things; writes plan by default.
- `call`, `logs`, `logs level`, `settings` and `config-table` call services with optional captured
  logs, filter server logs, inspect or change logger levels, search subsystem settings without
  displaying PASSWORD values, and read, diff, back up or restore configuration tables.
- `repo`, `ext`, `export` and `import` manage file-repository content, inspect and change extension
  packages, export entities, collections, projects or source-control layouts, and plan or import
  XML, zip and source-control content while preserving property values and table rows by default.

### Release

- `package` creates offline importable bundles, source-control layout zips and DPM-style extension
  packages for a project or the full solution.

### Knowledge and MCP

- `guide` lists, searches and reads the compiled workflow, verified platform quirks,
  service-code reference and solution-local documentation.
- `help` searches the ThingWorx Platform help for the server's version or another release and
  reads pages or selected sections as cached Markdown.
- `javadoc` searches ThingWorx Platform Java API classes and members and reads cached class,
  method, parameter, return and exception documentation.
- `mcp` serves 39 tools over stdio to MCP clients, with compact summaries, validated arguments
  and dry runs by default for tools that can write to a server.

[0.1.0]: https://github.com/butteredstardust/twaco/releases/tag/v0.1.0
