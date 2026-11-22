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

The spec rule version is required and pinned to `v1`. Ordinary `mashup new` commands
also default to v1; pass `--rules legacy` (MCP `rules: "legacy"`) to opt out. Existing
mashups retain their `twacoPageRules` during add, set and recipe edits; no marker means legacy.
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
| `chart` | `recipe chart` | `- chart: {id: counts, kind: bar, data: rows, x: Name, stacked: true}` or `kind: combo, series: [{field: Count, kind: bar}, {field: Total, kind: line}]` |
| `panel` | container with grey panel style; generated title label and toolbar band | `- panel: {id: box, title: Counts, toolbar: [{date_range: {id: dates}}], children: [{chart: {id: counts, kind: bar, data: rows, x: Name}}]}` |
| `slider` | `ptcsslider` | `- slider: {id: level, min: 0, max: 100, value: 35, labels: true}` |
| `kpi_dial` | `ptcskpidial` | `- kpi_dial: {id: dial, title: Level, kind: half, unit: '(%)', value: {bind: level.Value}}` |
| `spacer` | responsive container authoring and v1 spacer rule | `- spacer: {id: space}` |
| `date_range` | datepicker authoring, `DateRange: true` | `- date_range: {id: dates}` |
| `dropdown` | `recipe picker` | `- dropdown: {id: choice, options: rows, display: Name}` |
| `text_field` | textfield authoring | `- text_field: {id: query, hint: Search}` |
| `pagination` | `recipe pagination` | `- pagination: {id: pages, for: results}` |
| `grid` | grid authoring and `recipe columns` | `- grid: {id: results, data: rows, columns: [Name, {field: Count, title: Total}]}` |
| `label` | label authoring | `- label: {id: note, text: Details}` |
| `heading` | label authoring with header LabelType | `- heading: {id: title, text: Alerts}` |
| `button` | button authoring, `recipe enable-when` | `- button: {id: preview, label: Preview, variant: transparent, icon: 'cds:icon_view', enabled_when: results.selected}` |
| `row` / `column` | responsive flex containers, equal fill or positive `weight`, optional pixel `gap` | `- row: {id: split, size: fill, gap: 12, children: [{column: {id: main, weight: 2}}, {column: {id: side, weight: 1}}]}` |
| `side_panel` | `recipe side-panel`; children go in its tab container | `- side_panel: {id: side, title: Compare, help: Pick assets, tab: Filters, children: [{label: {id: note, text: Hello}}]}` |
| `list` | `recipe picker --multiple --no-none-row`, `recipe empty-state` | `- list: {id: picked, data: all.selected, display: Name, selection: multiple, empty: No results.}` |
| `collection` | `mashup collection add` code (cell mashup, mapping, preset, `--columns`), `recipe empty-state` | `- collection: {id: cards, data: rows, per_row: 4, cell: {title: Name, kpi_dial: Count}}` or `mashup: Acme.Cell_MU, map: {Name: Name}` |
| `nav_menu` | label, button and text-field authoring (`recipe nested-menu` for Menu entities) | `- nav_menu: {id: menu, search: Search Menu..., groups: [{title: Group 01, items: [Page 01]}, {title: Group 02, items: [Page 02, Page 03]}]}` |
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
through the chart recipe, and `stacked: true` requires bar. A `combo` chart uses the native
`ptcschartcombo` and requires `series` (1 to 8 entries): each `field` is a numeric column and each
`kind` is `bar` (column) or `line`; they are written to `DataField<n>`, `Series<n>Type` and
`NumberOfSeries`. Each series accepts `title` and `axis: primary|secondary` (primary by default).
`secondary` emits `DisplaySeries<n>On: yaxis2`, enables two axes with `NumberOfYaxes: 2`, and
sets the right `YAxis2Label` from that series' title (or field). When several series share the
secondary axis, the last secondary series supplies its label. Combo charts disable legend grouping
(`DisableLegendGrouping: true`) to keep series in one compact bottom legend without axis headings.
Unknown axis values are located errors. Pagination names a grid with data
and creates the same local slicing expressions and reset bindings as the pagination recipe.

Trend tiles go in a `kpi_row` through `children` (a `kpi_row` takes `tiles` or `children`). A
`trend_tile` has a caption (`label`), a bound `value` and optional `unit`, `trend` (`up`, `down`
or `flat`) and `delta` (text or `{bind: ...}`). Value, unit, direction glyph and delta sit in one
centred row under the caption, as in the kit's AssetKPI tiles. The direction glyph and delta are
coloured labels (green, red, grey); `ptcsicon` has no known styler, so it cannot carry the colour.
Direction is static: it does not follow a data field. Optional
`sparkline: {field: Value, x: year, data: history}` adds a native `ptcschartline` under the value.
`data` defaults to the value binding's source; supply it when history comes from another service.
Service All Data binds directly to chart Data; `XAxisField` and `DataField1` select the fields
with `NumberOfSeries: 1`. Spark view hides axes, rulers, title, legend, markers and values. Both
the chart and its wrapper stay 40px high; widget styles override the theme's 160px chart minimum
and remove chart-layout padding. Native axis allowances are zero, and title, notes, legend,
action and zoom parts are hidden explicitly. The native optional `YAxisMinimumValues` and
`YAxisMaximumValues` remain unset for the data's automatic range. `recipe sparkline` uses the
same properties and styles.

A `detail_pair` is a content-height row: optional status dot, caption left, spacer, bound value
right. Place it in a `card`, which then stretches its children. `kind` (`ok`, `warning`, `error`,
`info`) or the alias `status` colours a fixed dot. Alternatively, `state: Acme.Status_ST`
uses the bound value to colour a native 12px circle through `ShapeFormatter`. The shape is a
member of the caption container with a 12px floor, so it shares the caption's height instead
of depending on an empty placement branch. `recipe label-pairs --status-state` already places
its native circles beside captions. The StateDefinition is resolved offline from the solution and embedded as the same snapshot used by grid columns;
unknown definitions are located errors. `state` cannot be combined with fixed `status`/`kind`.
If any direct pair in a card has a dot, every pair reserves the same dot-column width.
Rows have no fixed height, which removes the inner overflow of the old card plus label rows.

A `button` accepts `variant` (`primary`, `secondary`, `tertiary`, `transparent`, `danger`, written
to `ButtonType`), `icon` (a `cds:` name checked against the 10.2 list, written to `SVGIcon`, as the
kit does) and `enabled_when: <grid>.selected`. The gate is the enable-when recipe: an expression
from the grid's `RowSelected` into the button's `Disabled`, with a stable widget ID derived from
the button.

Date ranges narrower than 300px (an explicit `Width` below that) get the hints `From` and `To`
instead of the automatic `DD-MM-YY`, which truncates in two short inputs. Explicit hint props win.

A `panel` is the kit's framed chart box: grey `#F7F7F7` fill, 1px `#E4E7E9` border, column layout,
equal flexible width (`flex-grow: 1`, override with `props: {flex-grow: 2}`) and an 8px margin. An
optional `title` becomes a regular-size label and `toolbar` a 48px band at the top of the panel
(`<id>--title`, `<id>--toolbar`). Charts in a panel keep their minimum height and share the
remaining height. Place panels in a `row: {id: charts, size: fill, children: [...]}`. Raw `widget` flexcontainers remain supported.

A `slider` writes `Minimum`, `Maximum`, `Value`, `DisplayMinMaxLabels` (`labels`, default true) and a
`Label` from `title`; it defaults to `size: fill` so its wrapper spans the row, with the kit's 110px
height. A `kpi_dial` writes the kit's dial settings per `kind`: `dial` (start 45, end 315 degrees,
clockwise), `half` (90 to 270), `horizontal` (left to right) and `vertical` (bottom to top);
`min`/`max` default to 0/100, `unit` is written verbatim to `UnitOfMeasure` (the kit uses `(%)`) and
shows the unit label, `title` is the dial's label. Its `value` is a number or `{bind: node.Prop}`,
for example the slider's `Value`. A `card` holding a dial stretches it.

Card and trend-tile titles use the regular label size (`LabelType: label`), as the kit does, instead
of the card recipe's sub-header; the recipe command keeps its own size.

## Side panels, lists, collections and navigation

These elements come from the kit's Compare and Administrator pages. Place a `side_panel` in a
`row` beside the content column.

A `side_panel` is the kit's grey left column: 276px wide, `#F7F7F7`, a 72px title row with the
`title` at header size and an optional `help` icon (a disabled transparent button with the
tooltip), over a one-tab strip (`tab`, default `Filters`). Its `children` go in the tab's
container, which stacks them. Its 250px inner width bounds text fields, including their
horizontal margins; filter columns apply the same bound. Smaller authored widths are retained.
It requires an empty column, as `recipe side-panel` does.

An `info` icon (heading, label and `page_header`) is the same disabled button after the text: the
heading or label becomes a row holding both and keeps its authored id (`<id>--title`,
`<id>--info` name the parts). Headings and labels use `cds:icon_info`, `page_header` the
`cds:icon_help_circle` help icon. `page_header` still takes `help` for a plain subtitle; `info` is the
kit's icon.

A `list` is a native `ptcslist` filled by `recipe picker`: it needs `data` and `display`, shows no
caption and no "(None)" row, and `selection: multiple` turns on `MultiSelect`. A list's rows are
also a data source: `data: <list>.selected` on another list binds the first list's
`SelectedItems` to its `Data`, and the `display` field is checked on the first list's source.

`empty: <text>` is the kit's "No results." state and works on a `list`, `collection` and `grid`
through `recipe empty-state`. A collection and a grid carry it in their own properties
(`NoDataText` with the eye icon, `NoResultsLabel`). A list has no such property, so the recipe
adds a centred notice (the eye icon over the full text) that replaces the list while its rows
are empty. Complementary visibility bindings hide the list's entire placement row, including
its border and reserved height, and show the notice's content-height row. The command recipe
uses the same bindings and sizing.

A `collection` is a native `ptcscollection` configured by the `mashup collection add` code.
Give either `mashup: <existing cell>` with `map: {row field: cell parameter}`, or `cell:
{title: <field>, kpi_dial: <numeric field>}`. With `cell:` the compiler writes a second page
for the cells: `<page>Cell_MU` (the page name without `_MU`, plus `Cell_MU`), a `kpi-tile`
page whose title label and dial are bound to parameters named like the fields. It is a
page spec of its own (`src/<page>Cell_MU/page.yaml`, built with the page spec's `parameters:`
and `{bind: param.<name>}`). `mashup new --spec` / `mashup build` keep it in sync while
its XML, sidecars, spec and baseline still match the last generated bytes. The parent
`.page-baseline.json` records their SHA-256 hashes. If any file was edited, the build keeps
all cell files and reports `cell edited, not regenerated` in CLI and MCP (`cell_notices`).
Older cells are adopted only when their spec, sidecars and XML still match their own
generated baseline; other existing cells are kept. Edit or rebuild a cell independently
to take ownership; delete its files to let the parent generate it again.
`per_row` works the cell width out for a collection `across` pixels wide (default 1400, as
`--columns` and `--across` do on the command) and a generated cell uses the `cards` preset
(framed 270px cards, no initial selection; `preset: cards` asks for it on a `mashup:` cell).
The shared cards preset leaves automatic first-row selection off. Uncaptioned v1 KPI dials
clear Composer's `Label` placeholder in spec and command builds; a title or authored label wins. A
collection fills the remaining height by default, like a grid.

A `nav_menu` is the kit's administration column drawn with native widgets: an optional search
field (`search` is its hint, with the search icon), each group's grey caption over its items,
and every item a full-width transparent button with 14px semibold dark text, 24px left
padding and square corners, the first drawn on the kit's `#e4e7e9` selected background.
Group captions use grey 14px text. Item
buttons are named `<id>--item-<group>-<item>` (1-based), so `events` can address them
(`menu--item-1-1.Clicked`). The kit builds this list from a service-fed collection with two cell
mashups and highlights the clicked row; the buttons only mark the first item (a recorded native
deviation), so use `recipe nested-menu` over a Menu entity for a menu that tracks the selection.

`filter_panel` takes `title` (the kit's "Filter" caption, a `title`-size label first in the
fields) and `collapsible: true`, which runs `recipe collapsible --tab inside` on the filter
column so the chevron sits on its top-right edge; `collapsible: false` removes the template's
chevron. Field rows retain their measured content heights through explicit CSS flex bases and
heights, including when a title and collapsible toggle are present. Spare height belongs to
the fields area above the footer, not to individual rows; command builds use the same pass. `text_field` takes `icon: search` (or `cds:icon_search`) for the field's icon, `button`
accepts the short icon name too (`icon: view`). Data `params` with literal values already
worked (`params: {kind: version}`). A page may declare `parameters: {Name: STRING}` (input
parameters, base type each) and bind them with `{bind: param.Name}`, which is how a cell page
reads its row.

Command equivalents: `recipe side-panel <mashup> <empty column> --title Compare --info "Tip"
--tab Filters`; `recipe picker <mashup> <list> --data ... --x Name --no-none-row --multiple`;
`recipe empty-state <mashup> <widget> --say "No results." [--data <list rows>]`;
`mashup collection add ... --columns 4 --across 1116 --preset cards`, with the cell made by
`mashup new <cell> --template kpi-tile --param Name:STRING:IN` and `mashup bind mashup:Name
ui:<label>.LabelText`; `recipe collapsible <mashup> <container> --tab inside`; a field's search icon is
`mashup set ... --prop Icon=cds:icon_search`; an information
icon is `mashup add ... button` with `--prop ButtonType=transparent --prop SVGIcon=cds:icon_info
--prop Disabled=true --prop TooltipField="Tip"`.
For a navigation item, use `mashup set <page> <button> --prop Size=standard
--prop ButtonType=transparent --prop LabelAlignment=left --prop Margin=0
--style width=100% --style box-sizing=border-box --style border-radius=0px
--style padding-left=24px --style label:font-size=14px --style label:font-weight=600
--style label:color=#080909`; add `--style background=#e4e7e9` for the selected item.
Group captions use `--prop LabelType=label --style label:font-size=14px
--style label:color=#6B7478`.

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

The benchmark pages (A, H, J, K, L, M, N, O) use the small offline [Acme solution](../tests/fixtures/mashup/pages/bench_solution/twaco.toml).
Copy that directory to a scratch workspace and pass one of these specs to
`twaco mashup new <page.name> --project Acme.Bench --spec <file>` to review a plan.
Add `--apply` to create local files, then `twaco mashup build <page.name>` should plan no changes.
Deployment is separate. Golden files beside each spec record XML, content, CSS and stable IDs;
CI also checks both front ends, rebuilds, retained hand edits and conflicts.

| Benchmark / kit | Spec | Remaining differences from the kit |
| --- | --- | --- |
| A / AssetKPI | [KPI dashboard](../tests/fixtures/mashup/pages/bench_a/page.yaml) | Native trend tiles replace the contained trend doodads (static direction, service-bound sparklines); detail pairs map numeric values through an offline StateDefinition; the right chart is a native bar+line combo with a secondary axis; charts sit in framed grey panels with their date toolbars. |
| H / AssetListTopFilter | [Asset list](../tests/fixtures/mashup/pages/bench_h/page.yaml) | Search invokes SearchAssets. Its contract has only query, so category/date filtering needs another service contract. View Details is the kit's transparent eye-icon button, selection-gated; navigation is outside the event subset. Pagination slices the grid locally. |
| J / AlertsAndNotifications | [Alerts tabs](../tests/fixtures/mashup/pages/bench_j/page.yaml) | Asset fields substitute for a dedicated alert contract. Severity/date controls need filtering services; Form is styled but cannot open a popup through spec events yet. Application uses a second grid over the same source. |
| K / AdminMetrics | [Metrics filters](../tests/fixtures/mashup/pages/bench_k/page.yaml) | Apply binds all side-filter values; Reset is present but has no reset recipe in specs. The template chevron is retained; no collapsible recipe is exposed. Toolbar search/category/date and View Details are visual controls awaiting separate filter/navigation wiring. Pagination uses the existing local slicing recipe. |
| O / WDK_Gauges | [Gauges and KPI](../tests/fixtures/mashup/pages/bench_o/page.yaml) | The kit's four needle gauges use a legacy `gauge` type with no authoring contract and no native widget; the native `ptcskpidial` (dial and half layouts) stands in, a recorded native deviation. Dials bind to one slider; the kit's absolute-positioned slider becomes a full-width slider with min/max labels. |
| L / Compare | [Compare](../tests/fixtures/mashup/pages/bench_l/page.yaml) | The kit's side lists are collections over cell mashups; the spec uses native lists (selected items feed the second list) with an eye-icon notice for the empty state. The side panel's one tab and 276px column follow the kit; the cell page is a generated KPI tile (title over a dial) instead of the kit's detail card. Not yet measured live. |
| M / Administrator | [Admin shell](../tests/fixtures/mashup/pages/bench_m/page.yaml) | The navigation is native buttons in a column (the first marked selected), not the kit's service-fed collection with section headers; the filter column's chevron is the kit's inside tab. Filters are not wired to the service, and Apply/Reset have no events in this spec. Not yet measured live. |
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

## Kit structures

V1 filter columns follow the exported AdminPage01/AdminMetrics containers 44/50/51/55/54:
fixed-width collapsible row 43 contains a `default`, growing `LastContainer` column 44.
That column contains the growing fields column with direct controls, then a footer column with a 1px divider and
`LastContainer` button row. A `title` label comes first (`LabelType: title`, margin `16`);
text fields use `0 16 16`, other filter controls `8 16 16`. Controls retain their native heights.
The kit's 273px controls are bounded to the narrower twaco column (text/date defaults remain
240px). Kit buttons have margins `24 8 24 24` and `24 24 24 8`; their widths are 108px here,
adapted from the kit's 120px to fit the existing 280px column. No per-field row, sum-of-rows
minimum or trailing remaining-space filler is generated. Command additions `--in filters`
and `--bottom filters`, plus panel/collapsible recipes, use the same layout code.

| Area | Exported kit structure | Previous twaco structure | V1 structure now |
| --- | --- | --- | --- |
| Filters (K, M, Q1) | AdminPage01/AdminMetrics: direct controls in 50; footer 51 wraps divider 55 and button row 54 | Fixed per-field rows, estimated fields minimum, sibling divider/footer and trailing filler | Direct controls and nested footer, with the kit margins above |
| Side panel (L) | Compare: 72px title band 101; growing row 103 holds tabset; growing tab column 105; fixed content bands and final list row 108 | 72px title plus tab row sized by placement; generic remainder filler | Explicit growing `default` tab row and `LastContainer` tab content; existing list/empty-state branches retained |
| Navigation (M) | Administrator: 72px title 7; 100px search-tab band 8/11; collection in growing row 34 | Native static caption/button rows and trailing filler | Static captions/buttons directly in one column; no filler. The service-fed collection/cell menu has no static-menu equivalent in the exports; the native button adaptation retains stable event addresses and font styling |
| Generated collection cell (L) | CompareCell: fixed 24px top band, 160px image band, growing details column, fixed 18px status band | KPI-tile template with title/dial, plus generic filler where no child grew | The title/dial-only adaptation retains the KPI template; filling containers use `LastContainer`. No image/status bands are fabricated: the cell API exposes neither role, and no exported kit has a title/dial-only cell |
| Main content | AdminPage01 5/19 and Compare 5/19: fixed header/toolbar/pagination bands around a `default` growing grid/collection row | Range rows with calculated floors and no `LastContainer` | Kit grid/collection remainder rows use `default` + `LastContainer` and keep any authored or chart height floor so tall pages scroll; the horizontal-width correction remains |

Other benchmark output changes are limited to kit remainder containers and removal of the
55d fillers: A/O gain the page-column remainder marker; H/N gain the grid-row marker and
`default` sizing; J gains tab/body remainder markers; Q2-Q4 gain markers on their fill regions.
All also gain the page-column marker. Their controls, event wiring, chart floors and width
corrections are retained. These markers follow the growing page/content columns in
AdminPage01 and Compare, and the tab content in Administrator/Compare.

These comparisons are offline inspections of XML exports, not new runtime measurements.

## Combined layouts and selection

Use `row` and `column` with `children` for responsive layouts. `size: fill` siblings share the
available space equally; a positive numeric `weight` sets their proportions (for example 2:1).
A weight also opts into fill sizing. `size: fit` keeps content height. `gap` is an optional
nonnegative pixel spacing between children. Explicit widget props still override defaults.

Charts in rows get equal widths, with a minimum height of 240px (a larger native minimum is
retained). Charts and grids share remaining column space. Status cards keep their content height.
Content minimums propagate through layout wrappers, and the page scrolls when those minimums
exceed the viewport; pagination remains in the page flow. Explicit fixed sizes can still constrain
content. A chart accepts `legend: false` to reclaim plot space in a dense chart row; legends
otherwise retain native settings at the bottom. No custom chart HTML or rendering is used.

A field-only `filter_panel` occupies 280px beside the main column. Its empty content pane is
hidden, fields stack from the top and the kit footer stays at the bottom. The filter column
is capped at the viewport height minus ancestor gutters, even when a taller main column grows
the page. The fields column grows; native controls keep their heights and margins.
The same v1 structure and viewport style apply to command-built filter-panel templates. A date range
defaults to 240px and short From/To hints; explicit Width props win.

Bind a detail value to a grid selection with `{bind: fleet.selected.ThingName}`, where `fleet`
is a grid id with `data`. The compiler validates the field against that data service's result
shape and creates a typed expression reading the named grid's `SelectedRows` first row. This
enables single selection if selection was disabled, follows that grid even when another grid
shares the service, and works with local pagination. No selection produces an undefined value;
it does not substitute the first row of All Data. `enabled_when: fleet.selected` remains the
boolean gate for buttons. Selection expressions have deterministic ids and rebuild without churn.

The combined examples [Q1](../tests/fixtures/mashup/pages/bench_q1/page.yaml),
[Q2](../tests/fixtures/mashup/pages/bench_q2/page.yaml),
[Q3](../tests/fixtures/mashup/pages/bench_q3/page.yaml) and
[Q4](../tests/fixtures/mashup/pages/bench_q4/page.yaml) use the same offline Acme solution.
Q3 hides chart legends; Q4 follows the selected asset and aligns mixed status pairs.
These are offline structural regressions, not measured visual scores.

A grid used by `<grid>.selected.<Field>` bindings selects the first row on load with the native
`DefaultSelectedRows: "1"` property. Set `auto_select: false` on that grid to opt out, or `true`
to request first-row selection explicitly. An authored `props.DefaultSelectedRows` takes priority.
Selection gates alone do not enable automatic selection.

In v1 pages, responsive containers do not own vertical scrolling. Their content floors propagate
through the center container so the page can scroll to pagination and footers at short heights.
Filter fields stack at full width from the top, including under a title and collapsible tab.
Generated field rows use width sizing rather than a height-based horizontal flex basis.
Columns of non-growing rows get a trailing empty growing container with zero minimum height:
it takes the spare height before the platform's bounding-box auto margins can spread the rows.
The shared spec and command helper also covers card stacks, side-panel content and navigation
columns when they have no growing child. Its ID derives from the parent without consuming widget
IDs, and command edits rebuild it after placement. The compact filter footer stays at the bottom.
Legacy layout output is unchanged.

Explicit `size: fit` containers clear generated fixed/max heights and keep natural content height,
including rows of detail cards. The page still owns vertical overflow.


Command equivalents: create an ordinary mashup with `mashup new <name> --template
<template>` to use the same page-overflow and filter fields/footer rule
operations by default. `recipe panel` in v1 keeps automatic content height. Use
`--rules legacy` at creation to keep legacy command layout. `recipe columns ... --auto-select-first` enables
first-row grid selection (`DefaultSelectedRows: "1"`, `RowSelection: single`).
`recipe chart ... --series Count:bar --series Total:line:secondary` shares the spec
combo series/secondary-axis and compact legend properties; command series labels
use their field names. `recipe sparkline <name> <container> --data <call> --x <field>
--value <field>` shares the trend tile's 40px chart properties and styles, with a
direct service-row binding. Existing command equivalents for status dots are
`recipe label-pairs --status <field> --status-state <StateDefinition>`, and picker
first-row selection is `recipe picker --auto-select-first`.
