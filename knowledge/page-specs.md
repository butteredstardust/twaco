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
