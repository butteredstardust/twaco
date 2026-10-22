# ThingWorx service code reference

Status: twaco's service-writing reference. Examples use neutral Acme.Orders entities.

## Service contracts and results

An INFOTABLE service has one contract in three places: a standalone DataShape, the service
definition's INFOTABLE result with that DataShape aspect, and the InfoTable created by script.
JSDoc explains the contract but never replaces metadata. Sync the definition and implementation
together with twaco sync Acme.Orders.Manager_TS --check.

Create a typed result with Resources["InfoTableFunctions"].CreateInfoTableFromDataShape using an
InfoTable name and Acme.Orders.OrderResult_DS. DataShapes["Acme.Orders.OrderResult_DS"].CreateValues
does the same thing and suits rows for another API. For a DataTable write, use the target's own
Things["Acme.Orders.Orders_DT"].CreateValues. Initialize result before optional work: the variable
called result is returned, so no matches should produce an empty typed table rather than undefined.

The service sidecar is `definition.xml`: its result must have `baseType="INFOTABLE"` and an
`aspect.dataShape` that names the same contract as the script. A typed result can also be made
with `DataShapes["..."].CreateValues()`. Do not let an output table drift from the definition
just because the server currently accepts it.

The implementation sidecar is `script.js`; keep it synchronized with that definition.

```javascript
let result = Resources["InfoTableFunctions"].CreateInfoTableFromDataShape({
    infoTableName: "Orders",
    dataShapeName: "Acme.Orders.OrderResult_DS"
});

me.GetOrders({ customerId: customerId }).rows.toArray().forEach((order) => {
    const placedAt = new Date(order.placedAt);
    const total = parseFloat(order.total);
    if (!isNaN(placedAt.getTime()) && !isNaN(total)) {
        result.AddRow({
            orderId: String(order.orderId),
            placedAt: placedAt,
            total: total
        });
    }
});
```

## JSDoc and comments

Service bodies are scripts, not JavaScript function declarations. Give each an immediately
adjacent virtual-function JSDoc header with @function, @param, and @returns tags. Use number for
numeric ThingWorx types; string for STRING, USERNAME, THINGNAME, MASHUPNAME, PROPERTYNAME, and
SERVICENAME; boolean for BOOLEAN; Date for DATETIME; Object for INFOTABLE and JSON; and void for
NOTHING. Name an InfoTable's DataShape and describe a JSON payload contract.

The first statement follows the closing comment directly. Do not place another service-level
comment block there. Wrap JSDoc prose and tag descriptions at 100 columns. A helper needs JSDoc
only when its types, side effects, failure mode, or fallback affect safe use; do not duplicate it
in an explanatory comment immediately before the helper.

Comments preserve reasons: a verified engine limitation, DataShape constraint, intentional
fallback, or trade-off. Remove narration, decorative dividers, duplicate headers, and obvious
AddRow type labels. A type comment can be worthwhile at a real coercion boundary, such as changing
a numeric application identifier into STRING for an inherited DBConnection service.

## Constructing rows

AddRow receives a plain object keyed by the output DataShape. Normalize values before it reaches
that object. After null handling, use String for string-like fields; parseFloat plus an isNaN check
for NUMBER; parseInt(value, 10) plus an isNaN check for INTEGER and LONG; new Date plus an invalid
date check for DATETIME; valid GUID strings for GUID; and an InfoTable matching the nested
DataShape for INFOTABLE. Map input booleans deliberately: Boolean("false") is true.

For an optional value, use `String(value)` only after null handling. NUMBER conversion is
`parseFloat(value)` followed by validation; DATETIME conversion is `new Date(value)` and
`isNaN(date.getTime())`. These checks keep an invalid source value out of the service result.

For chart-like nested results, build a typed series InfoTable and use it as an INFOTABLE value in
the parent row. Prefer a declared nested DataShape over dynamically constructed fields.

## InfoTable traversal

Use rows.toArray().forEach for transform-only work without deletion or early exit. It supplies
normal JavaScript array iteration, clearer scope than legacy for each, and avoids engine-specific
row-collection and indexed-access behaviour. Do not mutate source rows during that pass.

Use an indexed loop when position, adjacent rows, careful deletion, or an early break matters.
The row count is table.rows.length, not table.length; getRowCount is also valid and reads well
when the count itself is the point. Never access rows[0] before an empty check. An optional detail
should become null or an explicit fallback. Multiple passes are appropriate when the later pass
uses a value accumulated across all rows.

## Querying, grouping, and tables

Build query filters explicitly. Add only meaningful AND filters, then call
Resources["InfoTableFunctions"].Query for a non-empty filter set. QueryDataTableEntries should
receive an explicit maxItems, query, values, source, and tags. The same query shape works for
DataTable lookups and post-processing an InfoTable.

Use Resources["InfoTableFunctions"].Aggregate for grouping, counts, and distinct values. Its
output is a new InfoTable; project it into the declared service result when its fields differ.
Do not store temporary work on a debug Thing.

For a DataTable write, create typed values on the DataTable, set only deliberate fields, and call
AddOrUpdateDataTableEntry with tags, source, values, and location as appropriate. Updates include
the primary key and use UpdateDataTableEntry; DeleteDataTableEntryByKey removes by key. Read the
existing row before ownership, audit, or change decisions. A mashup cannot be trusted to enforce
authorization.

```javascript
let values = Things["Acme.Orders.Orders_DT"].CreateValues();
values.orderId = orderId;
values.status = "Ready";
Things["Acme.Orders.Orders_DT"].UpdateDataTableEntry({
    values: values,
    sourceType: undefined,
    location: undefined,
    source: me.name,
    tags: undefined
});
```

## Dynamic access and JSON

Dynamic Thing, property, and service calls use bracket access, such as
Things[thingName][propertyName] and Things[managerName][serviceName](parameters). Before using a
name from a user or a DataTable row, require a useful value, enforce its Thing or template
boundary, verify that the service may be dynamically invoked, validate returned InfoTables before
their first row, and log context without secrets. This is a capability boundary.

Most results need a named DataShape for Composer, mashup bindings, validation, and later readers.
For an actually unknown or nested schema, InfoTableFunctions.FromJSON accepts a dataShape with
fieldDefinitions and rows. AddField is only for a schema that cannot be known at design time:
runtime fields are hard to bind and cannot meet a stable declared output without extra validation.
For external JSON, build plain arrays and objects and stringify only if the other API needs text.

## Errors and review

At a service boundary, log entity and diagnostic context, then throw a safe user-facing message.
For a recoverable row failure, warn and proceed only when the chosen fallback is intentional.
Never silently return incomplete data or reveal raw exceptions or credentials.

Before sync, verify definition-aligned JSDoc; useful helper and coercion documentation; matching
INFOTABLE metadata, DataShape, AddRow keys, and runtime values; safe empty input and first-row
access; suitable loop choice; constrained dynamic names; CreateValues for DataTable writes; and no
debug writes, implicit globals, legacy for each, hard-coded secrets or endpoints, string defaults
for NUMBER/BOOLEAN/DATETIME, swallowed failures, source mutation during a snapshot pass, or
unconsidered per-row remote calls. Then run twaco sync Acme.Orders.Manager_TS --check and twaco
check.

Avoid implicit globals such as `params`. The legacy form `for each (row in table.rows)` is not a
replacement for `rows.toArray().forEach`; use `source.rows.length`, never `source.length`, and
guard the first row. `getRowCount()` remains valid when a count is the only purpose. For a dynamic
payload, `InfoTableFunctions.Aggregate` produces another table, while `JSON.stringify(payload)`
is only needed by APIs that explicitly request a string.
