# ThingWorx service code reference

Status: curated ThingWorx service-code reference

Examples use a sample solution, `Acme.Dashboards`; the patterns are general.

## Purpose

This guide distills recurring ThingWorx server-side JavaScript patterns into reusable forms.
Consult it before creating or substantially changing a service, especially one that returns an
InfoTable.

The templates below are intentionally generic. Never copy credentials, private endpoints,
test Thing names, or debug side effects into production code.

## Service JSDoc and comments

ThingWorx service scripts are top-level script bodies rather than JavaScript function
declarations. Document each one as a virtual function whose contract matches the paired
`definition.xml`:

```javascript
/**
 * Cards on one dashboard, in card order.
 *
 * @function GetDashboardCards
 * @param {number} dashboardUid - UID of the dashboard whose cards to return.
 * @param {string} searchTerm - Optional case-insensitive title match.
 * @returns {Object} An InfoTable shaped by Acme.Example.Result_DS.
 */
```

Use these JSDoc type mappings consistently:

| ThingWorx base type | JSDoc type |
|---|---|
| `NUMBER`, `INTEGER`, `LONG` and other numeric types | `number` |
| `STRING`, `USERNAME`, `THINGNAME`, `MASHUPNAME`, `PROPERTYNAME`, `SERVICENAME` | `string` |
| `BOOLEAN` | `boolean` |
| `DATETIME` | `Date` |
| `INFOTABLE`, `JSON` | `Object` |
| `NOTHING` | `void` |

For `INFOTABLE`, name the declared DataShape in `@returns` or `@param`. For `JSON`,
name the payload contract in the description. Keep the header synchronized with
`definition.xml`; JSDoc never substitutes for ThingWorx result metadata.

The first statement follows the closing `*/` directly. Do not put another service-level
`//` block between the JSDoc and the code. Fold useful contract context into the JSDoc;
move implementation rationale next to the statement or branch it explains.

Wrap JSDoc prose and tag descriptions at 100 columns. A documented helper also has one
documentation source: never place an explanatory `//` block immediately before its JSDoc.
Condense useful rationale into the JSDoc and remove duplicated narration.

Document a local helper when a reviewer needs its type contract, side effects, failure
mode, or fallback behavior to use it safely:

```javascript
/**
 * Validates an identifier read from persisted card data.
 *
 * @param {*} value - Persisted UID value.
 * @param {string} fieldName - Field used for error context.
 * @returns {number} A positive integer UID.
 * @throws {Error} When persisted card data is corrupt.
 */
function requiredPersistedUid(value, fieldName) {
    // ...
}
```

Do not add JSDoc mechanically to a trivial helper whose name and expression already make
its contract obvious.

Comments should explain why the code exists: a verified ThingWorx engine quirk, a
DataShape constraint, an intentional fallback, or a tradeoff that a future edit could
accidentally undo. Remove comments that paraphrase the next statement, duplicate the
service header, divide code with decorative rules, or label every `AddRow` value with an
obvious base type. A targeted type comment is appropriate only when it guards a real
coercion boundary, such as converting a numeric application UID to `STRING` for inherited
DBConnection services.

## Service output contract

An InfoTable-returning service has three pieces that must agree:

1. A standalone DataShape defines the fields and base types.
2. The service definition declares `baseType="INFOTABLE"` and names that DataShape.
3. The service script creates and returns an InfoTable with the same shape.

### Service definition

In a service sidecar `definition.xml`:

```xml
<ResultType
 aspect.dataShape="Acme.Dashboards.Example_DS"
 baseType="INFOTABLE"
 description=""
 name="result"
 ordinal="0"></ResultType>
```

The complete service definition and implementation are synchronized through:

```powershell
twaco sync Acme.Dashboards.Management_TS --check
```

Do not change only the JavaScript when the result DataShape or service signature also
changes.

### Preferred typed result construction

ThingWorx supports both forms:

```javascript
let result = Resources["InfoTableFunctions"].CreateInfoTableFromDataShape({
    infoTableName: "InfoTable",
    dataShapeName: "Acme.Dashboards.Example_DS"
});
```

and:

```javascript
let result = DataShapes["Acme.Dashboards.Example_DS"].CreateValues();
```

Both create a typed, initially empty InfoTable. Prefer the first form when constructing
a service result because its intent is explicit. `DataShapes["..."].CreateValues()` is
also appropriate and is common when preparing typed rows for another ThingWorx API.

For a DataTable write, use the DataTable's own schema:

```javascript
let values = Things["Acme.Dashboards.Example_DT"].CreateValues();
```

### Complete output template

```javascript
let result = Resources["InfoTableFunctions"].CreateInfoTableFromDataShape({
    infoTableName: "InfoTable",
    dataShapeName: "Acme.Dashboards.Example_DS"
});

let source = me.GetSourceRows({
    filter: filter // STRING
});

source.rows.toArray().forEach((row) => {
    result.AddRow({
        id: row.id,                              // GUID
        name: String(row.name || ""),            // STRING
        value: Number(row.value),                // NUMBER
        timestamp: new Date(row.timestamp),      // DATETIME
        enabled: Boolean(row.enabled)             // BOOLEAN
    });
});
```

ThingWorx uses the variable named `result` as the service return value. Initialize it
before optional work so the service returns an empty typed InfoTable instead of
`undefined` when no rows match.

These patterns cover typed reports, row projection, multi-pass calculation, and fallback
rows without depending on a private source collection.

## Adding rows

Use a plain object whose keys match the output DataShape:

```javascript
result.AddRow({
    id: generateGUID(),               // GUID
    dashboardName: dashboardName,     // STRING
    createdDate: new Date(),          // DATETIME
    ownedBy: ownerName                // USERNAME
});
```

Use comments for non-obvious base types. They help catch mismatches during review.

### Normalize values before `AddRow`

Do not assume source rows expose values with the output DataShape's runtime type.

```javascript
const parsedValue = parseFloat(row.value);
const parsedDate = new Date(row.timestamp);

if (isNaN(parsedValue)) {
    logger.warn("{} - skipping row {} because value is not numeric", me.name, row.id);
    return;
}

if (isNaN(parsedDate.getTime())) {
    logger.warn("{} - skipping row {} because timestamp is invalid", me.name, row.id);
    return;
}

result.AddRow({
    id: String(row.id),               // STRING
    value: parsedValue,               // NUMBER
    timestamp: parsedDate             // DATETIME
});
```

Useful conversions:

| Target base type | Typical conversion |
|---|---|
| STRING, USERNAME, THINGNAME, MASHUPNAME | `String(value)` after null handling |
| NUMBER | `parseFloat(value)` plus `isNaN` check |
| INTEGER, LONG | `parseInt(value, 10)` plus `isNaN` check |
| BOOLEAN | Explicit domain conversion; do not use `Boolean("false")` |
| DATETIME | `new Date(value)` plus `isNaN(date.getTime())` check |
| GUID | Preserve/generate a valid GUID string |
| INFOTABLE | Supply an InfoTable matching the field's nested DataShape |

For booleans that may arrive as strings:

```javascript
function toBoolean(value) {
    if (typeof value === "boolean") {
        return value;
    }
    return String(value).toLowerCase() === "true";
}
```

### Nested InfoTables

Chart data can be built as an InfoTable and placed in a parent row when the parent
DataShape field is `INFOTABLE`.

```javascript
let series = Resources["InfoTableFunctions"].CreateInfoTableFromDataShape({
    infoTableName: "Series",
    dataShapeName: "PTC.Charts.TimeSeries_DS"
});

source.rows.toArray().forEach((row) => {
    series.AddRow({
        timestamp: new Date(row.timestamp), // DATETIME
        value: parseFloat(row.value)         // NUMBER
    });
});

result.AddRow({
    cardId: cardId,                         // GUID
    history: series                         // INFOTABLE
});
```

For production code, prefer a named DataShape over constructing the nested shape
dynamically.

## Reading and iterating InfoTable rows

### `rows.toArray().forEach`

Use this for transformation-only passes where no early exit or row deletion is needed:

```javascript
source.rows.toArray().forEach((row) => {
    result.AddRow({
        id: row.id,
        displayName: row.displayName
    });
});
```

Why call `toArray()`:

- it gives standard JavaScript array iteration;
- callback scoping is clearer than legacy `for each`;
- it avoids depending on implementation-specific methods of ThingWorx's row
  collection;
- it avoids engine-specific surprises seen with indexed access.

Do not mutate or delete rows from the source InfoTable inside this loop.

### Indexed loops

Use an indexed loop when the index matters, when reading adjacent rows, when
short-circuiting, or when carefully deleting:

```javascript
for (let i = 0; i < source.rows.length; i++) {
    const row = source.rows[i];

    if (row.id === requestedId) {
        result = row;
        break;
    }
}
```

Indexed access is appropriate for ordered calculations, first/last timestamps,
adjacent-row comparisons, and mapping generated item numbers.

Use `source.rows.length`, not `source.length`, for row count.

### Empty and first-row guards

Never read `rows[0]` without checking. Structure the surrounding work so an empty
table simply leaves the initialized result unchanged:

```javascript
if (source && source.rows.length > 0) {
    const firstRow = source.rows[0];
    result.AddRow({
        id: firstRow.id
    });
}
```

When a first row is optional:

```javascript
const detail = details.rows.length > 0 ? details.rows[0] : null;

result.AddRow({
    name: row.name,
    value: detail ? detail.value : 0
});
```

### Row counts

Both forms are supported:

```javascript
const count = table.rows.length;
```

```javascript
const count = table.getRowCount();
```

Prefer `rows.length` when already working with the rows collection. `getRowCount()` is
clear when the count itself is the result.

### Multiple passes

Multiple passes are appropriate when the second pass depends on a value calculated
from all rows:

```javascript
let total = 0;

source.rows.toArray().forEach((row) => {
    total += Number(row.value) || 0;
});

if (total > 0) {
    source.rows.toArray().forEach((row) => {
        result.AddRow({
            category: row.category,
            percentage: (Number(row.value) || 0) / total
        });
    });
}
```

## Filtering InfoTables

Build query filters explicitly:

```javascript
const query = {
    filters: {
        type: "AND",
        filters: []
    }
};

if (ownerName && ownerName.trim() !== "") {
    query.filters.filters.push({
        fieldName: "ownedBy",
        type: "EQ",
        value: ownerName
    });
}

if (searchTerm && searchTerm.trim() !== "") {
    query.filters.filters.push({
        fieldName: "dashboardName",
        type: "LIKE",
        value: "%" + searchTerm.trim() + "%",
        isCaseSensitive: false
    });
}
```

Filter an existing InfoTable:

```javascript
if (query.filters.filters.length > 0) {
    result = Resources["InfoTableFunctions"].Query({
        t: result,
        query: query
    });
}
```

Query a DataTable:

```javascript
let rows = Things["Acme.Example.Items_DT"].QueryDataTableEntries({
    maxItems: 500,
    query: query,
    values: undefined,
    source: undefined,
    tags: undefined
});
```

This pattern works for both DataTable lookups and post-processing generated InfoTables.

## Aggregating InfoTables

Use `InfoTableFunctions.Aggregate` for grouping, counts, and distinct values:

```javascript
let grouped = Resources["InfoTableFunctions"].Aggregate({
    t: source,
    columns: "status",
    aggregates: "COUNT",
    groupByColumns: "category"
});
```

Treat the aggregate result as a new InfoTable and project it into a declared output
DataShape if the generated fields do not exactly match the service result contract.

Avoid writing temporary tables to a debug Thing from production services.

## DataTable writes

Prepare a typed values InfoTable before calling a DataTable service:

```javascript
let values = Things["Acme.Example.Items_DT"].CreateValues();
values.id = generateGUID();            // GUID
values.dashboardName = dashboardName;  // STRING
values.createdDate = new Date();       // DATETIME
values.ownedBy = ownerName;            // USERNAME

Things["Acme.Example.Items_DT"].AddOrUpdateDataTableEntry({
    tags: [],
    source: me.name,
    values: values,
    location: {
        latitude: 0,
        longitude: 0,
        elevation: 0,
        units: "WGS84"
    }
});
```

For updates, set the primary key and only fields intentionally being changed:

```javascript
let values = Things["Acme.Example.Items_DT"].CreateValues();
values.id = dashboardId;
values.dashboardName = newName;

Things["Acme.Example.Items_DT"].UpdateDataTableEntry({
    values: values,
    sourceType: undefined,
    location: undefined,
    source: me.name,
    tags: undefined
});
```

For a delete:

```javascript
Things["Acme.Example.Items_DT"].DeleteDataTableEntryByKey({
    key: dashboardId
});
```

Read the existing row first when validating ownership, detecting a change, or creating
an audit event. Do not rely on the mashup to enforce authorization.

## Dynamic Thing, property, and service access

Runtime-selected Things and properties use bracket access:

```javascript
const value = Things[thingName][propertyName];
```

and dynamic services:

```javascript
const serviceResult = Things[managerName][serviceName]({
    thingName: thingName,
    propertyName: propertyName
});
```

Before using values supplied by a user or DataTable row:

- confirm the names are present and meaningful;
- enforce the permitted Thing/ThingTemplate boundary;
- verify the service is intended for dynamic invocation;
- validate the returned InfoTable before accessing its first row;
- log entity context without logging secrets.

Dynamic invocation is a capability boundary, not just a convenience.

## JSON and dynamic InfoTables

### Prefer named DataShapes

Most service results should use a named DataShape. This gives Composer, mashup
bindings, validation, and future agents a stable contract.

### `FromJSON`

For a genuinely dynamic or nested shape:

```javascript
const json = {
    dataShape: {
        fieldDefinitions: {
            name: {
                name: "name",
                baseType: "STRING"
            },
            value: {
                name: "value",
                baseType: "NUMBER"
            }
        }
    },
    rows: []
};

let table = Resources["InfoTableFunctions"].FromJSON({
    json: json
});
```

### `AddField`

Fields derived from JSON keys can be added at runtime:

```javascript
table.AddField({
    name: fieldName,
    baseType: baseType
});
```

Use this only when the schema truly cannot be known at design time. Runtime-added
fields are harder to bind in mashups and cannot satisfy a stable service output
DataShape without additional validation.

### InfoTable to plain JSON

For an external request, create plain arrays and objects explicitly:

```javascript
const items = [];

source.rows.toArray().forEach((row) => {
    items.push({
        id: String(row.id),
        quantity: Number(row.quantity)
    });
});

const payload = {
    items: items
};
```

Use `JSON.stringify(payload)` only if the called API requires a string. Do not copy
credentials or intentionally malformed JSON from experiments.

## Error handling

For a service-level failure:

```javascript
try {
    // service work
} catch (err) {
    logger.error(
        "{} - {}:{} - {}",
        me.name,
        err.fileName,
        err.lineNumber,
        err
    );
    throw "Unable to load dashboard data.";
}
```

For recoverable per-row work:

```javascript
source.rows.toArray().forEach((row) => {
    try {
        const detail = Things[row.thingName].GetDetail();
        result.AddRow(mapDetail(detail));
    } catch (err) {
        logger.warn(
            "{} - unable to load detail for Thing {}: {}",
            me.name,
            row.thingName,
            err
        );
    }
});
```

Do not silently return incomplete data. Make the fallback and its log level
intentional.

## Patterns to avoid

Avoid these unsafe or outdated forms:

- hard-coded usernames, passwords, app keys, or private endpoints;
- assigning temporary results to a debug Thing;
- implicit globals such as undeclared `row`, `i`, `params`, or `result`;
- legacy `for each (row in table.rows)` syntax;
- using `table.length` when the row count is `table.rows.length`;
- reading `rows[0]` without an empty check;
- returning a DataShape different from the service definition's `aspect.dataShape`;
- creating runtime fields when a stable DataShape can be declared;
- swallowing exceptions or exposing raw exception details to end users;
- using string defaults for NUMBER, BOOLEAN, or DATETIME output fields;
- modifying a source InfoTable while iterating a `toArray()` snapshot;
- per-row remote calls without considering batching, latency, and partial failure.

## Review checklist for an InfoTable service

Before syncing:

- [ ] `script.js` starts with a definition-aligned `@function`, `@param`, and `@returns` header.
- [ ] The first statement follows the service JSDoc with no second top-level comment block.
- [ ] Structured JSDoc values name their DataShape or JSON payload contract.
- [ ] Non-trivial helpers document contracts or side effects that are not obvious from the code.
- [ ] JSDoc is wrapped at 100 columns and has no explanatory `//` preamble.
- [ ] Comments explain intent or constraints without narrating code or repeating documentation.
- [ ] Base-type comments are retained only at genuine coercion boundaries.
- [ ] `definition.xml` declares `baseType="INFOTABLE"`.
- [ ] `aspect.dataShape` names the intended output DataShape.
- [ ] Every `AddRow` key exists in that DataShape.
- [ ] Every value is normalized to its declared base type.
- [ ] Empty input returns an empty typed InfoTable.
- [ ] `rows[0]` access is guarded.
- [ ] `rows.toArray().forEach` is used only for safe transformation passes.
- [ ] Indexed loops are used when order, early exit, or adjacent rows matter.
- [ ] Dynamic Thing/property/service names are validated and constrained.
- [ ] DataTable writes use `CreateValues()`.
- [ ] Errors include diagnostic context without secrets.
- [ ] No temporary debug writes or hard-coded test entities remain.

Then run:

```powershell
twaco sync Acme.Dashboards.Management_TS --check
twaco check
```

This document is the stable reference for agents working on ThingWorx service code.
