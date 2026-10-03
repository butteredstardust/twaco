# Knowledge

Code alone does not tell an agent, or a new team member, how ThingWorx behaves, what a
solution's services are for, or which platform call takes which parameter. twaco brings that
knowledge to the command line and to MCP, in five places.

| Command | Answers | Needs |
| --- | --- | --- |
| `twaco guide` | How do I work here, and what does the platform do that I would not expect? | nothing |
| `twaco catalog` | Which services exist, where, with what signature, and what are they for? | the repository |
| `twaco types` | Is this script calling things that exist, with the right arguments? | the repository, TypeScript |
| `twaco help` | What does the ThingWorx documentation say about this? | internet, once |
| `twaco javadoc` | What does this platform class or Resource service take and return? | internet, once |

## `twaco guide`

Topics built into twaco, read and searched together with the solution's own markdown.

- **`workflow`:** how to work on a solution with twaco: the change loop, deploys, a designer's
  drop, releases, and what never to do on a live server.
- **`quirks`:** over fifty behaviours of a running ThingWorx server that the documentation does
  not tell you. Each was verified against a live instance. A few
  examples: the Importer rejects app keys; `SetConfigurationTableRows` replaces a row instead
  of patching it; a NaN in a result row fails the whole service; `const` is function-scoped in
  the script engine; a DataBinding with two PropertyMaps applies only the first.
- **`service-code`:** how to write ThingWorx services well: JSDoc, output contracts, building
  and reading InfoTables, DataTable writes, error handling, and a review checklist.
- **The solution's own documents:** AGENTS.md, CLAUDE.md and everything under `docs/`, or what
  `[knowledge] paths` names.

```sh
twaco guide                                  # the topics
twaco guide --search "configuration table row replaced"
twaco guide quirks --section "SetConfigurationTableRows"
twaco guide workflow
```

Search ranks `##` sections by how many of your words they hold, then by how rare those words
are, with a word in a heading counting extra. Common words are dropped, a word's ending does
not matter (`replaced` finds `replaces`), and words spelled apart match code that joins them
(`add member` finds `AddMember`). A long topic read without `--section` gives its outline.

`guide` works outside a solution too; then it has only the built-in topics.

## `twaco catalog`

Every service the repository's entities can call, derived from the XML each time, so it is
never stale:

```sh
twaco catalog                                # every entity with services
twaco catalog Acme.Dashboards.Manager        # one entity, or the last part of its name
twaco catalog --search share --json          # services whose name, parameters or description match
```

A Thing or ThingTemplate lists every service it can call, with where each comes from: its
own, a ThingShape's, or a template's up the chain, the nearest definition winning. A
ThingShape lists its own services and what implements it. Each service shows its parameters
and types, its result, its description, and whether code exists for it.

## `twaco types`

`twaco types` writes TypeScript declarations for every entity, DataShape and service, and a
`jsconfig.json` beside every service script. Editors such as VS Code then autocomplete
`me.`, `Things["..."]`, InfoTable rows and service parameters. `twaco types --platform` adds
the server's own Resources, base templates and shapes, cached in `.twaco/platform.json`.

`twaco types --check` type-checks every service at once with TypeScript (about a second on
a solution of a hundred services). Typical findings are a misspelled property, a missing
parameter, or a service called with an argument it does not take.

## `twaco help`

The ThingWorx Platform help center, searched and read as Markdown. It is fetched from PTC's
public site for your server's version (or `[help] version`) and cached in your user cache
folder.

```sh
twaco help search "configuration tables"
twaco help page <page> --section "Creating a Configuration Table"
```

## `twaco javadoc`

The ThingWorx Platform Java API documentation, version 10.1.0, the one PTC publishes.
Search it by class or member name, and read a class or one member as Markdown:

```sh
twaco javadoc search InfoTableFunctions.Sort
twaco javadoc class InfoTableFunctions --member Sort
twaco javadoc class InfoTable
```

It is most useful for two things: the Java methods of objects a script holds (`InfoTable`,
`ValueCollection`, `Thing`), and the exact parameter names of Resource services. `Sort` takes
`ascending`, not `isAscending`, and a call with the wrong name does not fail; the parameter
simply never arrives.

## Writing your own

The most valuable knowledge about a solution is what only its people know: what it is for,
where its data lives, what must stay true, and what broke before. Keep it in the solution's
AGENTS.md and `docs/`, where `guide` finds it. `twaco init --agents` starts an AGENTS.md
with a place for each. A platform behaviour that would surprise anyone, not only your
solution, is worth contributing to the `quirks` topic: see [CONTRIBUTING](../CONTRIBUTING.md).
