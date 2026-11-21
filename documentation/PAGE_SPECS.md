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

The primitive elements are `grid`, `label`, `heading`, `button` and
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
generated role, in the range 1000?999999 (four to six digits). Template numbers are reserved. A collision
is an error and never causes renumbering. Wrapper roles name their enclosed widgets, and sources,
bindings and events use semantic addresses for deterministic UUIDs. Ordinary authoring retains
UUIDv4 through the document's injectable generator. Identical specs and offline metadata produce
identical artifacts. Mapping declaration order does not affect output; node array order does.

The compiler calls the same in-memory new/add/set/source/bind/on/recipe operations as command
planners. XML is filled through `mashup::sync` scanner spans and splices. The compiler neither
serializes entity XML nor invokes the CLI. Future elements extend the node enum and reuse this
session. Reusable component libraries remain separate work.


## Element reference

These nodes also accept the common ID, props, style, surface and size fields. IDs are
required, including on spacers, KPI tiles and header actions. Generated internal names use
`<id>--<role>`; choose other names for authored nodes. All output uses native widgets.
Examples below are nodes in a slot; `rows` is a declared INFOTABLE source with `Name` and
numeric `Count` fields. Bindings and events use authored IDs, including tile and action IDs.

| Element | Existing operation reused | Example |
| --- | --- | --- |
| `page_header` | heading/label/button authoring; v1 header row | `- page_header: {id: header, title: Alerts, help: Current alerts, actions: [{id: refresh, label: Refresh}]}` |
| `toolbar` | row authoring and `recipe toolbar --band` | `- toolbar: {id: tools, children: [{text_field: {id: search, hint: Search}}, {spacer: {id: space}}, {button: {id: refresh, label: Refresh}}]}` |
| `kpi_row` | `recipe card`, label bindings and equal panel widths | `- kpi_row: {id: totals, tiles: [{id: total, label: Total, value: {bind: rows.Count}, unit: alerts, color: '#123456'}]}` |
| `trend_tile` | `recipe card` caption, value/unit/arrow labels in one centred row | `- trend_tile: {id: total, label: Total, value: {bind: rows.Count}, unit: '%', trend: up, delta: {bind: rows.Name}}` |
| `detail_pair` | row container, caption/spacer/value labels, optional status dot | `- detail_pair: {id: first, label: Name, value: {bind: rows.Name}, kind: ok}` |
| `card` | `recipe card` | `- card: {id: state, title: Status, value: {bind: rows.Count}, status: Healthy, kind: ok, children: [{label: {id: detail, text: Details}}]}` |
| `filter_panel` | filter-panel template and filter-footer authoring rules | `- filter_panel: {id: filters, fields: [{text_field: {id: query, hint: Search}}], footer: [apply, reset]}` |
| `tabs` | tabset authoring (`sync_tabs`), tab-content template, add-contained | `- tabs: {id: views, tabs: [{label: Overview, content: [{label: {id: overview, text: Overview}}]}, {label: Details, mashup: Acme.Child_MU}]}` |
| `chart` | `recipe chart` | `- chart: {id: counts, kind: bar, data: rows, x: Name, stacked: true}` |
| `spacer` | responsive container authoring and v1 spacer rule | `- spacer: {id: space}` |
| `date_range` | datepicker authoring, `DateRange: true` | `- date_range: {id: dates}` |
| `dropdown` | `recipe picker` | `- dropdown: {id: choice, options: rows, display: Name}` |
| `text_field` | textfield authoring | `- text_field: {id: query, hint: Search}` |
| `pagination` | `recipe pagination` | `- pagination: {id: pages, for: results}` |
| `grid` | grid authoring and `recipe columns` | `- grid: {id: results, data: rows, columns: [Name, {field: Count, title: Total}]}` |
| `label` | label authoring | `- label: {id: note, text: Details}` |
| `heading` | label authoring with header LabelType | `- heading: {id: title, text: Alerts}` |
| `button` | button authoring, `recipe enable-when` | `- button: {id: preview, label: Preview, variant: transparent, icon: 'cds:icon_view', enabled_when: results.selected}` |
| `widget` | catalogue authoring | `- widget: {id: input, type: ptcstextfield, props: {Text: Acme}}` |

Toolbar `band` defaults to true: the existing recipe supplies the grey 48px band. Set
`band: false` for a plain row. A spacer grows inside its row and pushes following controls
right. KPI tiles reuse the native card caption/value styling, equal flex widths and an optional
unit label; `color` styles the value label. Card `kind` defaults to `info` and accepts `info`,
`ok`, `warning`, `error`; value and status accept text or `{bind: ...}`.

Filter fields go in the template's filters slot; footer accepts `apply` and `reset` and uses
the existing primary/tertiary button rules. The 280px column includes the footer's 20px
gutters, leaving 120px for each button. Their generated IDs are `<id>--apply` and
`<id>--reset`, so events can invoke services through those names. Optional children go in the
flexible content slot. Tabs require 1?16 unique labels and exactly one of `mashup` or `content`
per tab. Inline content uses tab-content's body slot; contained mashups use the shared offline
parameter checks. Inline specs do not reserve the template's unused toolbar slot; an authored
toolbar provides the single band at any nesting depth. A pie chart requires `value: Count`; line and bar charts draw numeric series
through the chart recipe, and `stacked: true` requires bar. Pagination names a grid with data
and creates the same local slicing expressions and reset bindings as the pagination recipe.

Trend tiles go in a `kpi_row` through `children` (a `kpi_row` takes `tiles` or `children`). A
`trend_tile` has a caption (`label`), a bound `value` and optional `unit`, `trend` (`up`, `down`
or `flat`) and `delta` (text or `{bind: ...}`). Value, unit, direction glyph and delta sit in one
centred row under the caption, as in the kit's AssetKPI tiles. The direction glyph and delta are
coloured labels (green, red, grey); `ptcsicon` has no known styler, so it cannot carry the colour.
Direction is static: it does not follow a data field.

A `detail_pair` is a content-height row: optional status dot, caption left, spacer, bound value
right. Place it in a `card`, which then stretches its children. `kind` (`ok`, `warning`, `error`,
`info`) colours the dot. There is no StateDefinition mapping yet, so the dot colour is fixed.
Rows have no fixed height, which removes the inner overflow of the old card plus label rows.

A `button` accepts `variant` (`primary`, `secondary`, `tertiary`, `transparent`, `danger`, written
to `ButtonType`), `icon` (a `cds:` name checked against the 10.2 list, written to `SVGIcon`, as the
kit does) and `enabled_when: <grid>.selected`. The gate is the enable-when recipe: an expression
from the grid's `RowSelected` into the button's `Disabled`, with a stable widget ID derived from
the button.

Date ranges narrower than 300px (an explicit `Width` below that) get the hints `From` and `To`
instead of the automatic `DD-MM-YY`, which truncates in two short inputs. Explicit hint props win.

## Planned elements

These are proposals, not implemented elements. They compose native widgets and named parts; no
bitmap chart or custom HTML is needed.

| Proposed element | Native widgets and parts | Contract to add |
| --- | --- | --- |
| `trend_tile` sparkline | compact `ptcschartline` (plot/series, axes and legend hidden) | Series field and period caption; a bounded sparkline under the value. |
| `detail_pair` status mapping | `Acme.Status_ST` StateDefinition | Map a bound status to the dot colour. |
| `chart: {kind: combo}` | `ptcschartcombo` with native bar and line series, primary/secondary axes and legend; chart/plot/legend parts for spacing and theme styling | Per-series field, bar/line kind, axis and label; reuse INFOTABLE binding and chart recipe validation. Replaces A's line-only approximation. |

Part selectors and compact chart settings must be checked against the embedded platform
metadata and kit exports when implemented; the plot/series names above describe native
parts to expose, not additional accepted `style` keys today.

## Baseline and hand edits

Commit `src/<Mashup>/.page-baseline.json` beside `page.yaml`. Format 1 records compiler
`page-v1`, rule version `v1`, ID policy `twaco-page-ids-v2`, the node-to-widget ID map, generated ownership paths, generated
content and CSS, and managed XML identity values (`name`, `projectName`). XML outside the
mashup payload and identity remains unmanaged. The baseline always records the last generated
artifacts, never the merged result. Unsupported baseline versions are refused. Baselines made before ID policy v2 are recognised
and refused with a compatible-version instruction; they are never silently renumbered.

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


## Worked examples

The benchmark pages use the small offline [Acme solution](../tests/fixtures/mashup/pages/bench_solution/twaco.toml).
Copy that directory to a scratch workspace and pass one of these specs to
`twaco mashup new <page.name> --project Acme.Bench --spec <file>` to review a plan.
Add `--apply` to create local files, then `twaco mashup build <page.name>` should plan no changes.
Deployment is separate. Golden files beside each spec record XML, content, CSS and stable IDs;
CI also checks both front ends, rebuilds, retained hand edits and conflicts.

| Benchmark / kit | Spec | Remaining differences from the kit |
| --- | --- | --- |
| A / AssetKPI | [KPI dashboard](../tests/fixtures/mashup/pages/bench_a/page.yaml) | Native trend tiles replace the contained trend doodads (static direction, no sparkline); detail pairs carry status dots with fixed colours; a line chart replaces the combo chart. |
| H / AssetListTopFilter | [Asset list](../tests/fixtures/mashup/pages/bench_h/page.yaml) | Search and refresh invoke SearchAssets. Its contract has only query, so category/date filtering needs another service contract. View Details is selection-gated but navigation is outside the event subset. Pagination slices the grid locally. |
| J / AlertsAndNotifications | [Alerts tabs](../tests/fixtures/mashup/pages/bench_j/page.yaml) | Asset fields substitute for a dedicated alert contract. Severity/date controls need filtering services; Form is styled but cannot open a popup through spec events yet. Application uses a second grid over the same source. |
| K / AdminMetrics | [Metrics filters](../tests/fixtures/mashup/pages/bench_k/page.yaml) | Apply binds all side-filter values; Reset is present but has no reset recipe in specs. The template chevron is retained; no collapsible recipe is exposed. Toolbar search/category/date and View Details are visual controls awaiting separate filter/navigation wiring. Pagination uses the existing local slicing recipe. |
| N / Documents | [Document actions](../tests/fixtures/mashup/pages/bench_n/page.yaml) | Transparent icon buttons as in the kit; Preview is enabled by the grid's selection. Preview window and download actions need navigation/download event support. |

These are offline acceptance fixtures, not newly measured visual scores. A and H still require
live comparison at 1440x900 and 1440x717 before claiming their previous score floor.
All examples use native widgets. Their service contracts are synthetic and contain no live data.

Spec-built blank pages own a 24px gutter; inline tab bodies own none. Toolbar bands use a fixed
48px height with centered controls. Responsive placement wrappers size to content, while the
spacer branch alone grows. Date ranges default to the kit's 340px width, date-only mode and
`DD-MM-YY`; a date range bound to a service parameter starts empty so an unset filter does
not silently restrict results to today. Set `InitializeWithCurrentDateTime: true` explicitly
to opt in. Explicit props (for example a narrow filter's Width) still win. Cards and KPI rows
default to content height; charts stay inside their authored panels and share the remaining
height, retaining their minimum height so a short page scrolls above the chart region.
Existing unversioned
mashups continue using legacy rules and unchanged legacy goldens.
