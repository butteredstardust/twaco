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

Page keys beyond the above: `master: Acme.Master_MM` and `use_master_theme: true` set the mashup's
master; `root: {props: {...}}` sets properties of the template's root container (a `null` removes
one); `layout: as-built` keeps the v1 layout rules off a page that adopts an existing mashup (see
[Adopting an existing mashup](#adopting-an-existing-mashup)).

Parameters are `name: TYPE`, `name: TYPE:DIRECTION` (`BOTH`, `IN`, `OUT`), or
`name: INFOTABLE:Acme.Shape_DS[:DIRECTION]`; with no direction a parameter is an input. A data entry
that loads with the page and also refreshes says `on: load, refresh: 5` (seconds); an expression's
widget takes `props` for what its own fields do not cover. A tab may carry `props` for its own
container. A bound property can read a widget's selected rows: `{bind: orders.SelectedRows.Name}`.

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
session. Reusable components expand into this same element tree; see Components below.


## Element reference

These nodes also accept the common ID, props, style, surface and size fields. IDs are
required, including on spacers, KPI tiles and header actions. Generated internal names use
`<id>--<role>`; choose other names for authored nodes. All output uses native widgets.
Examples below are nodes in a slot; `rows` is a declared INFOTABLE source with `Name` and
numeric `Count` fields. Bindings and events use authored IDs, including tile and action IDs.

| Element | Existing operation reused | Example |
| --- | --- | --- |
| `page_header` | heading/label/button authoring; v1 header row | `- page_header: {id: header, title: Alerts, color: primary, help: Current alerts, actions: [{id: refresh, label: Refresh}]}` |
| `toolbar` | row authoring and `recipe toolbar --band` | `- toolbar: {id: tools, children: [{text_field: {id: search, hint: Search}}, {spacer: {id: space}}, {button: {id: refresh, label: Refresh}}]}` |
| `kpi_row` | `recipe card`, label bindings and equal panel widths | `- kpi_row: {id: totals, tiles: [{id: total, label: Total, value: {bind: rows.Count}, unit: alerts, color: '#123456'}]}` |
| `trend_tile` | `recipe card` caption, value/unit/arrow labels in one centred row | `- trend_tile: {id: total, label: Total, value: {bind: rows.Count}, unit: '%', trend: up, delta: {bind: rows.Name}}` |
| `detail_pair` | row container, caption/spacer/value labels, optional status dot | `- detail_pair: {id: first, label: Name, value: {bind: rows.Name}, kind: ok}` |
| `card_grid` | rows and equal cells; `recipe card-grid` | `- card_grid: {id: gallery, size: fill, per_row: 3, children: [{card: {id: one, title: One, align: center}}]}` |
| `block` | `recipe card` with a background colour | `- block: {id: oee, color: pink, caption: OEE, value: {bind: rows.Count}}` |
| `card` | `recipe card`; with `align`, `recipe section` | `- card: {id: state, title: Status, value: {bind: rows.Count}, status: Healthy, kind: ok, children: [{label: {id: detail, text: Details}}]}` |
| `filter_panel` | filter-panel template and filter-footer authoring rules | `- filter_panel: {id: filters, fields: [{text_field: {id: query, hint: Search}}], footer: [apply, reset]}` |
| `tabs` | tabset authoring (`sync_tabs`), tab-content template, add-contained | `- tabs: {id: views, tabs: [{label: Overview, content: [{label: {id: overview, text: Overview}}]}, {label: Details, mashup: Acme.Child_MU}]}` |
| `chart` | `recipe chart` | `- chart: {id: counts, kind: bar, data: rows, x: Name, stacked: true}` or `kind: combo, series: [{field: Count, kind: bar}, {field: Total, kind: line}]` |
| `panel` | container with grey panel style; generated title label and toolbar band | `- panel: {id: box, title: Counts, toolbar: [{date_range: {id: dates}}], children: [{chart: {id: counts, kind: bar, data: rows, x: Name}}]}` |
| `slider` | `ptcsslider` | `- slider: {id: level, min: 0, max: 100, value: 35, labels: true}` |
| `kpi_dial` | `ptcskpidial` | `- kpi_dial: {id: dial, title: Level, kind: half, unit: '(%)', value: {bind: level.Value}}` |
| `spacer` | responsive container authoring and v1 spacer rule | `- spacer: {id: space}` |
| `date_range` | datepicker authoring, `DateRange: true` | `- date_range: {id: dates}` |
| `dropdown` | `recipe picker` | `- dropdown: {id: choice, options: rows, display: Name}` |
| `text_field` | textfield authoring; `variant`, `icon` | `- text_field: {id: query, hint: Search, variant: rounded}` |
| `pagination` | `recipe pagination` | `- pagination: {id: pages, for: results}` |
| `grid` | grid authoring and `recipe columns` | `- grid: {id: results, data: rows, columns: [Name, {field: Count, title: Total}]}` |
| `label` | label authoring | `- label: {id: note, text: Details, style: {color: ink}}` |
| `text` | label authoring with body LabelType | `- text: {id: lead, text: Four stations, one line.}` |
| `heading` | label authoring with header LabelType; `size`, `highlight` | `- heading: {id: title, text: Alerts, size: lg}` |
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
(`DisableLegendGrouping: true`) to keep series in one compact legend without axis headings.
Unknown axis values are located errors. Pagination names a grid with data
and creates the same local slicing expressions and reset bindings as the pagination recipe.

Trend tiles go in a `kpi_row` through `children` (a `kpi_row` takes `tiles` or `children`). A
`trend_tile` has a caption (`label`), a bound `value` and optional `unit`, `trend` (`up`, `down`
or `flat`) and `delta` (text or `{bind: ...}`). Value, unit, direction glyph and delta sit in one
centred row under the caption, as in the kit's AssetKPI tiles. The direction glyph and delta are
coloured labels (green, red, grey); these tiles retain their existing coloured-label implementation.
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

## Gallery, text and style elements

Batch 2b adds the elements of the inputs gallery (benchmark P) and of the layout-only version of
the web page benchmark (S). Each has a command equivalent built on the same authoring code.

| Spec | Result | Command equivalent |
| --- | --- | --- |
| `card_grid: {id, per_row, children}` | A column of equal rows, each holding `per_row` equal cells; the last row is padded with empty growing cells. Cells are `card`, `block`, `row`, `column`, `panel` or `trend_tile`. Add `size: fill` and the rows share the page's height | `mashup recipe card-grid <column> --per-row N --cells M`, then `recipe section` on each cell |
| `card: {title, align: center, children}` | A kit section: a framed column holding a fixed 40px title row (`sub-header` label) over a `LastContainer` body row. `align: center` centres the content on both axes; `start` (default for align) keeps it top left. A card without `align` is unchanged | `mashup recipe section <column> --title T [--align center]`, then `mashup add --in <body>` |
| `page_header: {color: primary}` | The title label's text colour | `mashup set <title label> --style label:color=primary` |
| `text_field: {variant: rounded}` (also `focus-background`, `focus-glow`, `focus-underline`) | Managed style parts of the text box and its `focus` state | `mashup recipe input-variant <textfield> --variant rounded` |
| `heading: {size: sm\|md\|lg\|display, highlight: lime}` | Label type `title`, `sub-header`, `header` (the heading default), `large-header`; `display` adds a 72px, weight 800 face by style; `highlight` is the background behind the label | `mashup add ptcslabel --label T --prop LabelType=header`, `mashup set --style background=lime` |
| `text: {text}` / `label: {text, style}` | `ptcslabel` of type `body` / `label` | `mashup add ptcslabel` |
| `block: {color, caption, value}` | A card with the colour as its background, a caption label and a value label that takes `{bind: ...}` | `mashup recipe card --title C --say V`, then `mashup set --style background=pink` |
| `style: {background, color, border, font}` | On `label`, `heading`, `text`, `block` and `chart` (a chart takes `background` and `border`): managed style entries. `color` and `font` style the text, `border` is a 2px solid line | `mashup set --style background=ink --style label:color=lime ...` |

A `style` list of `--style` settings still works; the map is the short form for these four keys.
Colour names are `primary` (the kits' title blue), `ink`, `lime`, `pink`, `purple`, `orange`
and `mint`, or a `#hex` value; `font` is `mono`, `sans` or a family list. The same names work in
`mashup set --style` (`background=lime`, `label:color=primary`, `label:font-family=mono`). They are
fixed hex values for now; the theme tokens of batch 4 replace them.

Kit structures used (the inputs gallery): the page column (`default`, grow 1, `LastContainer`), a
fixed 40px title row, two gallery rows of `default` growing columns, and a section column holding
a fixed 40px title row (row, align-items center) over a `LastContainer` row (justify and align
center) that holds the field. Equal cards come from equal growing siblings, not fixed sizes, and
the centring from the body row.

Differences from the kit, all in the cells: `flex-basis` is `0px` instead of `auto` so widths and
heights stay equal whatever the cards hold, and section columns stretch their rows instead of using
`align-items: flex-start`. The kit draws the card frame and its 20px gap with the master class
`wdk_Section`, which is not in the mashup; the section carries a 1px #BEC6CA managed border and a
10px margin instead. The kit's rounded input is the text box part's `border-radius: 20px`
(exported StyleProperties), so `rounded` is kit-exact. The other three kit inputs are the master
classes `wdk_Input2` to `wdk_Input4`, whose CSS is not exported; `focus-background`, `focus-glow` and
`focus-underline` use the text box's `focus` state (`background`, `border-color` with `box-shadow`,
`border-width: 0 0 2px 0`) and are an approximation that has not been compared with the kit at run
time. Invented where no kit exists: the 72px `display` heading, the `highlight` fill, the colour
names and the block, which has no kit equivalent; its card recipe is the existing one. S has no
kit, so its rows reuse the structures above and the existing card and toolbar rules. A `theme` for S,
token names in `style`, the underline of the active navigation label, hard shadows and chart bar
colours are batch 4; the layout is verified offline only.

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
spec and mashup first; see [Adopting an existing mashup](#adopting-an-existing-mashup) for how
a derived spec does this. If XML and sidecars disagree, explicitly run `twaco sync` to keep the
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
| O / kit gauges | [Gauges and KPI](../tests/fixtures/mashup/pages/bench_o/page.yaml) | The kit's four needle gauges use a legacy `gauge` type with no authoring contract and no native widget; the native `ptcskpidial` (dial and half layouts) stands in, a recorded native deviation. Dials bind to one slider; the kit's absolute-positioned slider becomes a full-width slider with min/max labels. |
| L / Compare | [Compare](../tests/fixtures/mashup/pages/bench_l/page.yaml) | The kit's side lists are collections over cell mashups; the spec uses native lists (selected items feed the second list) with an eye-icon notice for the empty state. The side panel's one tab and 276px column follow the kit; the cell page is a generated KPI tile (title over a dial) instead of the kit's detail card. Not yet measured live. |
| M / Administrator | [Admin shell](../tests/fixtures/mashup/pages/bench_m/page.yaml) | The navigation is native buttons in a column (the first marked selected), not the kit's service-fed collection with section headers; the filter column's chevron is the kit's inside tab. Filters are not wired to the service, and Apply/Reset have no events in this spec. Not yet measured live. |
| P / kit inputs gallery | [Inputs gallery](../tests/fixtures/mashup/pages/bench_p/page.yaml) | The content area only, without the kit's shell. Equal cards come from growing `default` rows and columns and the centring from the body row, as in the export; `flex-basis` is 0 and the frame and gap are managed style and margin. `rounded` is kit-exact; the three focus looks are native approximations of the kit's master CSS classes. Not yet measured live. |
| S / web page | [Line 03 live](../tests/fixtures/mashup/pages/bench_s/page.yaml) | Layout only: no kit exists for it, so rows reuse the kit structures, and colours are plain names. Theme tokens, the active-navigation underline, hard shadows and chart bar colours are batch 4. Not yet measured live. |
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

V1 filter columns follow the kit's admin pages: a fixed-width collapsible row contains a `default`,
growing `LastContainer` column.
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
| Filters (K, M, Q1) | Admin pages: direct controls in the fields column; the footer column wraps a divider and the button row | Fixed per-field rows, estimated fields minimum, sibling divider/footer and trailing filler | Direct controls and nested footer, with the kit margins above |
| Side panel (L) | Compare: 72px title band; a growing row holds the tab set; a growing tab column; fixed content bands and a final list row | 72px title plus tab row sized by placement; generic remainder filler | Explicit growing `default` tab row and `LastContainer` tab content; existing list/empty-state branches retained |
| Navigation (M) | Administrator: 72px title; 100px search-tab band; collection in a growing row | Native static caption/button rows and trailing filler | Static captions/buttons directly in one column; no filler. The service-fed collection/cell menu has no static-menu equivalent in the exports; the native button adaptation retains stable event addresses and font styling |
| Generated collection cell (L) | CompareCell: fixed 24px top band, 160px image band, growing details column, fixed 18px status band | KPI-tile template with title/dial, plus generic filler where no child grew | The title/dial-only adaptation retains the KPI template; filling containers use `LastContainer`. No image/status bands are fabricated: the cell API exposes neither role, and no exported kit has a title/dial-only cell |
| Main content | Admin pages and Compare: fixed header/toolbar/pagination bands around a `default` growing grid/collection row | Range rows with calculated floors and no `LastContainer` | Kit grid/collection remainder rows use `default` + `LastContainer` and keep any authored or chart height floor so tall pages scroll; the horizontal-width correction remains |

Other benchmark output changes are limited to kit remainder containers and removal of the
fillers: A/O gain the page-column remainder marker; H/N gain the grid-row marker and
`default` sizing; J gains tab/body remainder markers; Q2-Q4 gain markers on their fill regions.
All also gain the page-column marker. Their controls, event wiring, chart floors and width
corrections are retained. These markers follow the growing page/content columns in
the admin pages and Compare, and the tab content in Administrator/Compare.

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
content. A chart accepts `legend: false` to reclaim plot space in a dense chart row; v1 legends otherwise sit at the top, aligned right. A single numeric-series chart hides its
legend by default; pie charts retain it. Explicit `legend: true|false` and native props win.
Pie charts show value markers and use ordered native slice/legend palette styles.
Short bar charts keep values inside the bars, aligned to the end. No custom chart HTML or rendering is used.

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
Q3 keeps the pie legend and hides its other chart legends; Q4 follows the selected asset and aligns mixed status pairs.
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


## Native controls, media and wiring

All examples below are synthetic. Native controls and media use the same catalogue authoring
planners as `mashup add`, `mashup bind` and `mashup set`; they accept `props`, `style` maps/settings
and layout sizes. Authored properties override element defaults. New defaults are recorded in the
v1 rule table; legacy rules are unchanged.

| Element | Fields | Command path |
| --- | --- | --- |
| `link` | `text`/`label` (string or binding), `url` (string or binding) or `mashup`, `target` (`_self`, `_blank`, `_popup`), `icon` | `mashup add ... ptcslink --prop Text=... --prop LinkURL=... --prop TargetWindow=...`; an icon adds a separate `ptcsicon` beside the link |
| `image` | `source` (Media entity name, URL, or binding), `scaling` (`fit-x`, `fit-y`, `fit`, `contain`, `cover`, `auto`, `25%`, `50%`, `75%`), `position` (`top`, `center`, `bottom`), `alt` | `mashup add ... ptcsimage --prop SourceURL=/Thingworx/MediaEntities/Acme.Logo_MD --prop Scaling=contain --prop AlternateText=Logo` |
| `icon` | `icon` (CDS name or shorthand), `source`, `alt`, `icon_size` (`small`, `medium`, `large`, `xlarge`, `custom`), `size: sm/md/lg/xl`, token `color` | `mashup add ... ptcsicon --prop SVGIcon=cds:icon_info --prop Size=large`; `mashup set ... --style color=ink` |
| `divider` | `orientation` (`horizontal` or `vertical`), token `color` | `mashup add ... ptcsdivider --prop VerticalDivider=true`; `mashup set ... --style line:background=line` |
| `checkbox`, `toggle` | `label`/`text`, bindable boolean `value` | `mashup add ... ptcscheckbox` / `ptcstogglebutton --prop Label=Enabled --prop State=true` |
| `radio` | `label`/`text`, bindable boolean `value`, bindable `group`; or static `options: [Open, Closed]` | `mashup add ... ptcsradio --prop Label=Open --prop Radiogroup=AcmeStatus`; bind `State` |
| `textarea` | `label`/`text`, `hint`, bindable `value`, `max_length` | `mashup add ... ptcstextarea --prop Label=Notes --prop HintText=Enter --prop MaxNumberOfCharacters=500`; bind `Text` |
| `date_picker` | single date: `label`/`text`, `hint`, bindable `value`, `date_only`, `format` | `mashup add ... ptcsdatepicker --prop DateRange=false --prop DateOnly=true --prop FormatToken=YYYY-MM-DD`; bind `DateTime` |
| `value_display` | `label`/`text`, scalar or bindable `value` | `mashup add ... ptcsvaluedisplay --prop Label=Count`; bind `Data`. A literal uses an `expression2` constant, a Loaded-to-Evaluate event and a binding because native `Data` is not editable |
| `events_router` | `data_type` (default STRING), `input_count` (1 to 20), `routes` (bindings or null for an unbound input) | `mashup add ... eventsrouter --prop DataType=BOOLEAN --prop NumberOfInputs=2`; bind `Input1`, `Input2`, read `Output` |
| `embed` | static `mashup`, `params: {name: literal or {bind: ...}}` | `mashup add-contained ... Acme.Child_MU --set id=...`; bind the container's child parameters |

A radio is a native boolean control, as in the kits. Static options expand to radios with
IDs `<id>--1`, `<id>--2`, etc., sharing `Radiogroup`; read each radio's `State`. A data-bound
radio binds `value` and optionally `group`. A dynamic list of options needs a collection cell;
`ptcsradio` has no native options-table contract. A link icon is an adjacent native icon;
`ptcslink` has no native icon property. Media entity names become `/Thingworx/MediaEntities/<name>`;
URLs remain unchanged. Divider colour styles its native `line` part's background.
An embed uses `add-contained`'s child-parameter contract. Its current offline catalogue has no
independent editable styling contract: style/size the surrounding row or column, or the child
mashup's own controls, rather than setting styles on `mashupcontainer` itself. Function elements
have no visual styling or layout.

```yaml
# Inside page.slots.center:
- link: {id: details, text: Details, mashup: Acme.Detail_MU, target: _blank}
- image: {id: logo, source: Acme.Logo_MD, scaling: contain, alt: Acme logo}
- icon: {id: help, icon: info, size: lg, color: ink}
- divider: {id: separator, orientation: horizontal, color: line}
- checkbox: {id: archived, label: Include archived, value: false}
- radio: {id: state, options: [Open, Closed], group: AcmeStatus}
- toggle: {id: enabled, label: Enabled, value: true}
- textarea: {id: notes, label: Notes, hint: Enter notes, max_length: 500}
- date_picker: {id: when, label: As of, date_only: true}
- value_display: {id: count, label: Count, value: {bind: rows.Count}}
- embed: {id: detail, mashup: Acme.Detail_MU, params: {id: {bind: rows.Name}}}
```

`chart` also accepts `kind: pareto` and `kind: waterfall` with the same flat `data` and `x`
contract as line/bar. Pie uses its native `DataField` for labels and `ValueField` for numbers;
derivation recognises that pair. Combo still requires explicit numeric `series`. The command
path is `mashup add` with the chart's native type followed by `recipe chart --data ... --x ...`
(and `--value` for pie, `--series` for combo). `kind: schedule` is refused with a nested-interval
contract explanation: use `widget: {type: ptcschartschedule}` and explicit native bindings.

Native service pagination uses `pagination: {id: pages, mode: server, total: {bind: rows.Count},
page_size: 25, page_number: 1}`. Bind `pages.PageNumber` and `pages.PageSize` into service params,
and use `PageNumberChanged` / `PageSizeChanged` events to invoke that service. This adds no local
slicing functions. The command path is `mashup add ... ptcspagination`, properties and bindings.
`pagination: {for: grid}` keeps its existing local slicing behaviour. `dropdown: {id: empty,
unbound: true}` represents an unconfigured kit control without inventing a service. Grid
controls without a supported table feed retain their native properties and explicit bindings.

A collection with `data` retains the existing configured-cell workflow. To preserve native kit
layout and a table-property binding, use `collection: {id: cards, source: {bind: rows}, mashup:
Acme.Cell_MU, map: {Name: title}}`; the same properties and binding are available through
`mashup add ... ptcscollection`. Derivation uses this native form and retains layout properties. An unconfigured native
collection explicitly uses `unbound: true` instead of a source; omitting both data and source
without that marker remains an error.
Use the `data` workflow when row/child mapping validation is required. The native source form
preserves direct mapping properties and native binding checks; it cannot validate an unavailable
row shape. Tabs read labels from native `Items` when present, falling back to `Tab<n>Name`. KPI dials
recognise dial/horizontal/vertical layouts and retain custom angles and formatting in `props`.
Unsupported entity-service feeds are reported in notes; recognising a control does not claim
that an unsupported feed was preserved.

An expression is a data item. Short strings are expressions; scripts containing a semicolon or
newline are carried verbatim. Inputs can infer their types from a binding reference or declare
`{type: NUMBER, value: {bind: rows.Count}}`; a typed input can also have a literal value.
`output` is a ThingWorx base type. Optional `auto_evaluate` (true), `fire_on_first_value` (false)
and `data_change: ALWAYS|VALUE` preserve native evaluation choices. `on: load` or a node event
invokes `Evaluate`. Bind to `late` or `late.Output`; events may invoke `late.Evaluate`.

```yaml
# Inside page:
data:
  late: {expression: "Count > 3", inputs: {Count: rows.Count}, output: BOOLEAN}
events:
  - {on: open_btn.Clicked, navigate: {mashup: Acme.Detail_MU, mode: popup, params: {id: {bind: rows.Name}}}}
```

Navigation `mode` is `page` (default), `new_tab`, `popup` or `modal-popup`; native aliases
`replace` and `new` are also accepted. Optional `width`, `height`, `title` and `params` share
`recipe popup`'s child-parameter checks. Parameter values are literals or `{bind: ...}`. A plain dotted reference to a known node/data alias is also accepted; `grid.SelectedRow.Name` reads `grid.selected.Name` through the selected-row helper.
An optional `id` names the generated function for subsequent event references; `props` retains
native popup options during derivation. Command equivalents are `mashup add ... expression2
--input Count:NUMBER --prop Expression="result = Count > 3;" --prop OutputBaseType=BOOLEAN`,
`mashup bind`, and `mashup recipe popup ... --pass id=<source>` (or `mashup set` for fixed function
inputs). Both CLI and MCP use these core planners; MCP page builds still plan by default.

`events_router` represents the kits' simple typed fan-in: each `routes` entry binds one native
`InputN` and consumers bind `<id>.Output`. An omitted count defaults to the number of routes
(at least two). For example, `events_router: {id: route, data_type: BOOLEAN, routes:
[{bind: first.State}, {bind: second.State}]}`. It is a function beside the layout, with no
visual container. The local exports use two/three STRING or BOOLEAN inputs and no special events.
Other native properties and events remain available through `props` and explicit event wiring.

Static navigation with one Navigate trigger derives as an event action. Dynamic targets and
shared navigation functions remain raw. `dynamic_panel`
stays raw because its two native panes and push/trigger behaviours differ from `side_panel`.
`menubar` stays raw because primary/secondary Menu entities, infotable navigation and page-host
selection do not map to `nav_menu`'s button groups. Decorative `shape` and map extensions
(`googlemap`, `OpenStreetMap`) intentionally remain raw. Raw-type notes name these limitations.
All kit observations in this batch are from local exports; no live behaviour is claimed.

## Components

Components are embedded YAML definitions plus `twaco/components/*.yaml` in the solution.
A solution definition with the same `name` overrides the built-in. Other names add components.
`twaco components list` lists names, kit sources and props; `twaco components show <name>`
shows the entire definition. Both accept `--json`. MCP `components` lists definitions; its
optional `name` shows one. No server is contacted.

Use the explicit `component` node so element names and component names remain distinct:

```yaml
- component:
    use: section_card
    id: inputs
    title: Standard input
    align: center
    children:
      - text_field: {id: query, hint: Search}
```

The command equivalent adds the same expanded element tree under an existing container:

```sh
twaco mashup recipe component Acme.Page_MU section_card center --set id=inputs --set title="Standard input" --set align=center --apply
```

`--set` is repeatable. Values are JSON when parseable, otherwise strings. Arrays, objects
and `{bind: ...}` are passed as JSON; bindings name existing widget IDs or existing service
aliases (`rows.Count`); `param.<name>` reads an existing mashup parameter. Generated cells
inherit the parent's StyleTheme. MCP uses `mashup_recipe` with `recipe: "component"`, `kind`,
`widget` (the parent container), and `settings: ["id=inputs", "title=Standard input"]`.
Command recipes plan by default; MCP applies only with `dry_run: false`. Components
with generated collection cells include those cells' XML, sidecars, specs and baselines in
the same transaction as the parent edit. Compatible existing cell specs retain their bytes
and use their baseline merge; a different existing cell spec or a merge conflict writes nothing. The recipe report includes every generated file.

| Built-in | Source pattern | Props |
| --- | --- | --- |
| `kpi_tile` | `AssetKPI`, caption/value row | label, value (required binding), unit |
| `trend_tile` | `AssetKPI`, caption/value/trend row | label, value (required binding), unit, trend, delta, color, sparkline |
| `status_card` | `twaco` (existing card recipe) | title, value, status, state, detail, kind, children |
| `filter_panel` | `AdminPage` | title, fields, footer, children |
| `toolbar_band` | `AdminMetrics` | children |
| `page_header` | `AdminPage` | title (required), help, info, actions |
| `section_card` | `Inputs` | title (required), align, children |
| `side_panel` | `Compare` | title (required), help, tab, children |

The `source` field records provenance; `twaco` denotes an invented/adapted combination.
These definitions reuse the already documented native-widget adaptations. Existing element
syntax and output are unchanged. Inspection of the supplied exports is offline evidence;
this batch makes no runtime comparison claim.

A solution definition has this same schema:

```yaml
name: metric
source: twaco
props:
  caption: {type: string, default: Total}
  data: {type: string, required: true}
  value: {type: value, required: true}
contract:
  data: data
  fields: {Count: NUMBER}
tree:
  card:
    id: '$id'
    title: '$caption'
    value: '$value'
```

`props` types are `string`, `number`, `boolean`, `array`, `object`, or `value` (any YAML
value, including bindings). A missing optional prop expands to null. `$prop` substitutes
its typed value, and `${id}--role` interpolates an ID in a string. A prop array used as
an array item is spliced into that array, so a component can append caller children. `id` and `use` are
reserved. The tree has one root node and can use other components. Give internal nodes
IDs based on `${id}`; caller-supplied children retain their IDs. Cycles and more than 32
component levels are refused. Unknown props, missing required props and contract mismatches
name the component and point to the invocation. A contract names the prop that supplies a
data alias and required result fields with their ThingWorx base types (`ANY` accepts any
known field type). Both spec builds and command recipes check the same offline service
contracts. Build application also refuses component files added, removed or changed after
planning.

## Deriving a spec

```sh
twaco mashup spec Acme.Page_MU --out adopted.page.yaml
twaco mashup spec Acme.Page_MU --json
```

Without `--out`, the spec goes to stdout and coverage to stderr. `--json` returns `spec`,
`elements`, `raw`, `raw_types` (always present, including `{}`), `widgets`, `exact`, `hand_edited` and `notes` (the last three only
when they have something to say); it also writes the requested output file when present. MCP
`mashup_spec` takes `mashup` and returns the same report without writing a file.

**A mashup built from a spec** (`src/<Mashup>/page.yaml` with its `.page-baseline.json`) returns
that `page.yaml` byte for byte (`exact: true`) while the mashup still matches its baseline. If the
mashup was edited by hand after the last build, the report still carries `page.yaml`, with
`exact: false`, `hand_edited` naming each changed widget as `node (widget id)` (plus `Data (wiring)`,
`DataBindings (wiring)` or `Events (wiring)` when the wiring changed) and a note that the file does
not describe those edits. A page with a `page.yaml` but no baseline is compared by compiling the
spec instead. The offline test covers every Acme benchmark spec (A, H, J, K, L, M, N, O, P,
Q1 to Q4 and S) both ways.

**Any other mashup** gets a YAML spec of elements, not a copy of its content:

| Widgets | Written as |
| --- | --- |
| flex container | `row` or `column`; `size: fit` for no growth, `size: {fixed: N}` for a fixed pixel size along the parent, `weight: N` for a growth other than 1; other properties that differ from the catalogue go in `props` |
| empty container in a row | `spacer` |
| label | `heading` (header), `text` (body), `label`; other label types are a `label` with `props: {LabelType: ...}` |
| button, text field, date range | `button` (`variant`, `icon`), `text_field` (`hint`), `date_range` or single `date_picker` |
| dropdown, list | `dropdown` (`options`, `display`), `list` (`data`, `display`, `selection`), when a service feeds them |
| grid | `grid` with `data` and `columns` (field, title, width) when a service feeds it |
| charts | `chart` with `kind`, `data`, `x`; pie uses `value`, combo lists `series`; schedule stays raw |
| collection | `collection` with native `source` binding, `mashup`, `map` and retained layout props |
| tab set | `tabs` with each tab's content |
| row with a header label and buttons | `page_header` with `actions` |
| 48px row of controls | `toolbar` |
| filter column (title, controls, divider footer with one or two buttons) | `filter_panel`; the buttons become `apply` and `reset` |
| side column (72px title row over a one-tab strip) | `side_panel` |
| column of a 40px title row (one sub-header label) over a body row | `card` with `align` |
| row of three-label columns (caption, bound value, unit) | `kpi_row` with `tiles` |
| native controls/media | `link`, `image`, `icon`, `divider`, `checkbox`, `radio`, `toggle`, `textarea`, `date_picker`, `value_display`, `embed` |
| functions/pagination | typed expression `data` entries, `events_router`, `navigate` actions, server-mode `pagination` |
| anything else | `widget: {id, type, props}` with only the properties that differ from the widget catalogue |

Services become `data` entries (`service`, `on: load`, `on: every Ns`, fixed `params`). A binding
from a widget property to a service parameter becomes `params: {name: {bind: node.Property}}`,
one into a widget property a `props` entry with `{bind: ...}`, a service feeding `Data` the
element's `data`, and every other trigger an `events` entry. Node ids are the widget ids with
anything but letters, digits, `_` and `-` replaced.

Not carried: widget styles, custom CSS, state formatting snapshots, bindings and events outside the supported function contracts and data that is not a Thing service. Properties the catalogue does not list are carried as props on a page that is derived to be adopted, and listed in `notes` otherwise. The report's `notes` list what applied.
Pagination derives as `pagination` with `mode: server`, preserving its existing wiring without
adding slicing expressions. Native expressions become typed `data` entries. Widgets without an element (decorative shapes, extension maps, dynamic navigation or embed targets, two-pane dynamic panels, native menu bars) stay as raw `widget:` nodes with a note.
Build the derived spec to review a plan: its widget tree matches the
original's widgets, nesting, services, bindings and events (the compiler adds layout wrappers
and the look), not its properties.

Coverage counts spec nodes, not widgets: `elements` is the nodes written as elements or
components, `raw` the `widget:` nodes with their types in `raw_types`, and `widgets` the widgets
of the mashup, so `widgets` against `elements + raw` shows how much the spec condenses.

Historical coverage before native controls and expression data items, for the synthetic kit-structure fixtures
(`tests/fixtures/mashup/pages/kit_structures`, hand-written Acme pages with the container
structure of the kit pages, no copied content):

| Fixture | Widgets | Elements | Raw | Spec bytes |
| --- | ---: | ---: | --- | ---: |
| filter_column | 23 | 13 | 1 (pagination) | 2018 |
| side_column | 16 | 9 | 0 | 1327 |
| section_gallery | 25 | 11 | 1 (date picker) | 1357 |
| admin_shell | 24 | 16 | 0 | 1818 |
| tabs_page | 10 | 7 | 1 (expression2) | 879 |

Historical derivation coverage before this batch, for benchmark pages built by their specs and derived again from their built content (as a page
without a baseline would be). All fourteen derive specs that parse; the six smallest (up to 40
widgets: H, J, K, L, N, P) are built again and keep their widgets, nesting, services, bindings and
events. The raw nodes are mostly the pagination widget with the expressions that slice its grid,
which the `pagination` element would duplicate.

| Page | Widgets | Elements | Raw (by type) | Spec bytes |
| --- | ---: | ---: | --- | ---: |
| A | 127 | 112 | 14 (shape 12, chart pie 1, date picker 1) | 37264 |
| H | 22 | 15 | 6 (expression2 4, grid 1, pagination 1) | 17360 |
| J | 21 | 18 | 0 | 4716 |
| K | 39 | 26 | 6 (expression2 3, date picker 1, grid 1, pagination 1) | 26862 |
| L | 36 | 26 | 5 (expression2 4, list 1) | 9393 |
| M | 48 | 34 | 6 (expression2 4, grid 1, pagination 1) | 25376 |
| N | 7 | 6 | 1 (expression2 1) | 1559 |
| O | 47 | 37 | 9 (kpi dial 8, slider 1) | 12403 |
| P | 36 | 35 | 0 | 7138 |
| Q1 | 66 | 52 | 5 (expression2 3, grid 1, pagination 1) | 35244 |
| Q2 | 86 | 85 | 0 | 20593 |
| Q3 | 57 | 49 | 7 (expression2 4, chart pie 1, grid 1, pagination 1) | 24571 |
| Q4 | 50 | 45 | 4 (expression2 4) | 13595 |
| S | 85 | 84 | 0 | 18513 |

## Adopting an existing mashup

`mashup spec` on a mashup that no spec built, then `mashup build` on the untouched `page.yaml`,
adopts it without changing a byte of it: the plan has no conflicts, no widget is renamed and none
moves. Four things make that so.

- **The page root is the template's root.** The mashup's root container maps onto the template's
  (`page.root.props`), and its children are the `center` slot, so the build adds no level. Other
  widgets beside the root container (functions, status messages) follow in the slot.
- **`layout: as-built`** on a page whose mashup carries no v1 marker (`twacoPageRules`) keeps the
  v1 layout rules off it: widgets go into their containers as the spec lists them, with no wrapper
  rows and no fix-up pass, and nothing is added that the mashup lacks. Derived nodes are rows,
  columns, leaf elements and `widget:` nodes; composites such as `card` or `filter_panel` are not
  guessed at. Properties the spec states always win over a rule; `props: {Name: null}` removes a
  property the mashup does not have.
- **Properties are carried.** A derived node lists what the mashup has that building it would not
  produce by itself: `CustomClass`, `Margin` (the root's `""` stays `""`), `flex-wrap`,
  `flex-size`, `flex-min-*`/`flex-max-*`, `align-items`, `ShowVerticalScrollbar`, `DisplayName`,
  and properties the catalogue does not list (`RowHeight`, `MinValue`/`MaxValue`, `TabNSVGIcon`,
  `ToolbarMaxRows`, `Scrolling`) as props. The tool builds the spec offline and adds a prop for
  every difference it finds, then builds again. A number the platform accepts for a pixel option
  (`DefaultFontSize: 14`) is read as the option `"14px"` and kept as the number. A contained
  mashup is `embed`. `page.master` and `page.use_master_theme` keep the mashup's master. A master
  (`mashupType: targetmashup`) is refused by `mashup spec` and `mashup build`: that is a later
  batch.
- **Ids are kept.** At the first build the generated widgets are lined up with the mashup's by
  type and position, and the baseline records which `(node, role)` keeps which id
  (`pinned_widgets`), plus the ids of data sections, service calls, bindings and events
  (`pinned_wiring`). Every later build reapplies them, so `custom.css` selectors and external
  references stay valid. Nodes added later get stable hashed ids as usual. Bindings and events
  differ in no way a build would notice, so their order is not compared, and neither are
  `id_index`, twaco's own markers or the static value of a property a binding writes.

When a first build still conflicts, each difference is listed (also in `--json`) with its JSON
path, the mashup's value, the spec's value and the spec line responsible, the property's own line
when the node states it:

```text
Acme.Hand_MU: /UI/Widgets/flexcontainer-3/Properties/CustomClass: the mashup has "acme-title",
the spec generates "other-title"; change src/Acme.Hand_MU/page.yaml:14 (props.CustomClass) or
resolve the hand edit
```

`mashup spec` lists in `notes` each binding or event it could not write (`source -> target`) and
each property it left out (`widget property`). Compile errors from `mashup build` and
`mashup new --spec` point at the offending key's line and name the spec file that was read;
with `--json` they are `{"error": {"file", "line", "column", "message"}}`. A plan says
`files_state: "would write"`; the same report after `--apply` says `written`.

## Tokens and themes

Design tokens resolve locally. `twaco mashup tokens` lists `convergence` (default), `clean`
and `brutalist`; `twaco mashup tokens brutalist --json` shows resolved names and native theme
props. Presets contain twaco's own values. Convergence retains the existing colour names and
`mono`/`sans` values.

A spec's top-level `tokens: brutalist` or `tokens: ./tokens.yaml` overrides `[mashup] tokens`
in twaco.toml, then convergence. Spec paths are relative to the spec; config paths are relative
to twaco.toml. Command styling uses the config setting or convergence.

```yaml
version: 1
name: Acme
extends: brutalist
colors:
  accent: '#52A4E8'
roles:
  primary: accent
fonts:
  sans: Arial, sans-serif
radius:
  default: none
shadow:
  hard: 4px 4px 0 ink
vars:
  e-chart-tick-label-color: '${colors.paper}'
```

Token files are strict version 1 YAML. Sections are `colors`, `roles`, `fonts`, `radius`,
`border`, `shadow`, `space`, `chart` (a colour list), `vars`, and optional `block` defaults
(`border`, `shadow`, `radius`). Colour scales such as `primary-50` and `grey-900` are optional.
Roles (`primary`, `success`, `warning`, `danger`, `info`, `background`, `surface`,
`surface-alt`, `text`, `text-muted`, `heading`, `line`, `on-primary`, `on-dark`, `selected`,
`card`, `page-card`, `band`, `card-line`, `panel-line`, `nav-selected`, `nav-text`,
`nav-caption`, `input-focus`) name
colours or hex values; `border` takes `width` and `color` for the `border:` style shorthand and
`line` for the native line thickness of inputs and grids. `border.card` controls recipe
card/section/panel frames (default 1px); `border.band` controls the toolbar bottom border
(default 0px). Brutalist references `border.width` for both and `border.color` for frame colours.
`card` preserves the recipe white fill, `page-card` the `surface: true` fill, and `band` the
kit grey fill under convergence; brutalist uses paper for all three. Status/trend colours
remain fixed kit semantics. Qualified references use `${colors.ink}`, `${roles.text}`,
`${fonts.mono}`, etc. Unknown references and circular inheritance are refused with known names.
Inheritance accepts presets or another file relative to the extending file.

```sh
twaco new theme Acme.Page_TH --tokens brutalist
twaco new theme Acme.Page_TH --from Acme.Base_TH --tokens ./tokens.yaml --prop global-color-core-primary=#CCFF00 --apply
```

Themes overlay base props, token props, then `--prop`. The mapping sets native ptcs globals
(colours, text colours, fonts, radii), the primary button text and chart text and series; other
widget variables follow those globals through the platform's theme engine. The mapping leaves
native flexcontainer variables untouched; lines keep their native thickness unless
`border.line` or `vars` sets it.
A token set to `inherit` writes no variable, so the platform value stays. `vars` overrides
the mapping. Variable names must use lowercase hyphens:
`backgroundColor` is refused with a `global-color-bg-primary` hint. Well-formed names outside
twaco's mapping are allowed and reported in the plan.

`page.theme` names an existing StyleTheme. Tokens resolve style values and do not create a theme.
With explicit tokens and the default theme, the build plan prints a `new theme --tokens` reminder.
Generate and select the theme to restyle widgets natively, including chart axis and legend text.
Custom CSS is separate from tokens; use native theme variables for ptcs styling.

Colour properties take colour or role names, `font-family` takes font names, `border-radius`
takes radius names, and `box-shadow` takes shadow names. For example:

```yaml
tokens: brutalist
# inside page.slots:
# - block: {id: oee, color: pink, caption: OEE, value: 84, size: xl,
#           style: {border: ink, shadow: hard, radius: none}}
```

Style maps accept `background`, `color`, `border`, `font`, `shadow`, `radius`. A border colour
uses the token border width and a solid line. Brutalist blocks default to ink borders and hard
shadows; `size: xl` sets the value to 48px and gives the block an uncapped height range
with a minimum derived from its value size. A label with `band: true` sits in a full-width
row; its background and border style the row while the text label grows inside it.
Derived specs retain explicit `tokens:` and keep derived hex colours as hex.

Command parity: set `[mashup] tokens = "brutalist"` in twaco.toml, then use
`twaco mashup set Acme.Page_MU <widget> --style box-shadow=hard --apply`. MCP provides
`mashup_tokens` and `entity_new` with `kind: "theme", tokens: "brutalist"`.

## Side columns, navigation and generated cell roles

V1 side panels and leading filter columns reach the page edges; their content sibling owns
its 24px gutter. Filter panels always have a 48px title row (default `Filter`), with the
inside collapse tab reserved at its right. Fields start below that row. A standalone filter
keeps the same grey background and separate 1px vertical rule. A leading authored heading
is placed in the header. Trend figures keep value, unit and arrow together, left aligned
immediately below the caption. Filled grids omit the outer bottom frame, as the kit does,
so row rules do not appear detached from the last row. Band labels centre text vertically.

`nav_menu` accepts `selected: Details`, naming exactly one item label; absent selection
highlights the first item. An item can be a string or `{label: Details, indent: 1}`;
indent is 0?12 levels of 18px beyond the base 24px inset. Item event IDs retain their
1-based group/item numbering. Selection is the initial highlight, not navigation wiring.

Generated collections accept `cell: {image: Image, title: Name, kpi_dial: Count,
status: {field: Status, state: Acme.Status_ST}}`. All roles are optional, at least one is
required. Image fields must be STRING or IMAGELINK; dial fields must be numeric. Status
uses the existing checked StateDefinition dot and label recipe. Image/status cells use a
column with image, title, dial and status in that order; title/dial-only cells retain the
KPI template. Bindings use typed cell parameters, with no generated service scripts.

Command recipes share these expansions:

```sh
twaco mashup recipe component Acme.Page_MU center --component nav_menu --set id=menu --set selected=Details --set 'groups=[{"title":"Acme","items":["Overview",{"label":"Details","indent":1}]}]'
twaco mashup recipe component Acme.Page_MU center --component collection --set id=cards --set data=assets --set 'cell={"image":"Image","title":"Name","status":{"field":"Status","state":"Acme.Status_ST"}}'
```

These commands plan by default. `assets` is an existing unique service alias in the mashup.
The generated cell files join the recipe's atomic transaction. Custom components can compose
these same elements. No CLI flags or MCP request fields change.

`tokens: clean` gives cards white fill, a 1px grey-200 border, 8px radius and soft shadow,
with a grey-50 page and the Inter font stack. Optional `radius.card` and `shadow.card`
override card/section finishes; convergence retains its existing frames. The synthetic
[clean A page](../tests/fixtures/mashup/pages/bench_clean/page.yaml) records the exact native
styles offline. Widget internals still use the selected StyleTheme.
