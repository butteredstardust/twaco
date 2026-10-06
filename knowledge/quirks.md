# ThingWorx platform quirks (verified live)

Behaviours of a running ThingWorx server that cannot be derived from source or the javadoc,
each verified against a live instance (ThingWorx 9.5 to 10.2). Entities in the examples use
the neutral `Acme.Dashboards` prefix. Read the section
that matches what you are about to do before a live import, a hand-written mashup binding, a
configuration-table change, or a service that introspects metadata or touches JSON. When a
section and the server disagree, test against the server: platform versions differ.

## Import (`/Thingworx/Importer`) needs a session, not an app key

The manual Importer endpoint returns **401/403 for a valid, working app key**,
even though that same key works fine for every other REST/service call
(`Things/{name}`, `Things/{name}/Services/{service}`, etc.). It needs the same
request shape Composer's own upload UI sends:

- `Authorization: Basic <base64(username:password)>` — not an `appKey` header.
- `X-Requested-With: XMLHttpRequest` and a non-empty `X-XSRF-TOKEN` header.
  The token's *value* is not actually checked against a session cookie — a
  fixed placeholder string works — only its *presence* matters.
- Multipart field name must be `file` (singular). `files` is silently wrong.
- Useful query params sent by Composer:
  `purpose=import&usedefaultdataprovider=false&usedefaultqueueprovider=false&WithSubsystems=false&IgnoreBadValueStreamData=false&overwriteConfigurationTableValues=true&overwritePropertyValues=true`.

`twaco import` and `twaco deploy` implement all of this; a twaco profile
(`.twaco/profiles/<name>.toml`) needs `username`/`password` as well as `url`; `twaco doctor`
shows what resolved.

## `GetServiceDefinitions`/`GetServiceDefinition` don't expose usable source info

Calling the base `Metadata`-category services `GetServiceDefinitions(category,
type, dataShape)` or `GetServiceDefinition(name)` on a Thing returns rows where
**`sourceType` and `sourceName` are both `null`** for services defined
directly on that Thing (verified: not just "empty string", actually `null`).
Do not use these fields to distinguish a Thing's own services from ones it
inherited from its ThingTemplate/ThingShapes — that comparison will always be
false and silently filter out everything.

To get only the services (and properties, etc.) actually defined on one
entity, call:

```javascript
const definition = Resources["EntityServices"].ReadEntityDefinitionAsJSON({
    type: "Things",  // STRING - entity collection: "Things", "ThingTemplates", etc.
    name: thingName  // STRING
});
const serviceDefinitions = definition.thingShape.serviceDefinitions; // object keyed by service name
```

This object contains *only* the entity's own services — no inherited-service
filtering needed. Each entry has `parameterDefinitions` (object keyed by
parameter name) and `resultType` (a flat object, with `resultType.aspects.dataShape`
holding the bound DataShape name for an `INFOTABLE` result, when set).

**Don't confuse this with the sibling `effectiveShape` node in the same
response.** `ReadEntityDefinitionAsJSON` returns both `thingShape` and
`effectiveShape`, and `effectiveShape.serviceDefinitions` is the *full* API
surface: the entity's own services plus everything inherited from its
ThingTemplate/ThingShapes *plus* the platform defaults baked into the base
`Thing` Java class itself (e.g. `AddVec3ValueStreamEntry`,
`QueryVec2PropertyHistory`, and the rest of the `StreamEntries`/property-history
services every Thing exposes) — none of which count as "inherited from a
ThingTemplate" in the ThingShape-comparison sense, since they come from the
base class, not a chosen template. If you're eyeballing the raw JSON and
wondering why a "custom services only" filter based on `thingShape` doesn't
show some service you can clearly see in the payload, check which of the two
sibling nodes you're actually looking at first.

## A service's declared result DataShape is metadata only, never enforced

`resultType.aspects.dataShape` on a service definition is a *declaration*. The platform does not
check it against the InfoTable the script actually builds: a service declared as returning
`A_DS` may `CreateInfoTableFromDataShape` an unrelated `B_DS`, return it, and every caller that
reads rows keeps working. Nothing warns, at save time or at runtime.

That makes the declaration a silent lie to anything that discovers services by metadata. A
consumer can select services by the declared shape while another inspects the real InfoTable, so
a drifted declaration can work at runtime but disappear from a compatible-service picker.

Whenever a service's output shape is part of a contract, assert the declaration matches the
script — `twaco call --with-logs` shows it.

## Importer 406 also fires for a DataShape/ThingShape referencing an entity not yet imported

The Project `dependsOn` case above isn't the only way the manual Importer endpoint
returns a bare `406 Not Acceptable` (empty body, no useful detail) for a missing
dependency. The same thing happens for any single entity whose XML references another
entity that doesn't exist on the target server yet:

- A DataShape with an `INFOTABLE` field carrying `aspect.dataShape="Some.Other_DS"`,
  imported before `Some.Other_DS` itself exists on the server.
- A ThingTemplate/Thing with `<ImplementedShape name="Some.Other_TS" type="ThingShape">`,
  imported before `Some.Other_TS` exists.

Both were verified live: importing entities in plain alphabetical order broke here,
because e.g. `Acme.Parent_DS` (which nests `aspect.dataShape="Acme.Child_DS"`)
sorts before `Acme.Child_DS` alphabetically, and a ThingTemplate implementing a
ThingShape sorts before `ThingShapes/` in a naive folder-by-folder import loop.

Import in dependency order, not alphabetical/folder order: **Project first** (see the
next section for why), then **DataShapes** (and among them, anything with a nested
`aspect.dataShape` reference must come after the DataShape it references) **->
ThingShapes -> ThingTemplates -> Things -> Mashups**. Groups/Organizations/StyleThemes/
MediaEntities have no cross-references and can go anywhere after Project. If a single
entity 406s with no other explanation, check what it references by name and import that
first.

## Importing an entity before its Project exists silently reassigns it to `PTCDefaultProject`

An entity export can declare `projectName="Acme.Dashboards"` in its
own XML. That attribute is **not** what actually determines Composer project membership
on import through the manual Importer endpoint -- membership is only assigned correctly
if the named `Project` entity *already exists on the target server* at the moment the
other entity is imported. If it doesn't yet, the platform imports the entity fine (HTTP
200, no warning anywhere) but silently files it under **`PTCDefaultProject`** instead,
discarding the XML's own `projectName` value.

Verified live: importing `Projects/Acme.Dashboards.xml` *last* (a reasonable-looking
choice, since a Project's own dependency graph -- other projects/extensions in
`dependsOn` -- points the other way) left every other imported entity showing up under
`PTCDefaultProject` in Composer, with `GET .../<EntityType>/<name>` confirming
`"projectName": "PTCDefaultProject"` for all of them despite their source XML saying
`Acme.Dashboards`. Re-importing the *same* entity XML again, unchanged, after the
Project entity existed, correctly moved it to `Acme.Dashboards` -- so this is a
one-time binding decided at each entity's *first* import, not something continuously
re-derived from the XML.

Fix/avoidance: import the `Project` entity **first**, before anything else that
declares that `projectName`. If entities already landed under `PTCDefaultProject`
because Project was imported later, re-import each affected entity's unchanged XML
again now that Project exists -- that alone corrects the association, no XML edit
needed.

## New multi-row ConfigurationTables need a real DataShape + a ConfigurationTableDefinition

A `<ConfigurationTable>` written in the common exported form — `dataShapeName=""` with an inline
`<DataShape><FieldDefinitions>...</FieldDefinitions></DataShape>`, and an
empty `<ConfigurationTableDefinitions/>` — **is a valid export format, but
raw XML import silently does not create that table** when importing onto a
Thing that didn't already have it. (It likely only ever gets created that way
through Composer's own "add configuration table with ad hoc fields" UI flow,
which does something import can't replicate.)

What does work reliably for import-created tables: reference a real, separate
top-level `DataShape` entity, and add a matching entry under
`ConfigurationTableDefinitions`:

```xml
<ConfigurationTableDefinitions>
    <ConfigurationTableDefinition category="" dataShapeName="Acme.Example.Settings_DS"
     description="" isHidden="false" isMultiRow="true" name="SettingsTable"
     ordinal="0" source="REST"/>
</ConfigurationTableDefinitions>
...
<ConfigurationTables>
    <ConfigurationTable dataShapeName="Acme.Example.Settings_DS" description=""
     isMultiRow="true" name="SettingsTable" ordinal="0">
        <DataShape><FieldDefinitions>...</FieldDefinitions></DataShape>
        <Rows>...</Rows>
    </ConfigurationTable>
</ConfigurationTables>
```

This matches configuration tables that reference real DataShapes. Prefer this pattern for any
*new* config table meant to survive a
plain import; the inline/no-name style is fine only for tables that already
exist on the target server.

## JSON-typed row values need a `<json>` wrapper, and arrays get auto-boxed

A `JSON`-baseType field's row value must be wrapped in a nested `<json>`
element around the `CDATA`, as exported JSON fields such as `editorSettings` are written:

```xml
<requiredFieldsJson><json><![CDATA[[{"name":"value","baseType":"NUMBER"}]]]></json></requiredFieldsJson>
```

Plain `<requiredFieldsJson><![CDATA[[...]]]></requiredFieldsJson>` (no `<json>`
wrapper) imports without error but the value silently comes back as an empty
object — the content is dropped, not rejected.

Separately: **a top-level JSON array is auto-wrapped by the platform as
`{"array": [...]}`**, both when read back over REST and (per live testing)
inside script. Don't assume `someJsonField` is a bare array or a JSON string;
it can be:

- a plain JS array,
- a Java-bridged object where `Array.isArray()` and direct property access
  silently fail; a naive `Array.isArray(value.array)` check can therefore return `false` and
  produce an empty result,
  or
- the `{"array": [...]}` shape.

One robust normalizer round-trips
through `JSON.parse(JSON.stringify(value))` before checking either shape —
that round-trip reliably converts any of the above into a plain JS
array/object, sidestepping the Java/Rhino interop gap entirely:

```javascript
function toFieldArray(jsonValue) {
    if (!jsonValue) return [];
    let normalized;
    try { normalized = JSON.parse(JSON.stringify(jsonValue)); } catch (err) { return []; }
    if (Array.isArray(normalized)) return normalized;
    if (normalized && Array.isArray(normalized.array)) return normalized.array;
    return [];
}
```

Also do **not** call `JSON.parse()` directly on a JSON-typed field's value in
script and assume it's a string — it usually isn't (already an
object/array-like value), and `JSON.parse()` on a non-string throws.

## A Composer browser-captured REST call is a `Resources["X"].Service(...)` call in script

A `curl` or DevTools capture of a Composer network request such as
`POST /Thingworx/Resources/SearchFunctions/Services/SpotlightSearchV2`, that path
maps directly to a server-side base-service call — do not port it as an HTTP call
from script:

```javascript
Resources["SearchFunctions"].SpotlightSearchV2({ /* same keys as the JSON body */ });
```

The JSON body's keys generally line up with the Java method's declared parameter
names (confirm the exact signature with `twaco javadoc` or the matching public Java API class) — but the
browser payload often carries extra Composer-UI-only keys not in the signature
(e.g. `SpotlightSearchV2`'s captured body includes `excludeFilters`, `context`,
`searchText`, `suppressEntityContext`, none of which are declared parameters).
Drop those; passing only the declared parameters is enough and was verified live.

`SearchFunctions.SpotlightSearchV2` (declared params: `searchExpression`, `tags`,
`types`, `thingTemplates`, `thingShapes`, `aspects`, `excludedAspects`,
`startDate`, `endDate`, `searchDescriptions`, `withPermissions`, `sortBy`,
`isAscending`, `maxItems`, `maxSearchItems`, `projectName`, `namespace`,
`includeInheritedThingShapes`) returns rows shaped like (verified live against a
`{"types":{"items":["Thing"]}}` search):

```json
{
    "name": "Acme.Experiments.TestThing",
    "description": "",
    "type": "Thing",
    "projectName": "Acme.Experiments",
    "namespace": "",
    "parentName": "Things",
    "parentType": "Things",
    "isSystemObject": false,
    "isEditable": true,
    "isExtension": false,
    "isEditableExtensionObject": false,
    "isEditableSystemObject": false,
    "lastModifiedDate": 1784874394272
}
```

`read`/`update`/`delete` (BOOLEAN) only appear when `withPermissions: true`.
`accessModifier` and `deprecated` (JSON) only appear when non-empty. When
projecting this into your own service's DataShape, do not assume `type` varies —
it is constant for whatever single collection you passed in `types.items`.

Before trusting a Resource service's result shape from javadoc alone, prefer a
quick live probe (POST the REST endpoint with `appKey`, print the response) the
way twaco's own client does — javadoc gives parameter
names/types but not always the runtime row shape.

## A Group's `AddMember`/`DeleteMember` take `member`, not `name`

The member services on a `Groups` entity are documented everywhere as taking a member name and a
type, and the parameter is called **`member`**:

```javascript
Groups["Acme.Dashboards.DashboardEditors_UG"].AddMember({
    member: userName,  // STRING - not `name`
    type: "User"       // STRING - singular, "User" or "Group"; "Users" fails
});
```

Getting either wrong fails as a bare `Unable to Invoke Service AddMember on <group>`, HTTP 500,
with no mention of a parameter. The ApplicationLog is one line more specific and still misleading
-- `Invalid Entity Name : null` is what a missing `member` looks like, because the service ran and
looked up a principal named nothing.

Verified live, and worth knowing before writing a guard for each:

- Both are **idempotent**: adding a member twice leaves one, deleting a non-member does nothing.
- Both **throw for a principal that does not exist**, with the same opaque 500. So the useful
  guard is `Users[name]` before the call, not a membership check.
- `GetGroupMembers()` returns the `GroupMember` shape -- `name`, `description`, `type` -- and
  `type` is how a nested group is told from a user. There is no members-that-are-users service.

`ReadEntityDefinitionAsJSON` does not carry service definitions for a Group, and
`GET /Groups/<name>/ServiceDefinitions` returns only names and descriptions. The parameter names
come from `GET /Groups/<name>/Metadata`, whose `serviceDefinitions.<Service>.Inputs` is the real
signature.

## Importing a Group does not wipe its members

Unlike a `Database` Thing's password, which an import blanks every time, a Group's membership
survives re-importing that Group from an XML export whose `<Members>` element is empty: a member
added live is still there afterwards. So group
entities in `Groups/` can ship with no members and the backend bundle will not revoke anyone's
access on the next deploy.

## `SetConfigurationTableRows` replaces the whole row, it does not patch it

The service reads as an update-or-add of the columns you hand it. It is not: a column left out
of the row is **written empty**, not left alone. Verified live on a multi-row table -- writing
`{UID, MashupName, Description}` into a ten-column row emptied the other seven, taking
`RequiredInputType` to `""`, `Order` to `0` and a JSON column to `{}`.

That is the quiet kind of damage. Nothing errors, and a blanked column often means something
permissive rather than nothing: an emptied `RequiredFields` reads as "this card requires no
particular fields", which offers it for every data source on the server.

A correct implementation that changes one field of a configuration row reads the row, merges the
change, and writes every column back. Two more verified details are convenient once this is
understood:

- Rows are matched on the DataShape's **primary key**, so `SetConfigurationTableRows` with an
  unused key adds a row and with an existing one replaces it.
- `DeleteConfigurationTableRows` needs **only** the primary key in its `values` row; the rest of
  the columns can be absent.

Pass `persistent: true` or the change is lost on the next restart.

## Adding a column to a ConfigurationTable takes two imports

A Thing's `<ConfigurationTable>` carries a copy of its DataShape inline, alongside the standalone
DataShape entity of the same name. Widening the column set means changing both -- and importing
them together still does not land the data.

Bundling the DataShape and the Thing into one import updates the **shape** first but can write the
rows against the old column set, so a new column reads back empty from every row. Nothing errors:
`GetConfigurationTable` returns the widened shape, and the value is simply absent. Importing the
identical Thing XML a second time lands the rows in full, because the shape already has the
column by then.

So an added column is a two-pass deployment:

```powershell
twaco deploy --only Acme.Dashboards.<Shape> --only Acme.Dashboards.Manager --apply
twaco deploy --only Acme.Dashboards.Manager --apply
```

Check the values rather than the shape afterwards. The shape is right after the first pass, which
is what makes this easy to declare finished while every row is still missing the column.

**Every entity that declares the table carries its own inline copy.** If a field is added to a
DataShape and a Thing but omitted from an inheriting ThingTemplate's `ConfigurationTables`, the
repository can retain two different column sets. The server regenerates its own copy from the
DataShape, so services and checks can still behave correctly; the drift may appear only when a
live export is compared with the repository. When a `_CT` DataShape gains a field, search the
field name across `Things/` and `ThingTemplates/` and confirm every inline copy lists it.

## A DataTable's actual data rows are never part of its entity XML export

A `DataTable`-templated Thing's exported XML only contains its **schema** (the
`DataThingSettings`/`Settings`/`Indexes` `ConfigurationTables`, including
`accumulatedDataShape`) -- never the table's business data rows. This is different from
a `ConfigurationTable` on an ordinary Thing, which genuinely does ship its rows inline in `<Rows>` and
imports them correctly. A `<Rows>` block hand-added to a `DataTable` Thing's XML is
simply ignored by the schema-only import path -- it does not error, it just has no
effect, which is easy to mistake for success.

DataTable seed data therefore needs a separate backup or an explicit initialization path.
To ship seed data for a new DataTable that still imports as a single, complete,
one-shot unit (no manual post-import step), add a `Subscription` on the DataTable
Thing's own `ThingStart` event that populates the table if empty: query
`maxItems: 1`, and if empty, build values via `me.CreateValues()` +
`me.AddOrUpdateDataTableEntry(...)` per row. `ThingStart` fires once when the Thing
initializes after import (or platform restart), so this self-seeds on a fresh import
with no extra step.

## A script service's `return` inside a nested block silently kills the whole service

A ThingWorx script service body is not compiled as an ordinary JS function. `return
<expr>;` is only valid as the **last statement of the top-level script**, e.g.
`ExampleService`'s `return localizedToken === "???" ? token : localizedToken;`. Using
`return` as an early exit from inside a nested block -- an `if` guard clause, a `for`
loop, etc. -- fails to compile with **`invalid return`**, and the platform then has
**no compiled handler at all** for that service, not just a broken code path.

This failure is silent at import time: the Importer returns `HTTP 200 success`,
and `Resources["EntityServices"].ReadEntityDefinitionAsJSON` still lists the service in
`serviceDefinitions` (its *metadata* imported fine). The only place it surfaces is at
**invocation time**, as a generic, unhelpful error that gives no hint about `return`:

```text
Unable to Invoke Service <Name> on <Thing> : No service handler defined for service <Name> on thing [<Thing>]
```

The real cause is in `ThingworxStorage/logs/ErrorLog.log` (or the portable install's
equivalent, e.g. `ThingWorx-Portable-10-0-Postgres_Blank/ThingworxStorage/logs/`), logged
once at the point the entity's ThingShape/ThingTemplate was (re)loaded -- search for the
service name, not the time of the failed call:

```text
Unable to create service handler for <Name> in <Entity>:ThingShape : invalid return (<Name>#<line>)
```

`ScriptErrorLog.log` has the matching `Execution error in service script [<Name>] ::
invalid return (<Name>#<line>#<col>)` line with an exact line/column.

Do not use a guard-clause-with-early-return pattern in a service script:

```javascript
// Fails to compile -- "invalid return" -- and disables the whole service.
if (!device) {
    logger.warn(...);
    return result;
}
... rest of the logic ...
```

Restructure into nested `if`/`else` so nothing returns from inside a block; let `result`
fall through to the end of the script unchanged when a guard fails:

```javascript
if (!device) {
    logger.warn(...);
} else {
    ... rest of the logic ...
}
```

A **declared function returns freely**, because it compiles as a real function: when the
logic genuinely wants early exits, lift it into a named helper and call it from the top
level. When a newly imported script service throws "No service handler defined" and the
entity definition metadata says the service exists, check the script for an early `return`
inside a block before suspecting anything else.

## DataShapes and Mashups have no delete service — the REST verb works, with query parameters

`Resources/EntityServices` offers `DeleteThing`, `DeleteThingShape`, `DeleteThingTemplate`,
`DeleteMediaEntity`, `DeleteGroup`, `DeleteOrganization`, `DeleteProject`… but **no
`DeleteDataShape` and no `DeleteMashup`**. The obvious fallback,
`DELETE /Thingworx/DataShapes/<name>` with an app key or with Basic auth, answers **HTTP 500**
and an HTML error page, which can make those entity types appear undeletable.

Composer deletes them perfectly well, and a capture of its own request shows the difference:
it repeats `Accept` and `Content-Type` **as query parameters as well as headers**, and sends
`X-Requested-With: XMLHttpRequest`:

```
DELETE /Thingworx/Mashups/<name>?Accept=application%2Fjson&Content-Type=application%2Fjson
Authorization: Basic <user:password>
Accept: application/json
Content-Type: application/json
X-Requested-With: XMLHttpRequest
```

With the query parameters present the same DELETE returns 200 and the entity is gone.
Check that the entity exists first, and confirm afterwards that it is
gone, with a REST GET of `/Thingworx/<Collection>/<Name>`, which answers 404 when it is
(this endpoint can answer 200 without deleting, and so does the Exporter for a missing entity). It authenticates with
username/password, like the importer — the app key is not accepted here
either.

## Re-importing a `Database` Thing wipes its live credentials

A `Database` Thing export should carry an empty `<password></password>` because a real credential
does not belong in git. Importing that entity therefore **overwrites whatever
password is configured on the running server with nothing**, and every DBConnection call
immediately starts failing:

```
org.postgresql.util.PSQLException: The server requested SCRAM-based authentication,
but the password is an empty string.
```

This may not show up during single-entity work but appears when the whole project is imported.
`isConnected` on the Database Thing goes `false`; recovery requires writing the credentials back
through the solution's deployment service and restarting the Thing. The same applies to any
`Database`-templated Thing whose `ConnectionInfo` is exported with a blanked field.

## Import the whole project as one bundle, not entity by entity

Importing a large solution's split entity files one at a time through `/Thingworx/Importer` is
slow and forces the caller to get the dependency order right. ThingWorx's own exporter emits a **single `<Entities>`
document** containing every collection and resolves ordering internally on import;
`twaco bundle` produces the same thing from the split files, and importing that
one file avoids the per-entity overhead.

The bundle used to follow ThingWorx's own export order, which puts `Things` *before*
`ThingTemplates`. That is fine for references, which the platform resolves internally, but not
for a **configuration table whose columns changed**: the Thing's rows arrive while its template
still declares the old columns, and the values land empty — right number of rows, right column
names, nothing in them. Importing the template and then the Thing separately restores the values.

`twaco bundle` orders collections so a definition always precedes what fills it:
DataShapes, ThingShapes, ThingTemplates, Things, Mashups. Verified by rolling the server back to
the old column names and importing the reordered bundle once — all three tables came back
populated. Keep that property if you touch `COLLECTION_ORDER`.

## `baseDataShape` does not survive the Importer endpoint

A `<DataShape baseDataShape="Some.Other_DS" ...>` imports with **HTTP 200** and no warning,
but the resulting entity has `baseDataShape` of `null` and only its own locally declared
`FieldDefinition`s. In a live test, a derived shape imported with two local fields returned only
those two fields from `/DataShapes/<name>/FieldDefinitions`; the existing, previously imported
base shape contributed none. This is not an ordering problem.

**Do not rely on DataShape inheritance through this import path** — declare every field explicitly on
each DataShape, even when that duplicates a base shape's fields. The failure is silent: you
get a valid, queryable, wrong-shaped InfoTable rather than an error.

## Reading InfoTable rows: use `forEach` plus explicit conversion, never a bare indexed loop

The pattern in the `service-code` topic
(`source.rows.toArray().forEach((row) => result.AddRow({ id: String(row.id), ... }))`) is not
just style — deviating from it produces **wrong data, not an error**. In a live test, this loop,

```javascript
const rows = source.rows.toArray();
for (let i = 0; i < rows.length; i++) {
    const row = rows[i];
    result.AddRow({ UID: row.UID, DashboardName: row.DashboardName, ... });  // no conversion
}
```

over a two-row source produced two output rows **both holding the first source row's values**.
Rewriting it as `.forEach` with `String(...)`/`new Date(...)` around every field read fixed it
immediately, same data, same DataShapes, same service. The row objects handed back by
`toArray()` behave like live views rather than snapshots, so a raw field reference can be
resolved later against the wrong row; forcing each value to a primitive at read time is what
makes the copy real.

Symptom to recognize: an aggregation service returns the right *number* of rows with duplicated
content. Check for un-coerced field reads before suspecting the query.

## A DataBinding with two PropertyMaps only applies the first one

`DataBindings` entries can carry several `PropertyMaps`, and Composer produces them that way, but
at runtime only the **first** map is applied when the target is a mashup container or a service.
In a live test, a selected row was bound to a container with two property maps. The first target
updated while the second stayed `undefined` inside the contained mashup. Splitting it into two
bindings, one property map each, fixed it.

**When authoring bindings by hand, write one `PropertyMap` per `DataBinding`.** The symptom is
nasty: some of the values arrive, so the binding looks alive, and the missing ones read as
"the parameter isn't set" rather than as a binding error.

## A contained mashup only receives parameters its container declares

A `mashupcontainer` carries its own `MashupParameters` array, listing the contained mashup's
parameters with a `ParmDef` each. Adding a parameter to the inner mashup is not enough: until the
container declares it too, a binding aimed at it lands nowhere, with no error. Symptom is the
same as every other broken binding here — the value simply reads as unset inside the child.

## A `DynamicThingShapes` data source does nothing until `EntityName` is bound

A mashup data source of `EntityType: "DynamicThingShapes"` names the *ThingShape* in its design-time
`EntityName` (`Acme.Dashboards.Management_TS`). That is only the service list — at runtime the
source has no entity to call until something binds a real Thing into it. The binding is the
non-obvious part, because its target is not a widget:

```json
"SourceArea": "Mashup", "SourceId": "mashup-root",
"PropertyMaps": [{ "SourceProperty": "Manager", "TargetProperty": "value",
                   "TargetPropertyType": "Entity" }],
"TargetArea": "Data", "TargetId": "EntityName",
"TargetSection": "DynamicThingShapes_Acme.Dashboards.Management_TS"
```

Without it **every service on that source silently does nothing** — no request, no error, no log
line. For each mashup, count `Data` sources of this type against bindings whose `TargetId` is
`EntityName`.

## A whole-InfoTable binding uses an empty `SourceProperty`, not `"All"`

Binding a service's entire result to a widget's `Data` — a collection, a dropdown, a grid — is
`SourceDetails: "AllData"` with `SourceProperty: ""`. Writing `"All"` looks right, matches the
label Composer shows in the binding tree, and silently binds nothing. Set `SourceName` to the
service name alongside it, as Composer does.

## `ptcsdropdown` has no `Changed` event, and `SelectedText` is the *value*

Two PTCS dropdown details are readable in
`…/webapps/Thingworx/Common/mashup-common-widgets-shared.js`, which is the authority when the
documentation is vague — search it for `widgetName:"ptcsdropdown"` and read the block above.

- Its events are **`SelectedTextChanged`** and `SelectedItemsChanged`. There is no `Changed`. An
  event handler wired to `Changed` is accepted by the import and simply never fires. (`ptcsdatepicker`
  *does* have `Changed`, so the two widgets do not agree — do not generalise from one to the other.)
- `SelectedText` is declared `src: "selectedValue"`, so it carries the **`ValueField`** value, not
  the `DisplayField` text. With `DisplayField: PeriodDisplayName` and `ValueField: Period` it
  yields `Last7Days`, not `Last 7 Days`. It is also `isBindingTarget: true`, so writing a saved
  value into it to preselect a row works.

## Mashup parameter `<name>Changed` events do not fire for every parameter

Some `<name>Changed` events fire reliably on `mashup-root`, while equivalent parameters written
by a parent mashup's binding may issue no event or service call. Parameters written by a widget
inside the mashup do raise the event.

Reliable pattern instead — the same one the card mashups already use for `propertyName`: an
`expression2` widget with `AutoEvaluate: true`, `FireOnFirstValue: true` and `Expression: "input"`,
the parameter bound to its `input`, and the service invoked from the expression's `Changed` event.

## Collection cell dragging: a drag handle, `CellMoved`, and a build that has it

Drag-and-drop cell reordering arrived with the Collection widget in **10.1** and does not exist
in 10.0.x at all. It is worth knowing what "does not exist" looks like, because a mashup
property bag accepts a property the widget has never heard of: on 10.0.2 the widget object
reported `DragCells: true` while the `<ptcs-collection>` element underneath showed no drag
attribute, the page had zero draggable elements, and `DragActionsData` stayed `undefined`
through a real drag. Nothing errored and nothing logged. When a widget property seems to do
nothing, read the custom element rather than the widget wrapper, and check the build:

```javascript
Object.keys(document.querySelector('ptcs-collection').constructor.properties);
```

```powershell
# what this build's Composer will offer for any widget
python -c "import json;d=json.load(open(r'<install>/apache-tomcat/webapps/Thingworx/Common/locales/en/translation-widgets.json',encoding='utf-8'));c=d['tw']['ptcs-collection-ide'];print(sorted(c['properties']));print(sorted(c['events']))"
# the running platform's real version, whatever the startup banner claims
Get-Content '<install>/apache-tomcat/webapps/Thingworx/META-INF/MANIFEST.MF' | Select-String Version
```

On a build that has it (verified live on 10.1.0 b47), three things decide whether a drag
happens. The first is easy to overlook:

- **A drag starts only from the widget's own drag handle.** The mousedown handler is
  `s.closest("[drag-control]") && this._dragStart(...)`, so a mousedown anywhere else in the
  cell — the card body, its title — is ignored. The collection renders that handle itself, one
  per cell, as `div[part="drag-control"]`, placed according to **`CellActionsPosition`**
  (`"bottom"` puts a full-width grab strip along the bottom edge, showing a move cursor icon).
  It appears only while dragging is enabled, which is why a cell looks inert until then.
- **`DragCells` enables it**, `DragCellsBetweenWidgets` extends it across two collections
  sharing a data shape. Dragging is refused outright when sorting or grouping is on:
  `_isDragAllowed = dragEnable && !_sort && !_group`.
- **The event is `CellMoved`.** There is no `DragActionsDataChanged` — `DragActionsData` is
  declared `readOnly`, `isBindingTarget: false`, `isBindingSource: true`, so it is something to
  bind *from*, never a trigger. `CellMoved` fires with `{sourceIdx, targetIdx}`, zero-based
  indexes into the rows the collection is showing, never identifiers. `CellAdded` and
  `CellRemoved` are its cross-widget counterparts.

A complete implementation binds a reorder toggle to `DragCells`, binds `DragActionsData` into a
move service's JSON input, invokes it from `CellMoved`, and refreshes the rows when it completes.

## Runtime mashup definitions are cached hard in the browser

After importing a changed mashup, a normal reload of `/Thingworx/Runtime/index.html#mashup=...`
can keep serving the previous definition — widget property changes appear to have silently not
imported. Appending a cache-busting query parameter
(`index.html?cb=<timestamp>#mashup=...`) forces the new definition. Verify a mashup edit this way
before concluding the edit itself didn't take.

## A PTCS chart with no `XAxisType` silently draws nothing

`ptcschartwaterfall` and `ptcschartpareto` with `XAxisField` and `YAxisType` set but **no
`XAxisType`** render a correctly-scaled y axis and no bars, x-axis labels, or error. The widget can
still be receiving rows: probing `document.querySelector('ptcs-chart-waterfall').data`
in the runtime showed `[[<x>, [<value>], null, {}], …]`, the widget's own internal form.

Without `XAxisType` the widget cannot build a band scale from the x values, so every bar gets
zero width. Setting `XAxisType: 'label'` (what the working `ptcschartline` cards already had)
made both draw immediately. If a PTCS chart is silently blank, compare its axis-type properties
against a chart of the same family that works before touching the data.

Note also that mashup property changes are cached hard — see the caching section above — so
verify a fix with a cache-busted URL before concluding it didn't work.

## The PTCS autorefresh widget's `RefreshInterval` is in **seconds**, not milliseconds

`autorefreshfunction`'s `RefreshInterval` is a count of seconds — its design-time default of `30`
means half a minute, not 30ms. Any mashup parameter or persisted column bound to that property
must therefore use seconds too. Nothing errors for a value expressed in milliseconds; it simply
produces an unexpectedly long refresh interval.

## A Thing's own subscriptions must go in the `ThingShape` block, not the trailing one

A Thing export has two `<Subscriptions>` elements: one inside `<ThingShape>`, right after
`</ServiceImplementations>`, and one at the end after `</ImplementedShapes>`. A subscription
placed in the trailing element **imports silently and is then absent** — the entity comes back
from `/Thingworx/Things/<name>` with both blocks empty, no error anywhere. Put it in the
`ThingShape` block, which is where the platform's own extensions carry theirs.

## A Timer with no `runAsUser` never runs its handlers

`Timer`'s `Settings` configuration table has `enabled`, `updateRate` and `runAsUser`. With
`enabled: true` and a sane `updateRate` but `runAsUser` empty, the timer reports as enabled and
nothing happens — the subscription has no user context to execute in. `runAsUser` is a
`USERNAME`, not a boolean.

## `QueryNumberPropertyHistory` returns a `value` column and does not honour `oldestFirst`

The result columns are `id`, `timestamp`, `value` — the reading is **not** in a column named
after the property. `oldestFirst: true` also does not order the result; rows come back newest
first regardless, so sort by `timestamp` yourself if order matters.

## `AddNumberValueStreamEntry` is called on the Thing, not on the value stream

Backdating history looks like a job for the value stream, but calling
`AddNumberValueStreamEntry` on the value stream Thing fails with "No value stream found for
effective value stream name for Thing [<the value stream>]". The service takes no source
parameter: the entry is attributed to whatever Thing it runs on, and the value stream is
resolved from that Thing's `valueStream`. Call it on the source Thing.

## Logged properties need a value stream, or the aspect does nothing

`aspect.isLogged="true"` on its own stores nothing. The Thing (or its template) also needs
`valueStream="<some ValueStream thing>"`, otherwise `QueryPropertyHistory` comes back empty and
every history-backed card silently has nothing to plot.

## `const` is function-scoped in the script engine, with two separate consequences

The engine behind ThingWorx services does not give `const` (or `let`) block scope. Two distinct
failures come out of that, and both are silent in a way that sends you looking somewhere else.

**Declaring the same name in two blocks is a compile error.** Adding
`const rows = ...` inside an `if` branch of a function that already declares `const rows` further
down fails the whole service with `TypeError: redeclaration of const rows`. The service is then
not registered at all, and calling it returns

    No service handler defined for service <name> on thing <thing>

which reads like a broken import or a missing ThingShape — the entity XML looks perfectly
correct, because the implementation *is* there; it just never compiled. `ScriptErrorLog.log` in
`ThingworxStorage/logs` has the real message. Check it before re-importing anything.

**A `const` in a loop body binds once.** Every later iteration silently keeps the first
iteration's value — no error at all:

```js
for (let i = 0; i < n; i++) {
    const width = 1 + Math.random();   // same value on every iteration
}
```

This produced a schedule chart whose first interval was correct and whose remaining eleven were
all zero-length, which looks exactly like bad arithmetic. Declare with `let` outside the loop and
assign inside. Note that a `const` inside a `forEach` callback is fine — each call is its own
function scope — which is why the pattern only bites in `for`/`while` bodies.

## NaN in a service result kills the whole service

`Number(undefined)` is `NaN`, and an InfoTable row containing it fails serialisation with

    JSON does not allow non-finite numbers

The service returns a 500 rather than a row with a missing value, so one absent number takes the
entire card down. Use a helper that yields `undefined` (which serialises as null) instead of
coercing with `Number()` when the value may legitimately be absent.

## Never write an unbounded loop in a service

A `while (cursor < end)` whose body could fail to advance — because of the `const`-in-loop
binding above, or a zero-length window — pins a platform thread at 100% CPU. The script timeout
does not necessarily reclaim it, the server stops responding, and recovering it can mean
reprovisioning the instance and its database. Bound every loop with a constant iteration cap in
addition to its logical condition, and cap any parameter that scales the work.

## `AddRow` drops nulls, so you cannot write a NULL through an InfoTable

`infoTable.AddRow({ SomeNumber: null })` does not put a null in the row — the field simply ends
up unset, indistinguishable from never having been supplied. Anything downstream that builds a
statement or payload from the row's present fields will therefore skip the column entirely.

This is why a numeric column cannot be blanked through `PTC.DBConnection`'s `Update`. If you need
a genuine null, the row has to be rewritten rather than updated.

## A ThingShape's services must use `me`, never a hardcoded Thing name

Composer's service editor can generate a hardcoded `Things["Acme.Dashboards.Manager"]` access
when an entity is selected from the dropdown. It works until the shape is used on a second Thing
or the manager is renamed, at which point every instance reads the first Thing's configuration.
Use `me`; the shape is the reusable unit.

## A DATETIME reaches a chart widget already stringified, so date formatting never fires

Bind an InfoTable's DATETIME column to a PTCS chart's `XAxisField` and the widget receives
`"Fri Jul 24 2026 15:53:40 GMT+0300 (Eastern European Summer Time)"` — a string, not a date.
`XAxisDateFormatToken` therefore has nothing to parse and is ignored, `XAxisType: 'date'` changes
nothing, and the axis fills a small card with truncated day names.

Format the label server-side instead and bind the axis to that derived column.

**On a label axis the label is the x key.** Two points sharing a label land on the same position,
so a line folds back on itself — which rules out blanking labels to thin a crowded axis. Every
label has to stay unique, so shorten the *format* rather than dropping labels, and let
`HorizontalAxisLabelsRotation` handle the density.

Useful properties for a chart in a small card, none of which are on by default:

| Property | Why |
|---|---|
| `HideValues: true` | the per-point value labels crowd out the plot itself |
| `HorizontalAxisLabelsRotation: -90` | a card gives a 24-point series ~11px per tick; sloped text needs more horizontal advance than that and gets cut |
| `HorizontalAxisMaxHeight` | defaults to 85, which is a third of a medium card |

## A PTCS chart plots every numeric column, and naming the series field does not stop it

Bind an InfoTable with three columns to a chart's `Data` and it draws a series per column it can
read as a number — including a DATETIME. Setting `DataField` (and `DataField1`) to the one field
you want makes no difference; the extra series is still there. Verified by reading
`document.querySelector('ptcs-chart-bar').data` in the runtime, which came back as
`["03:36", ["2000-01-01T03:36:44.092Z", 60.7], null, {}]` — two values per point.

It is easy to miss because it usually hides. On a chart whose y axis is bounded to the data's own
range, the timestamp series sits far off-scale and never paints. On a **horizontal** bar chart the
value axis runs along x instead, so the extra series may appear as alternating full-width bars.

Give a chart a table with only the columns it should plot, such as a shape containing only
`XAxisField` and `Value`, rather than an internal series that still carries the timestamp.

## The Schedule Chart's nested interval columns are matched by name, in lowercase

Every other PTCS chart takes the column to plot from a widget property (`XAxisField`,
`DataField`, `ValueField`), so the column can be called anything. The Schedule Chart's **nested**
table is the exception: `ResourceField`/`DataField` select the outer lane columns, but the
interval table inside each lane is read by fixed lowercase names — `reason`, `info`, `start`,
`end`, `color`. There is no widget property to point at different ones.

Renaming them to `Reason`/`Start`/`End` for naming consistency makes the Gantt render empty lanes
— no error, just nothing drawn.

## Changing a configuration table's columns *and* its rows needs two imports

Add or rename a column on a configuration table's DataShape and change that table's rows in the
same bundle, and the rows are applied against the column set the server had *before* the import.
Values for a column that did not exist yet come back as the type's default — a BOOLEAN written as
`true` reads as `false`, silently. Numbers behaved (an INTEGER column imported its value), so it
is easy to see the table populate and assume it all landed.

In a live test, newly added BOOLEAN columns were `false` after one import; importing the identical
bundle a second time produced the written values. Collection ordering does not help — the
DataShape is its own entity and imports first.

So when a configuration table's *shape* changes, import twice, and check a value afterwards
rather than assuming. `twaco deploy` runs one import; run it again after a shape change.
Note also that a bare second import blanks the database password like any other
import, so prefer `twaco deploy`, whose deploy step can re-apply it.

## A user's real name and email live in a `UserExtensions` configuration table, not properties

A ThingWorx `User` entity has no `emailAddress` or `fullName` property, and no platform service
returns them. They are rows in a multi-row configuration table called `UserExtensions` on the
User entity itself, one `name`/`value` pair per attribute:

```js
const extensions = Users[name].GetConfigurationTable({ tableName: "UserExtensions" });
extensions.rows.toArray().forEach((row) => {
    // row.name is "firstName" | "lastName" | "fullName" | "emailAddress" | "title" | "city" | ...
});
```

The collection accessor is `Users[...]`, not `Things[...]`. Over REST the same call is
`POST /Thingworx/Users/<name>/Services/GetConfigurationTable` — note the `Users` collection in
the path; the usual `Things/...` form answers `404 Entity Not Found : [Users]`.

**Every one of those rows can exist but be empty on an unconfigured user.** A display name derived
from them is therefore not reliable, while the username always is. Prefer `fullName`, fall back to
`firstName + " " + lastName`, and finally use the username so a persisted display name is never
blank. Keep identity fields based on the username and store the display name separately.

**`UserExtensions` is readable through a service and writable through none.** Every generic
configuration-table setter refuses it by name:

```
Calling the SetConfigurationTableRows service on a User's UserExtensions
configuration table is not supported
```

`SetConfigurationTable` and `SetMultiRowConfigurationTable` answer the same way, and there is no
`SetUserExtensions` alongside them. The route that works is the entity XML: export the user,
fill the `fullName`/`emailAddress` rows of its `<ConfigurationTable name="UserExtensions">`, and
import it back. The export carries `passwordHash` and the hashing parameters, so a round trip
leaves the account's password alone — which is what makes this safe to do to a user that already
exists. Keep such seeding in a script beside the project, never in its entities.

One trap on the way: build the replacement with a slice, not a regex backreference. A `` that
escapes wrong writes control characters into the document, and the Importer answers a malformed
one with a bare `406 Not Acceptable` that names nothing.

## Composer's "Export to File" is not the same document as a source-control export

Composer can produce either format, and they are different enough that an Export-to-File
document cannot be dropped into a source-control repository's collection folders as-is. A
source control export (the zip) is one pretty-printed file per entity, four-space indented, one
attribute per line in alphabetical order, wrapped in `<Entities majorVersion="10"
minorVersion="1" universal="password">`. **Export → *To File* writes every entity into one
document on a single line**, wrapped in an `<Entities build="b47" ... universal="">` header, and
differs in content as well:

- It carries `lastModifiedDate` on every entity, plus an `<Owner>` and a `<ConfigurationChanges>`
  element per entity. A source-control export has none of the four; strip them or every entity
  diffs on bookkeeping.
- `<mashupContent>` is one line of JSON. A repository may store it pretty-printed, and sidecar
  tooling can round-trip it as parsed JSON — so
  reformatting to that spelling is what makes a re-extracted `content.json` agree with the XML.
  Composer's own pretty form (Jackson's `"key" : value`, `[ { ... } ]`) appears in files taken
  from the zip and is equivalent; `twaco sync` compares parsed JSON, not text, so a
  mashup left in either spelling reports no drift.
- A service script's CDATA is written flush left, not indented to its `<code>` element. The
  sidecar sync indents to the element baseline when a script changes, so both forms exist in
  the tree and neither is drift.
- **It can carry a live encrypted database password** in a `Database` Thing's `ConnectionInfo`
  row. Repository exports should keep that field empty — see *Re-importing a `Database` Thing
  wipes its live credentials* — so never adopt that entity from an incoming export unchanged.
- Runtime property values and their timestamps come along on every Thing, so sample Things can
  differ even when nothing was designed.
- **A mashup parameter Composer re-created comes back `STRING`.** Re-adding a parameter in the
  editor loses its base type, so a `LONG` identifier parameter silently
  returns as `STRING` and the service it feeds stops matching a row. It lands in three places
  per mashup — the `<FieldDefinition>`, the `"BaseType"` beside its `"ParameterName"` in
  `mashupContent.UI`, and `SourcePropertyBaseType`/`TargetPropertyBaseType` in every
  `PropertyMap` naming it — and a container caches a **contained** mashup's parameter defs in its
  own `_currentParameterDefs`, which is a fourth place with no visible editor for it.
  Check all four whenever a parameter changes; otherwise the
  regression is invisible until a card draws empty.
- **An export can drop an inherited configuration table.** A ValueStream can come back without
  the `PersistenceProviderCustomSettings` table its template gives it. Adopting that
  entity is a silent downgrade, so compare element counts, not just the parts you changed.

The reliable way to adopt one is to compare semantically first — parse both sides, drop the
four bookkeeping items, compare `mashupContent` as parsed JSON and script text
whitespace-insensitively — and rewrite only the entities that actually moved. Rewriting all of
them reformats the whole tree and buries the handful of real changes.

## Paging a grid: the PTCS pagination widget's contract, and three ways to wire it wrong

`ptcspagination` does not page anything by itself. It holds `PageNumber` (1-based), `PageSize` and
`ResultsNumber`, and the paged grids in the platform's own extensions wire it as two services:

```
pagination.PageNumber -> pagedService.pageToLoad     (NUMBER -> INTEGER)
pagination.PageSize   -> pagedService.size           (NUMBER -> INTEGER)
countService.result   -> pagination.ResultsNumber    (NUMBER)
pagination.PageNumberChanged -> invoke pagedService
```

`ResultsNumber` is the *total*, not the page length: the widget derives the page count from it, so
it needs its own service. A hypothetical pair such as `PageRows(data, pageToLoad, size)` and
`CountRows(data)` can take the
already-fetched InfoTable and are pure transforms — no second database read per page turn.

Three things bite while wiring it:

- **A Data-area event *handler* is addressed by its data source, not by the service.**
  `EventHandlerId` is the source id (`DynamicThingShapes_<ThingShape>`) and `EventHandlerService`
  is the service name. A *trigger* is the opposite: `EventTriggerId` is the service name, with
  `EventTriggerSection` naming the source. Putting the service name in `EventHandlerId` produces
  an event that imports cleanly, shows no error, and never fires.
- **`AddField` reads `baseType` as null unless it is coerced with `String()`.** Cloning an input
  table's shape at runtime — the only way a *generic* pager can return something a grid still has
  columns for — reads `data.dataShape.fields[name].baseType`, which is a Java enum. Passing it
  straight through fails with `Unable To Convert To Field Definition: Invalid Base Type for field
  <name> [null]`, naming the field rather than the cause. `String(...)` both values.
  Passing `data.dataShape` to `CreateInfoTableFromDataShape`'s `dataShapeName` fails differently
  and just as opaquely: `Type not found: [com.thingworx.metadata.DataShapeDefinition@...]`.
- **A `ptcsgrid` column needs `Title`, and its `FormatOptions.FormatString` must be a platform
  pattern.** `Header` is not the key the widget reads, so a column set up with it renders a blank
  heading. For a numeric column `"0"` works; `""` renders the cell empty, and `"0.##"` reaches the
  cell as the number followed by literal `##`.

## A PTCS combo chart plots nothing until its fields are named, and `bar` is the column series

`ptcschartcombo` does not infer its columns the way `ptcschartline` does. Bound only to `Data` it
draws its no-data state, however many rows arrive. It needs `XAxisField`, one `DataField<n>` per
series, and `NumberOfSeries` set to how many are really bound.

`Series<n>Type` is what makes it a *combo*: `"bar"` draws that series as columns and `"line"` as a
line, over a base `ChartType` of `"line"`. `"column"` is not a value it accepts — it is ignored
silently, and the series stays points, which looks like the data being wrong rather than the
property being wrong.

## A grid's auto-generated columns come out alphabetical, and a DATETIME comes out unreadable

Bound to an InfoTable with no `ColumnFormat`, `ptcsgrid` generates its columns from the runtime
DataShape — which reaches script as a map keyed by field *name*. Iterating it yields alphabetical
order, not the order the DataShape declares, so an alert log declared `Device, Timestamp, Event,
Details` renders as `Details, Device, Event, Timestamp`. A hypothetical `PageRows` service can
restore the declared order by sorting on `ordinal` before `AddField`.

The same auto-generated column renders a `DATETIME` as the full JavaScript date string —
`Sat Aug 01 2026 14:36:04 GMT+0300 (Eastern European Summer Time)` — which is the grid's version
of the stringified-DATETIME quirk the charts have. Fixing it needs an explicit `ColumnFormat`
entry with a date renderer for that column, which means the grid stops auto-generating and has to
name every column it will ever show. On a card mashup shared by several data sources that is a
design decision, not a mechanical one.

## `SpotlightSearchV2` takes singular type names, and ignores one it does not recognise

`types: { items: [...] }` wants the **singular** entity type name. `"ThingShape"` and
`"ThingTemplate"` work; the plural collection names used everywhere else in the platform —
`"ThingShapes"`, `"ThingTemplates"` — return **zero rows without an error**, which reads as "this
server has no ThingTemplates" rather than as a mistake.

Worse, a type name the search does not recognise at all is **dropped rather than rejected**, so
the call falls back to searching everything. Verified live on 10.1.0 b47:

| `types.items` | rows |
|---|---|
| `["ThingTemplate"]` | 116 |
| `["ThingTemplates"]` | 0 |
| `["Bogus"]` | 1769 — every entity on the server |

So a typo in a type name fails in whichever direction is least visible: silently empty, or
silently everything. Neither raises. When a filter is built on one of these searches, check the
row count against a known entity before trusting it, and never let a search failure widen a
filter. Return an empty list on failure rather than matching the whole server.

`aspects: { isSystemObject: false }` is also worth a deliberate decision rather than a copy-paste:
it can remove `GenericThing`, which is a legitimate template for a configured filter to name.

## Importing a localization table merges its tokens; it does not replace the table

A `LocalizationTable` export such as `Default` or `fr` is the platform's own entity, and an
import of one carrying only a project's tokens could plausibly wipe every other token in it. It
does not. Verified on ThingWorx 10.1: importing a subset increased the token count without
removing existing tokens.

The reverse follows: an import never removes a token. Tokens from a renamed or retired prefix
stay until `LocalizationTables/<table>/Services/DeleteToken` (`{"name": "<token>"}`) removes
them. `GetTokens` lists what is there.

## Deleting a repository Thing deletes its files

`DELETE /Thingworx/Things/<repository Thing>` also removes
`ThingworxStorage/repository/<Thing name>/` and everything under it. Nothing warns first. A new
repository Thing does not create its folder until something writes to it, and it starts empty.
Copy the files out before deleting, then write them back with the new Thing's `SaveBinary`
(`path`, base64 `content`).

## A major-version extension upgrade waits for a server restart, and so does everything after it

`ExtensionPackageUploader?purpose=import` answers 200 and lists the package, whether or not it
installed. The outcome is in `reportMessage`. Verified on ThingWorx 10.2.0 b75:
uploading `PTC.Base` 10.1.0 over an installed 9.7.0 returned *"A major version change is detected
for extension PTC.Base"* and then *"Extension PTC.Base is queued for installation on the next
server restart"*. Every later upload that depends on it (`PTC.DBConnection` and the rest of
Solutions Common) was queued as well, *"because it depends on extension package(s)
[PTC.Base:10.1.0] also queued for installation on server restart"*.

`GetExtensionPackageList` still shows the old versions until the restart. So an import of a
project that needs the new extension has to wait for it, and a script that uploads and then
imports straight away fails for no visible reason.

## A SQL Command service runs in a transaction, so `CREATE DATABASE` needs a `COMMIT;` in front

A `Database` Thing's `SQLCommand` service executes inside a transaction, and Postgres refuses
`CREATE DATABASE` there: *"CREATE DATABASE cannot run inside a transaction block"*. Prefixing the
statement with `COMMIT;` ends that transaction first and the create goes through. Verified on
ThingWorx 10.2.0 b75, creating `acme_example` from a temporary Thing connected
to the `postgres` database:

```sql
COMMIT; CREATE DATABASE acme_example OWNER acme ENCODING 'UTF8' TEMPLATE template0
```

`CREATE ROLE` has no such restriction, and a `DO $$ ... $$` block makes it idempotent.

Changing that SQL on a Thing whose JDBC password someone typed into Composer: do not re-import
the Thing, which overwrites `ConnectionInfo`. `GET /Thingworx/Things/<name>` returns the entity
as JSON with the password still encrypted; edit
`thingShape.serviceImplementations.<service>.configurationTables.Query.rows[0].sql` and `PUT` the
whole document back. The connection survives, which is what Composer's own save does.

Hand-writing such a Thing for import: `<ResultType>` carries `baseType`, `name` and `ordinal` as
its own attributes. A `<FieldDefinition>` nested inside it imports as a field with no type, and
the Importer answers `406 Not Acceptable` with an empty body. The reason is only in
`ApplicationLog`: *"Invalid Base Type for field []"*.

## After an extension upgrade, DBConnection's managers can be missing from PTC.Base's registry

Symptom: DBConnection reads work, and every write fails with `Thing: does not exist.`, with no name
between the colon and the full stop. `ScriptLog` has the cause: *"No Manager was found for identifier
[PTC.DBConnection.Manager]"*. The write path resolves its configuration Thing through PTC.Base's
global manager registry, gets an empty name back, and calls `Things[""]`.

Verified on ThingWorx 10.2.0 b75 after an extension upgrade. The Things existed
(`PTC.DBConnection.Manager`, `PTC.DBConnection.HistoricalDataManager`,
`PTC.DefaultConfiguration.Manager`), but `PTC.Base.Manager.GetAllGlobalDefaultManagers` listed none
of them. Registering each fixed it:

```
POST /Thingworx/Things/PTC.Base.Manager/Services/AddGlobalManagerConfigurationEntry
{"managerName": "PTC.DBConnection.Manager", "key": "PTC.DBConnection.Manager"}
```

Compare `GetAllGlobalDefaultManagers` against a server that works before deploying a
DBConnection-backed project to a new one.

## SQL can run through a Thing on the `Database` template, with these steps

A Thing on the built-in `Database` ThingTemplate runs SQL through services whose
`ServiceImplementation` has `handlerName="SQLCommand"` (DDL and writes) or `"SQLQuery"` (reads). The SQL
is the `sql` cell of that service's `Query` table row, next to `maxItems` and `timeout`. It works on an
instance with no database client, and it needs these steps, in this order:

1. Import the Thing with a `ConnectionInfo` row (`jDBCConnectionURL`, `jDBCDriverClass`, `userName`,
   `maxConnections`, `connectionValidationString`). **The import blanks `password`**: the same rule that blanks a
   `Database` Thing's credentials on every re-import.
2. Encrypt the password with `Resources["EncryptionServices"].EncryptPropertyValue({ data: password })` (the
   parameter is `data`; the answer's row has `result`) and write it with `SetConfigurationTable` on `ConnectionInfo`.
3. **`RestartThing`.** Until the Thing restarts it keeps its pool's blank password and every call fails with
   `The server requested SCRAM-based authentication, but the password is an empty string`.
4. Call the service. A multi-statement `SQLCommand` is **one transaction**: `CREATE TABLE a (...); SELECT 1/0;`
   fails with `Execute Update failed` and table `a` does not exist afterwards. A statement that cannot run in a
   transaction (`CREATE DATABASE`) needs a leading `COMMIT;`.
5. `SQLQuery` accepts a data-modifying statement unless the connection is read-only. For PostgreSQL add
   `options=-c%20default_transaction_read_only%3Don` to the JDBC URL of the Thing that runs queries; a write is then
   refused by the database.
6. Delete the Thing afterwards: it holds the database credentials.


## The Importer records no change reason

`GetConfigurationChangeHistory` on an entity lists each change with `changeReason`, `user`, `changeAction` and
`timestamp`. An import through `/Importer` is recorded (`CREATE`, then `MODIFY`) with `changeReason` empty, and
the query parameters `reason`, `changeReason`, `Reason`, `changereason` and `comment` are all ignored. Only a
REST write to the entity's own URL takes a reason, so twaco's imports cannot carry one.

## `GET /Things/<name>` XML is not importable; the Exporter's XML is

`Accept: text/xml` on an entity's REST address returns a large `<Thing ...>` document (effective shape, no
`<Entities>` wrapper) that the Importer will not take back. `GET /Exporter/Things/<name>` returns the entity as an
export, which imports as it is: exporting a Thing, deleting it and importing the file brought it back with its
run-time permissions. An entity the Exporter does not have comes back as a 200 with an empty export, so check the
document for the entity before believing it.

## Permissions are read and written as JSON by the `*PermissionsAsJSON` services

Every entity has `Get` and `Set` services `RunTimePermissionsAsJSON`, `DesignTimePermissionsAsJSON` and
`VisibilityPermissionsAsJSON`. A `Set` takes the **text** of the whole JSON that the matching `Get` returned in its
`permissions` parameter (a run-time one is `{"permissions": [...]}`). A visibility principal is an organization or
an organizational unit; a group there is an HTTP 500. `GetDifferencesAsJSON({ otherEntity })` returns
`{"rows": [...]}` of what differs between two entities, and it always counts the name itself.
