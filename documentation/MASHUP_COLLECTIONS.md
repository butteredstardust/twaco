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

## How cells are laid out (10.2)

Read from the 10.2 collection widget's source and checked against the kit's rendering:

- With `UseMashupDimensions` false, every cell is exactly `CellWidth` wide (plus the cell-menu
  strip when the actions sit beside the cell) and `CellHeight` tall, plus a row for the cell menu
  when `CellActionsPosition` is `bottom` (the default). That row measured 50 px in Runtime, so a
  270 px cell with a menu takes 320 px.
- `Layout=flex` starts a new row when `n * CellWidth + (n - 1) * ColumnGap` reaches the
  collection's width less `LeftPadding`, `RightPadding` and its scrollbar. `ColumnGap` is the
  minimum space between cells; `RowHorizontalAlignment=space-evenly` spreads what is left over
  evenly around the cells.
- How many cells a row holds therefore depends on the window. The kit's card page (400x270 cells)
  shows four per row only when the collection is over about 1,780 CSS px wide (its reference
  rendering was about 2,200 CSS px wide); in a 1440 px window at 100% it shows three.
- `LeftAlignLastRow` pads a short last row with invisible cells of the same width, so its cells
  sit under the first columns instead of being centred.
- `RowHorizontalAlignment=stretch` gives each cell `flex: 1 0 auto`: `CellWidth` becomes a
  minimum and the cells grow to fill each row (with `LeftAlignLastRow`, the last row's cells keep
  the columns' width). This fills the width at any window size, but it was not checked in Runtime.

The kit's card collection (six KPI dial cards) sets, beside the catalogue defaults it keeps
(`Layout` flex, `RowHorizontalAlignment` space-evenly, `ColumnGap` and `RowGap` 40, `TopPadding`,
`RightPadding` and `BottomPadding` 24): `CellWidth` 400, `CellHeight` 270, `LeftAlignLastRow`
true, `ContentAlignment` top, `MashupBackground` transparent, `LeftPadding` 15, `SelectionMode`
single, `AutoSelectFirstRow` true and a `CellMenu`. The card's frame is the collection's `cell`
part style: a 1px solid `rgba(192,192,192,1)` border with a 6px radius, repeated in the hover,
selected and pressed states (pressed also sets a white background). `--preset cards` writes
exactly these.

## Command

```text
twaco mashup collection add <parent> <cell-mashup> --data <data:Entity.Service>
    --map <row field>=<cell parameter>[,<row field>=<cell parameter>...] [--uid <row field>]
    [--width <px> | --columns <n> [--across <px>]] [--height <px>]
    [--preset cards] [--menu <StateDefinition>]
    [--in <container> | --right-of <widget> | --left-of <widget> | --above <widget> | --below <widget>] [--apply] [--json]
```

It adds a `ptcscollection` with `Mashup`, `MashupPropertyBinding`, optional `UIDField`, and cell
dimensions, then binds the whole result to its `Data` in one edit.

- `--columns <n>` works `CellWidth` out from the wrap rule above, so `n` cells fit on a row of a
  collection `--across` px wide (default 1400: a full-width collection in a 1440 px window), with
  a gap kept at each end so space-evenly still shows space around the row. It is a width for one
  window size: a narrower window shows fewer cells per row, a wider one more. `--width` stays
  for an exact size, and the two cannot be combined.
- `--preset cards` writes the kit's card collection described above, frame included. Without
  `--height`, cells are the kit card's 270 px tall at any width: a narrower card still needs the
  height of its content, and a 192 px cell under a title and a filling dial scrolled.
- `--menu <StateDefinition>` sets `CellMenu`, refusing a StateDefinition the solution lacks; the
  result lists the `CellMenu:<item>` events its states fire. The call must already exist in
the parent. It refuses a cell mashup not in the solution, an output-only or missing cell parameter,
a duplicate mapping, a row field the result's DataShape lacks (when the shape is known), and a
field whose base type does not fit the parameter. Plan unless `--apply`; MCP is
`mashup_collection_add` with `dry_run`.

`mashup validate` then reports, for a collection whose data has a known shape: a mapped row field
the shape lacks (`unknown-field-property`), and a mapping to a parameter the cell lacks.
