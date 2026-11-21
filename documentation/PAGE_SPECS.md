# Page specs

Create a mashup with `twaco mashup new Acme.Alerts_MU --spec alerts.page.yaml --apply`.
This copies the spec to `src/Acme.Alerts_MU/page.yaml`. Edit that file and run
`twaco mashup build Acme.Alerts_MU` to review the plan, then repeat with `--apply` to write it.
Both commands accept `--json`. MCP uses `mashup_new` with `spec` and `mashup_build` with
`mashup`; `dry_run` defaults to true. These operations are entirely offline.

```yaml
version: 1
rules: v1
page:
  name: Acme.Alerts_MU
  template: list-page
  title: Alerts
  theme: PTC Convergence Theme
  data:
    alerts:
      service: Acme.Data.GetAlerts
      on: load
      params: {limit: 10}
  slots:
    toolbar:
      - button: {id: refresh, label: Refresh}
    body:
      - grid:
          id: alerts_grid
          data: alerts
          columns:
            - {field: Name, title: Device, width: 180}
            - {field: Severity, status: Acme.Severity_ST}
  events:
    - {on: refresh.Clicked, invoke: alerts}
```

The current elements are `grid`, `label`, `heading`, `button` and
`widget: {id: ..., type: ptcs...}`. Every authored node requires an ID containing ASCII
letters, digits, underscores or hyphens. Nodes accept `text` (also spelled `label`), catalogue
`props`, a list of `style` settings in the `mashup set --style` grammar, `surface: true|false`,
`size: fit|fill|{fixed: N}`, and `children`. Children require a container widget. Siblings follow
the parent's row or column direction. A title becomes a heading in the template's title slot,
or in its center when it has no title slot. Menu templates requiring menu configuration are
refused by this subset.

Grid columns have `field`, optional `title`, pixel `width`, date `format` and a StateDefinition
name in `status`. The existing columns recipe validates fields and embeds state snapshots.
Services, themes and StateDefinitions are resolved from the offline solution and its cached
platform metadata, using the same contracts as authoring commands.

Data triggers are `on: load`, `on: <node>.<event>` or `on: every Ns` (positive integer seconds).
An omitted trigger permits an explicit event to invoke the call. Fixed `params` use the service's
parameter types. A property or parameter can instead use `{bind: input.Text}`; a data source is
addressed by its declaration name, a result field by `alerts.Name`, and selected rows by
`alerts.SelectedRows`. Event `invoke` accepts a data declaration name or a node's service.
References need no `ui:` or `data:` prefixes. Unknown fields, repeated YAML keys, duplicate node
IDs, unresolved symbols and contract errors report `page.yaml:line:column`.

The rule version is required and pinned to `v1`; existing commands keep their legacy behavior.
Widget numeric suffixes derive from SHA-256 of the ID policy version, authored node ID and
generated role, within Composer's signed 32-bit range. Template numbers are reserved. A collision
is an error and never causes renumbering. Wrapper roles name their enclosed widgets, and sources,
bindings and events use semantic addresses for deterministic UUIDs. Ordinary authoring retains
UUIDv4 through the document's injectable generator. Identical specs and offline metadata produce
identical artifacts. Mapping declaration order does not affect output; node array order does.

The compiler calls the same in-memory new/add/set/source/bind/on/recipe operations as command
planners. XML is filled through `mashup::sync` scanner spans and splices. The compiler neither
serializes entity XML nor invokes the CLI. Future elements extend the node enum and reuse this
session; components and the remaining intent-level elements are separate work.


## Baseline and hand edits

Commit `src/<Mashup>/.page-baseline.json` beside `page.yaml`. Format 1 records compiler
`page-v1`, rule version `v1`, the node-to-widget ID map, generated ownership paths, generated
content and CSS, and managed XML identity values (`name`, `projectName`). XML outside the
mashup payload and identity remains unmanaged. The baseline always records the last generated
artifacts, never the merged result. Unsupported baseline versions are refused.

For baseline B, current C and generated G, build keeps C when G equals B, applies G when C
equals B, and accepts identical changes. Other changes conflict. Widgets merge by ID even when
moved; properties and style maps merge by key; sources, service calls, bindings and events merge
by their IDs or names. Child order is compared independently of value edits. Competing order
changes and deletion versus edit conflict. Unkeyed compound values, including ambiguous arrays
and custom CSS text, are compared conservatively as complete values. A CSS hand edit survives
when generation leaves the CSS unchanged.

Extra current widgets and properties stay unmanaged. Unchanged JSON values retain their original
spelling; XML changes use scanner spans and splices. A no-op build writes nothing. Reports show
created, changed and unchanged nodes, retained hand edits, and conflicts with a widget/property
path and a spec line. Resolve a conflict by reconciling the spec and hand edit, then plan again;
there is no force option. Conflicts write no artifacts or baseline.

An existing mashup without a baseline can adopt a spec only when all generated values already
match current values (extra unmanaged widgets/properties are allowed). Otherwise reconcile the
spec and mashup first. If XML and sidecars disagree, explicitly run `twaco sync` to keep the
sidecars or `twaco extract` to keep XML before building.

Apply takes the workspace lock, reloads and validates, and uses one journaled transaction for
XML, content.json, custom.css and the baseline (plus the copied spec for new mashups). Inputs
are rechecked before any installation, including unchanged files; stale plans are refused.
Transaction failures roll back, and the existing workspace-lock recovery handles interrupted
transactions. Build changes local files only; deploy remains a separate command.
