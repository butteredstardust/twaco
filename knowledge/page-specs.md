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
Use `mashup set` or recipes for small edits. `twaco mashup why Acme.Orders_MU tools`
explains the layout decisions. The full element reference is documentation/PAGE_SPECS.md.

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
panels, menu bars, decorative shapes and map extensions retain explicit raw contracts;
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
