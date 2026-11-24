# Page specs: the YAML wireframe tool

## Page spec, spec, wireframe, YAML, layout and page.yaml

Start a new mashup with a page spec: a YAML wireframe compiled offline into a responsive
ThingWorx 10.2 mashup using the kit layout rules. A spec names its template, slots,
widgets, service data and events. Stable node IDs preserve bindings across rebuilds.

Save a spec like this as `orders.page.yaml` (the service must exist in the solution):

```yaml
version: 1
rules: v1
page:
  name: Acme.Orders_MU
  template: blank
  data:
    orders: {service: Acme.Manager.GetOrders, on: load}
  slots:
    center:
      - toolbar: {id: tools, children: [{heading: {id: title, text: Orders}}]}
      - row:
          id: kpis
          children: [{block: {id: count, caption: Orders, value: 84, color: lime}}]
      - grid: {id: orders_grid, data: orders, columns: [{field: Name, title: Name}]}
```

`twaco new mashup Acme.Orders_MU --spec orders.page.yaml` plans it; add `--apply` to
write. Then edit `src/Acme.Orders_MU/page.yaml` and run
`twaco mashup build Acme.Orders_MU --apply`. Commit page.yaml and .page-baseline.json
with the XML and sidecars. Independent hand edits survive; conflicts write nothing.

Before restructuring an existing mashup, derive its page spec with
`twaco mashup spec Acme.Orders_MU --out derived.page.yaml`; review reported coverage.
Written to `src/Acme.Orders_MU/page.yaml`, that spec adopts the mashup: `mashup build` then plans
no change (no extra container level, widget and wiring ids kept in the baseline, layout kept
as built with `layout: as-built`, master kept with `master:`). A conflict lists each JSON path with
both values and the spec line. Masters use `kind: master` and a `page_host`.
Use `mashup set` or recipes for small edits. `twaco mashup why Acme.Orders_MU tools`
explains the layout decisions. The full element reference is documentation/PAGE_SPECS.md.

## Master, target mashup, page host, page_host and menu_bar

Set `page.kind: master` for a target mashup (`mashupType: targetmashup`), with exactly one
`page_host: {id: host}` where ordinary pages render. `new mashup --spec` uses the same root
as `new mashup --master`, including `DefaultMasterMashupStyle`, `WindowStyle`, `Favicon` and
`Font`. `page.props` carries mashup-root properties; `page.root.props` sets the template's root
container. Native `menu_bar` uses `ptcsmenubar` properties and bindings through `props`.
Derive recognises the host, menu and header elements. Untouched derived masters adopt with
no existing XML or sidecar changes and keep widget and wiring IDs in the baseline.
An ordinary page still attaches through `page.master: Acme.Master_MM` and
`page.use_master_theme: true`. A missing or duplicate host is refused with a repair instruction.
These contracts are tested offline, with no new live-server claim.

## Refresh, chain, status message, rich text and rte

Refresh after a write with `events: [{on: publish.done, invoke: status}, {on: publish.done,
invoke: messages}]`. `.done` emits `ServiceInvokeCompleted`; `.error` emits `Data_Error`.
Use `{on: publish.error, show: failed}` with `status_message: {id: failed, message: Failed,
message_type: error}`. Message text is bindable and its type is `info`, `warning` or `error`.
The command equivalent is `mashup on --event data:Acme.Messages.Publish.done --invoke
data:Acme.Messages.GetStatus`; failures invoke `ui:<statusmessage-id>.ShowMessage`.
`mashup event` creates parent-observable signals; refresh chains use `mashup on`.

Several calls can update one label: add `bindings: [{from: start.result, to: output.LabelText},
{from: stop.result, to: output.LabelText}]` under `page`. Derive keeps every source. Commands
use one `mashup bind` per pair.

`rich_text: {id: header, content: '<h2>Acme</h2>', read_only: true, show_toolbar: false,
font_size: 18, size: {fixed: 56}}` creates native `ptcsrte` (RTE). Content accepts a binding;
font size accepts a catalogue number or `Npx`. Its native default height is 280px; use fixed
size for a short header, or `size: fill` for a growing placement. Commands use `mashup add
ptcsrte --prop Data=... --prop ReadOnly=true --prop HideToolbar=true --prop DefaultFontSize=18`.
These are offline compiler features; no new live validation is claimed.

## Tokens and theme preset styling for a page spec

Use design tokens instead of hex values. `twaco mashup tokens` lists presets;
`twaco mashup tokens brutalist --json` shows resolved tokens and native theme variables.
Set top-level `tokens: brutalist` in page.yaml, generate its native StyleTheme with
`twaco new theme Acme.Orders_TH --tokens brutalist --apply`, then set
`page.theme: Acme.Orders_TH`. Tokens resolve colours, fonts, borders, radii and shadows;
they do not create or select the theme automatically. Presets are convergence (default),
clean and brutalist. A token YAML file can extend a preset with solution values.

## Link, image, icon, divider, checkbox, radio, toggle, textarea, date_picker, value_display and embed

Native controls accept readable fields and bindings. Use `link` for text/url/target, `image` for
Media entity or URL/scaling/alt, `icon` for CDS icon/size/token colour, and `divider` for orientation
and token colour. `checkbox`, `radio` and `toggle` bind boolean `value`; radio has `group` or static
`options`. `textarea` adds hint/max_length; `date_picker` binds one DateTime (`date_range` still
selects a range); `value_display` pairs a label with a scalar or binding. `embed` takes a static
mashup and checked `params`. The command paths are native `mashup add`, `set`, `bind` and
`add-contained`. See documentation/PAGE_SPECS.md for the field/property mappings and examples.

## Expression, expression2, events_router, eventsrouter, navigate, navigationfunction, pagination and kit variants

A data entry may use `expression`, typed or inferred `inputs`, and `output: BOOLEAN` (or another
base type). Bind to its alias or alias.Output; invoke alias.Evaluate. Native evaluation flags
survive derivation. Event `navigate: {mashup: Acme.Detail_MU, mode: popup, params: {id: {bind:
rows.Name}}}` shares `recipe popup`; modes include page/new_tab/popup/modal-popup. Native service
pagination uses `mode: server`, total/page_size/page_number and explicit events, keeping existing
slicing expressions. Unbound dropdowns use `unbound: true`; collections may preserve native
layout through `source: {bind: rows}`, mashup and map. Tabs recognise Items labels; KPI dials
recognise native layouts. Charts add pareto/waterfall; pie recognises DataField/ValueField;
combo uses explicit series. Schedule needs nested resource intervals and stays raw. The kits'
typed fan-in routers use `events_router`, `data_type`, `routes` bindings and Output. Dynamic
panels, decorative shapes and map extensions retain explicit raw contracts;
derivation notes explain unsupported dynamic sources or targets. Evidence is offline.

## Full-bleed side columns, nav_menu selection, cell image/status and clean cards

V1 side panels and leading filter columns put the gutter on their content sibling. Filter
panels reserve a title row and inside collapse tab above the fields, with the kit background
and a separate vertical divider. Trend values align left below captions; chart legends sit
compactly at the top and single-series bars hide them unless explicitly requested. Pie charts
keep legends and value markers. Grid row rules replace a detached outer bottom frame.

Use `nav_menu` with `selected: Details` and items `[Overview, {label: Details, indent: 1}]`.
Selected must name one item label; indent is 0 to 12 levels. Generated collections accept
`cell: {image: Image, title: Name, status: {field: Status, state: Acme.Status_ST}}`.
The built-in nav_menu and collection components work through `recipe component` too.
Clean cards use white, grey-200 borders, 8px corners and soft shadows on a grey-50 page with
Inter. The Acme fixtures assert native structure/styles offline; this is not a live finding.

## Full-bleed side columns, nav_menu selection, cell image/status and clean cards

V1 side panels and leading filter columns put the gutter on their content sibling. Filter
panels reserve a title row and inside collapse tab above the fields, with the kit background
and a separate vertical divider. Trend values align left below captions; chart legends sit
compactly at the top and single-series bars hide them unless explicitly requested. Pie charts
keep legends and value markers. Grid row rules replace a detached outer bottom frame.

Use `nav_menu` with `selected: Details` and items `[Overview, {label: Details, indent: 1}]`.
Selected must name one item label; indent is 0 to 12 levels. Generated collections accept
`cell: {image: Image, title: Name, status: {field: Status, state: Acme.Status_ST}}`.
The built-in nav_menu and collection components work through `recipe component` too.
Clean cards use white, grey-200 borders, 8px corners and soft shadows on a grey-50 page with
Inter. The Acme fixtures assert native structure/styles offline; this is not a live finding.

## Editable grid text columns

A grid column can set `{field: Notes, editable: true, editor: textarea,
preserve_whitespace: true}`. Editors are `textfield` or `textarea`; textarea requires
STRING, HTML or HYPERLINK. `preserve_whitespace` requires textarea and the STRING
renderer. The shared columns recipe writes `cellEditor` and the boolean
`preserveWhiteSpaces` (plural), including `false` when unset. Command equivalents are
`--editable-column Notes --column-editor Notes=textarea --preserve-whitespace Notes`.
