# Collections: the serialized contract

A developer note for authoring `ptcscollection` widgets: what Composer stores and what Runtime
reads. It comes from 32 collections in 30 mashups of a published design kit, read on ThingWorx
10.2, and a collection built with twaco and checked in Runtime. No kit content is in the repository;
the test fixtures are synthetic.

## What a collection stores

A collection repeats one cell mashup, once per row of the infotable bound to its `Data`.

| Property | Type | Meaning |
| --- | --- | --- |
| `Mashup` | MASHUPNAME | the cell mashup |
| `MashupPropertyBinding` | JSON object | `{"<row field>": "<cell parameter>"}`: each row's field value goes into that cell parameter |
| `MashupGlobalPropertyBinding` | JSON object | `{"<cell parameter>": "<BASETYPE>"}`: a cell parameter set once for every cell from a collection property of the same name; often `{}` |
| `UIDField` | FIELDNAME | the row field that identifies a cell (selection and updates); optional |
| `SelectedCellMashup`, `HeaderMashup` | MASHUPNAME | optional: the mashup a selected cell shows, and a header mashup |
| `CellWidth`, `CellHeight`, `Layout`, gaps, paddings | scalars | layout; defaults from the catalogue |

`MashupPropertyBinding` is a JSON object in the stored content, not a string. The key is the row
field and the value is the cell parameter: `{"description": "descriptionProperty"}` puts the
`description` field into the cell's `descriptionProperty` parameter.

Data reaches the collection by an ordinary whole-result binding:
`SourceArea Data`, `SourceDetails AllData`, `TargetId <collection>`, `TargetProperty Data`.

## Events and outgoing values

- Events: `CellClicked`, `CellDoubleClicked`, `CellLongClicked`, `CellRightClicked`,
  `CellMoved`, `CellRemoved`, `CellAdded`, and `CellMenu:<item>` when a `CellMenu`
  StateDefinition is set. Services: `Deselect`, `SelectAll`.
- The cell menu (10.2): every state with a `defaultValue` is an item, labelled with that value,
  its `description` the tooltip and its style's `image` the icon. The item fires
  `CellMenu:<value>` with spaces made hyphens and anything but letters, digits, `:`, `_` and `-`
  left out: "More Details" fires `CellMenu:More-Details`. `mashup on` takes only that form and
  refuses an item the repository's StateDefinition lacks; `mashup validate` reports one.
- `Event:<row field>` is a binding source on the collection: the field of the row whose cell
  fired the last event. In the kit every `Event:` name is a field of the bound DataShape, not a
  cell parameter.

## What Composer tolerates

- A mapping to a parameter the cell does not have. The kit has several, such as `imageURL` mapped
  for a cell with no `imageURL` parameter. Runtime ignores it. An authoring command must refuse
  it; validation may report it.
- No `UIDField`: 15 of the 32 have none.

## Verified in Runtime

A parent mashup with a collection, built with existing commands only, rendered one cell per row
with each row's values in the cell's labels (ThingWorx 10.2):

```text
twaco mashup new P.Cell_MU --param Day:STRING --param Value:NUMBER --apply
twaco mashup add P.Cell_MU label --apply                       # bound to mashup:Day, mashup:Value
twaco mashup source add P.Page_MU P.Kpi_TS GetChartData --entity-from mashup:Manager --apply
twaco mashup add P.Page_MU ptcscollection --prop Mashup=P.Cell_MU --prop UIDField=Date \
    --prop CellWidth=200 --prop CellHeight=120 --apply
twaco mashup set P.Page_MU ptcscollection-3 --prop 'MashupPropertyBinding={"Date":"Day","Value":"Value"}' --apply
twaco mashup bind P.Page_MU --from data:P.Kpi_TS.GetChartData --to ui:ptcscollection-3.Data --apply
twaco mashup on P.Page_MU --event mashup:Loaded --invoke data:P.Kpi_TS.GetChartData --apply
```

- Without a call on `Loaded` (or another trigger) the service never runs and the collection
  shows its "no data bound" state. `mashup source add` adds that event only with `--on-load`.
- `CellWidth` and `CellHeight` are the cell's size in pixels; the cell mashup's content must fit
  it, or the cell scrolls.

## Command

```text
twaco mashup collection add <parent> <cell-mashup> --data <data:Entity.Service>
    --map <row field>=<cell parameter>[,<row field>=<cell parameter>...] [--uid <row field>]
    [--width <px>] [--height <px>]
    [--in <container> | --right-of <widget> | --left-of <widget> | --above <widget> | --below <widget>] [--apply] [--json]
```

It adds a `ptcscollection` with `Mashup`, `MashupPropertyBinding`, optional `UIDField`, and cell
dimensions, then binds the whole result to its `Data` in one edit. The call must already exist in
the parent. It refuses a cell mashup not in the solution, an output-only or missing cell parameter,
a duplicate mapping, a row field the result's DataShape lacks (when the shape is known), and a
field whose base type does not fit the parameter. Plan unless `--apply`; MCP is
`mashup_collection_add` with `dry_run`.

`mashup validate` then reports, for a collection whose data has a known shape: a mapped row field
the shape lacks (`unknown-field-property`), and a mapping to a parameter the cell lacks.
