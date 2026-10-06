# Quick start

From a folder of ThingWorx exports to a checked, deployed change. Allow ten minutes.

## 1. Install

Download the archive for your platform from the
[latest release](https://github.com/butteredstardust/twaco/releases/latest), unpack it, and
put `twaco` on your `PATH`. Or build it:

```sh
cargo install --git https://github.com/butteredstardust/twaco
twaco --version
```

## 2. Start from a repository of exports

twaco works on a git repository that holds a solution's entities as Composer exports them,
one entity per file, in folders named after their collection:

```text
my-solution/
  Projects/Acme.Dashboards.xml
  Things/Acme.Dashboards.Manager.xml
  ThingShapes/Acme.Dashboards.Management_TS.xml
  ThingTemplates/...
  DataShapes/...
  Mashups/...
```

Composer's **Export > To Source Control** writes this layout, one folder per project
(`<Project>/Things/...`); twaco takes either. The entities' own `projectName` decides which
project each belongs to, so several projects can share one repository.

## 3. Describe the solution

```sh
cd my-solution
twaco init
```

`init` reads the entities and proposes a `twaco.toml`: the projects, where their folders are,
and the script layout the files already use. Nothing is written. When it looks right:

```sh
twaco init --write
twaco projects
```

`--write` creates `twaco.toml`, and an `AGENTS.md` and `CLAUDE.md` for AI agents where none
exist. Add `depends_on` to a project in `twaco.toml` when it must deploy after another.
[Configuration](CONFIGURATION.md) lists every setting.

## 4. Put the scripts in files

```sh
twaco extract --all
```

Every service script is now a file under `src/`:

```text
src/Acme.Dashboards.Management_TS/services/GetDashboards/
  definition.xml     the service's signature: parameters, result, description
  script.js          its code
```

DataShapes, mashups and DataTable configuration get sidecars too. Commit the result: from
now on, edit the sidecars, not the XML.

```sh
twaco check
```

`check` runs every gate and exits 0 when no blocking gate fails (a `[[check]]` hook marked
`gate = false` only reports). On an existing repository, the first run may report formatting
and script-trap findings; fix them,
or see [User guide: gates](USER_GUIDE.md#the-gates) for what each gate means.

## 5. Connect a server

First keep twaco's local files out of git. `twaco init --write` (or `twaco init --agents`, for a solution
that already has its config) adds these lines to the solution's `.gitignore`, and `twaco doctor` warns
when they are missing. By hand, they are:

```gitignore
.twaco/profiles/
.twaco/lock
.twaco/lock.holder
.twaco/backups/
.twaco/transactions/
.twaco/types/
.twaco/platform.json
**/services/*/jsconfig.json
**/services/*/twaco-globals.d.ts
```

Backups matter most: a server's copy of an entity, a database Thing's password included, is kept
there as the server holds it.

Commit `.twaco/baseline.json` when it appears: it records what was last deployed or pushed
(or adopted with `entity status --record`), so a teammate's deploy can tell your change from theirs.

Then create `.twaco/profiles/default.toml` in the solution:

```toml
url = "http://localhost:8080/Thingworx"
username = "Administrator"
password = "..."
```

Or put it in `~/.twaco/profiles/default.toml` to share it between solutions, or set
`TWACO_URL`, `TWACO_USERNAME` and `TWACO_PASSWORD`. Then:

```sh
twaco doctor
twaco entity status --all
```

`doctor` shows what resolved and whether the server answers. `entity status` compares every
entity with the server.

## 6. Make a change

Edit `src/<Entity>/services/<Service>/script.js`, then:

```sh
twaco sync <Entity>                     # write the script back into the entity's XML
twaco check                             # the gates
twaco deploy --only <Entity>            # the plan: what would be imported, and any conflict
twaco deploy --only <Entity> --apply    # import it
twaco call <Thing> <Service> '{"name": "x"}' --with-logs
```

`call --with-logs` prints the result, then what the call wrote to ScriptLog and
ApplicationLog. The gates check structure, never behaviour: call the service, including the
inputs that should fail.

## 7. Give it to an agent

```sh
claude mcp add twaco -- twaco mcp
```

The agent now has twaco's tools, and AGENTS.md points it at `twaco guide workflow`. See
[MCP server](MCP_SERVER.md) for other clients.

## Next

- [User guide](USER_GUIDE.md): the change loop in depth, deploys, conflicts, adopting a
  designer's Composer work, and releasing.
- [Commands](COMMANDS.md): every command and flag.
