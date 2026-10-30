# ThingWorx platform quirks (verified live)

These are behaviours measured on live ThingWorx servers from 9.5 through 10.2. They are not a
substitute for testing the target server. Examples use Acme.Orders names.

## Networks are declarative forests (verified on 10.1)

Network XML stores Connection elements under Connections. An empty from is a root; a Thing has one
parent. Contains and Child are usual but connection type is free text. Import replaces all live
connections, including omitted ones. Relevant services are AddConnection, DeleteConnection,
RemoveFromNetwork, SetConnections, GetNetworkConnections, GetSubNetworkConnections, and
GetChildConnections, with template variants.

The corresponding XML spellings are `<Connections>`, `from=""`, and `Connection`; a tree is not
represented by an implicit containment relationship. Treat an import as a full replacement rather
than a patch.

## Import (Importer) needs a session, not an app key

Importer returns 401 or 403 for a working app key although ordinary REST and service calls accept
it. Send Basic username/password, X-Requested-With XMLHttpRequest, and a non-empty X-XSRF-TOKEN;
only token presence matters, so a fixed placeholder works. Its multipart field is file, not files.
Composer uses purpose=import, usedefaultdataprovider=false, usedefaultqueueprovider=false,
WithSubsystems=false, IgnoreBadValueStreamData=false, overwriteConfigurationTableValues=true, and
overwritePropertyValues=true. twaco import/deploy handle this; profiles need username/password and
url, and doctor reports what resolved.

The endpoint is `/Thingworx/Importer`; ordinary calls such as `Things/{name}` and
`Things/{name}/Services/{service}` do accept `appKey`. The multipart name is exactly `file`.
`Authorization: Basic <base64(username:password)>` and `X-Requested-With: XMLHttpRequest` are
part of the observed request; a present `X-XSRF-TOKEN` is sufficient. A twaco profile
(`.twaco/profiles/<name>.toml`) therefore needs `username` and `password`, not only an app key;
never put its credentials in source or logs.

## GetServiceDefinitions and GetServiceDefinition do not identify source

For services defined directly on a Thing, Metadata-category GetServiceDefinitions and
GetServiceDefinition return sourceType and sourceName as null, not empty strings. They cannot
separate local from inherited services. EntityServices.ReadEntityDefinitionAsJSON exposes local
members at thingShape.serviceDefinitions; definitions are keyed by service name, parameters by
parameter name, and an INFOTABLE result's bound DataShape is resultType.aspects.dataShape.
effectiveShape.serviceDefinitions instead includes local, inherited, and base Thing Java services
such as value-stream and property-history APIs.

Call `Resources["EntityServices"].ReadEntityDefinitionAsJSON` with collection values such as
`Things`, `ThingTemplates`, or `ThingShapes`. In the response, `thingShape` is the direct
definition and `effectiveShape` is the complete callable surface: it includes platform methods
such as `AddVec3ValueStreamEntry`, `QueryVec2PropertyHistory`, and other `StreamEntries` APIs.
The exact response members are `parameterDefinitions` and `resultType`.

## A declared service result DataShape is not enforced

A service may declare one DataShape and return an InfoTable made from another without a save-time
or run-time warning. Metadata consumers can therefore disagree with callers reading rows. Treat
the declaration as a contract and check it against the script; twaco call --with-logs can show it.

## Importer 406 also means a referenced entity is absent

An otherwise unexplained empty-body 406 occurs when an INFOTABLE field references a missing
DataShape or an ImplementedShape names a missing ThingShape. Alphabetical or folder order can
therefore fail. Import Project first, then dependency-ordered DataShapes (nested DataShapes first),
ThingShapes, ThingTemplates, Things, and Mashups. Groups, Organizations, StyleThemes, and
MediaEntities can follow Project in any order.

## Importing before Project silently assigns PTCDefaultProject

projectName XML is honoured only when that Project already exists at the entity's first import.
Otherwise the import succeeds and files the entity under PTCDefaultProject. Re-importing unchanged
XML after Project exists moves it correctly, so import Project first and repair a bad order by
re-importing affected entities.

## New multi-row ConfigurationTables need a DataShape and definition

An inline unnamed DataShape with an empty ConfigurationTableDefinitions element is valid exported
XML but does not create a new table through raw import. For a new importable table, create a
top-level DataShape, name it in ConfigurationTable dataShapeName, and add a matching
ConfigurationTableDefinition. The inline form is safe only when the target table already exists.

## JSON row values need a json element; arrays can be boxed

A JSON ConfigurationTable cell needs a json child around its CDATA. Without it import succeeds but
the cell becomes an empty object. Top-level arrays can return as {array:[...]}, as ordinary arrays,
or as Java-bridged objects for which Array.isArray and property access fail. Normalize through
JSON.parse(JSON.stringify(value)) before testing either array form. Do not JSON.parse a JSON field
as though it were always a string.

## Composer REST captures map to Resources calls

A Composer request to Resources/X/Services/Y is normally Resources["X"].Y(parameters) in a
server-side script, not an HTTP request. Keep only declared service parameters: captures may add
UI-only fields. SearchFunctions.SpotlightSearchV2 accepts searchExpression, tags, types,
thingTemplates, thingShapes, aspects, excludedAspects, startDate, endDate, searchDescriptions,
withPermissions, sortBy, isAscending, maxItems, maxSearchItems, projectName, namespace, and
includeInheritedThingShapes. Its result includes name, description, type, projectName, namespace,
parentName, parentType, editable/system/extension flags, and lastModifiedDate; read/update/delete
appear only with withPermissions, while accessModifier and deprecated appear only when non-empty.
Probe live result shapes because javadoc need not describe runtime rows.

## A Group's `AddMember`/`DeleteMember` takes member, not `name`

To add a group member, use member and singular User or Group, never name or Users. This is the
group-member add contract. Bad input gives opaque HTTP 500;
ApplicationLog says Invalid Entity Name : null when member is missing. Both services are
idempotent but throw for a nonexistent principal, so check Users[name], not membership.
GetGroupMembers returns name, description, and type and has no users-only variant. Group service
metadata is available from GET Groups/name/Metadata; ReadEntityDefinitionAsJSON omits it and
ServiceDefinitions returns only names and descriptions.

## Importing a Group leaves members alone

An empty Members element does not remove live Group members on import. This differs from Database
passwords, which imports blank. Group XML can therefore carry no members without revoking access.

## SetConfigurationTableRows replaces the whole row, it does not patch it

SetConfigurationTableRows clears omitted columns rather than preserving them: string values become
empty, numbers zero, and JSON values empty objects. Read, merge, then write every column. Rows
match the DataShape primary key; a new key adds and an existing key replaces. Deletion needs only
the primary key in values. Use persistent=true or a restart loses the change.

## Adding a ConfigurationTable column takes two imports

The table's inline copy and its DataShape both need the new field. One combined import can update
the shape while writing rows against the old columns, silently omitting new values; import the
Thing again after the shape is present. Every declaring Thing and inheriting ThingTemplate has its
own inline copy. Server regeneration can hide a repository mismatch, so search all copies.

That second pass applies to a `_CT` shape too. `GetConfigurationTable` can show the new schema
after pass one even though a new row value is absent. Check all `Things/` and `ThingTemplates/`
copies rather than trusting an apparently healthy generated server copy.

## DataTable exports never contain business rows

DataTable entity XML contains DataThingSettings, Settings, Indexes, and accumulatedDataShape, not
business data. Hand-added Rows are ignored. Seed a fresh table through a ThingStart subscription:
query one row, and when empty use CreateValues plus AddOrUpdateDataTableEntry.

## Application keys without expiration last one day

CreateApplicationKey defaults to a one-day key on 10.1. Its user parameter is user, not
userNameReference; whitelist is one comma-separated string; expirationDate accepts epoch
milliseconds or ISO time. Import does not change key expiration. Use SetExpirationDate or recreate
it. GetKeyID returns rows[0].KeyID. twaco appkey create always supplies an expiration.

## Service-call language follows the user preference

The caller's language property chooses localization: the first available language in a list wins,
unknown languages fall back to Default. Accept-Language and a language query parameter do nothing.
SetLanguagePreferences changes it on the next call; GetPropertyValues reads language. twaco call
with --as and --lang sets a test user then restores it.

## DataShape import rebuilds accumulatedDataShape

Importing a DataShape merges its fields into every DataTable and Stream accumulatedDataShape;
base DataShape fields do not appear and imported table JSON is ignored. Existing fields retain
ordinal and only baseType follows the DataShape. Removed fields remain isDeleted true, restored
ones regain ordinal, and new fields take later ordinals in case-insensitive name order. JSON is
compact HashMap order. A rename loses old stored values. twaco sync and check manage this; do not
edit it by hand.

## A nested return invalidates an entire script service

Only the final top-level script statement may return. A return inside an if or loop gives invalid
return, leaves metadata import successful, but creates no handler; invocation says no service
handler defined. ErrorLog and ScriptErrorLog (under `ThingworxStorage/logs`) contain the line and
column, logged when the shape or template was reloaded, so search them for the service name rather
than the time of the failed call. Use
if/else and fall through instead of guard-returning from a block.

## DataShapes and Mashups delete through REST

They lack delete services but DELETE REST works with required query parameters. Check the target
and use the REST verb rather than assuming absence of a service prevents deletion.

## Re-importing a Database Thing clears its password

Database import overwrites ConnectionInfo without retaining its live credential. Put the password
back through the supported encrypted configuration path rather than treating import as harmless.

## Deploy a project bundle, not arbitrary entity order

Import dependency order and Project membership rules mean a complete project bundle is safer than
individual alphabetical XML imports.

## baseDataShape does not survive Importer

The Importer drops a DataShape baseDataShape relationship. Preserve inherited fields explicitly or
check the target after import.

## Iterate InfoTables with forEach and conversion

Prefer rows.toArray().forEach and convert Java-backed values deliberately. Bare indexed iteration
can hit engine interop behaviour.

## A DataBinding applies only its first PropertyMap

When a DataBinding carries two PropertyMaps, the platform uses the first. Use separate bindings.

## Contained mashups receive declared parameters only

A child parameter reaches the contained mashup only if its container declares it.

## DynamicThingShapes needs EntityName

The DynamicThingShapes data source does nothing until EntityName is bound.

## Whole InfoTable binding has an empty SourceProperty

Bind an entire InfoTable with an empty SourceProperty, not the string All.

## ptcsdropdown has no Changed event

ptcsdropdown lacks Changed, and SelectedText is the value rather than display text.

## Mashup nameChanged does not cover every parameter

Generated parameter Changed events are not reliable for every mashup parameter; verify the actual
event before building a reaction around it.

## Collection dragging needs all three pieces

Cell dragging needs a drag handle, CellMoved, and a platform build that supplies it.

## Browser mashup definitions are strongly cached

Runtime mashup edits can remain cached in the browser. Refresh or clear the appropriate cache
before diagnosing a server-side change as ineffective.

## A PTCS chart without XAxisType draws nothing

Set XAxisType as well as data; an omitted XAxisType can silently produce no chart.

## PTCS autorefresh uses seconds

RefreshInterval is seconds, not milliseconds.

## Thing subscriptions belong inside ThingShape

A Thing's own subscriptions go in its ThingShape block, not the trailing subscription location.

## Timers need runAsUser

A Timer without runAsUser does not run handlers.

## QueryNumberPropertyHistory ignores oldestFirst

It returns a value column and does not honour oldestFirst.

## AddNumberValueStreamEntry is a Thing service

Call AddNumberValueStreamEntry on the Thing, not the value stream.

## Logged properties require a value stream

The logging aspect has no effect without a configured value stream.

## const is function-scoped in the script engine

ThingWorx's engine scopes const like function scope; avoid relying on block-scoped JavaScript
semantics and account for the resulting duplicate-declaration and loop-capture consequences.

## NaN in a result fails the service

Returning NaN kills the whole service. Validate numeric conversions.

## Service loops must be bounded

An unbounded loop can exhaust a server worker. Set a limit or a guaranteed termination condition.

## AddRow cannot write NULL

AddRow drops null values; it cannot persist SQL-style NULL through an InfoTable.

## ThingShape services use me

Use me in a ThingShape service. A hard-coded Thing name breaks inherited instances.

## Charts receive DATETIME as text

Chart widgets receive DATETIME already stringified, so their date formatter does not run. Format
or adapt the field before binding.

## PTCS charts plot every numeric column

They plot all numeric columns; naming a series field alone does not suppress the rest.

## Schedule Chart interval fields are lowercase name matches

Its nested interval columns match by lowercase field name.

## Configuration table columns and rows need separate passes

Changing both schema and values needs the same two-pass import used for adding a column.

## UserExtensions holds a user's name and email

A User's real name and email are in the UserExtensions ConfigurationTable, not ordinary properties.

## Composer file export is not source-control export

Composer Export to File produces a different document from source-control export. Use the
importable export format for repository files.

## Paging a grid has a specific pagination contract

ptcspagination holds one-based PageNumber, PageSize, and total ResultsNumber. Bind PageNumber to
pageToLoad, PageSize to size, ResultsNumber to a count result, and PageNumberChanged to the paged
service. A handler identifies its data source in EventHandlerId and its service in
EventHandlerService; a trigger reverses that relationship. AddField needs String(baseType) for
Java enum values, and data.dataShape is not a CreateInfoTableFromDataShape name. ptcsgrid uses
Title, not Header; numeric format 0 works, empty format hides values, and 0.## prints literal ##.

## PTCS combo charts need named fields

ptcschartcombo needs XAxisField, DataField entries, and the actual NumberOfSeries; Data alone
shows no data. With ChartType line, SeriesType bar makes columns and line makes lines. column is
silently ignored.

## Auto-generated grid columns are alphabetical

ptcsgrid iterates runtime DataShape maps by field name, not declared ordinal. Sort by ordinal
before AddField when order matters. Its auto DATETIME renderer shows a full JavaScript date string;
an explicit date ColumnFormat fixes it but requires every possible column to be named.

## SpotlightSearchV2 type filters are singular

Use singular ThingShape and ThingTemplate. Plural names return zero rows, while an unknown type is
dropped and can search everything. Check counts and fail closed. isSystemObject=false can exclude
GenericThing.

## SpotlightSearchV2 search text is a pattern

searchExpression without wildcards is a case-insensitive whole-name match. Wrap user text in
asterisks unless it already has one; a match can include description text. Default maxItems silently
stops at 500, while maxSearchItems limits scanning unpredictably. Ask for one more row than shown
and leave maxSearchItems alone. A missing projectName also returns no rows.

## Localization imports merge tokens

New tables take file metadata and default to PTCDefaultProject without projectName. Existing tables
report partial-success, overwrite supplied token value/usage/context, never remove other tokens,
and trim CDATA whitespace. AddTokenAnnotated requires Default to contain a token; import does not.
DeleteToken returns 404 when absent. REST deletion needs Content-Type. Exporter XML is one long
line, not a commit-ready source file.

## Deleting a repository Thing deletes its files

DELETE of a repository Thing removes its storage directory recursively. New repositories get no
folder until the first write. Copy data first; SaveBinary can restore it.

## Major extension upgrades wait for restart

ExtensionPackageUploader can return 200 while reportMessage says a major version is queued for the
next restart. Dependent uploads queue too, and GetExtensionPackageList keeps old versions until
restart. Do not import a project requiring them until then.

## SQLCommand runs in a transaction

PostgreSQL CREATE DATABASE fails inside SQLCommand's transaction. Prefix it with COMMIT;. CREATE
ROLE does not need that, and a DO block can make it idempotent. To retain a Composer-entered JDBC
password while editing SQL, GET the Thing JSON, edit the service Query row, and PUT it back rather
than importing. ResultType attributes carry baseType, name, and ordinal; nesting FieldDefinition
there causes empty-body 406 and Invalid Base Type in ApplicationLog.

## DBConnection managers may be absent after upgrade

If reads work but writes say Thing does not exist with no name, ScriptLog may report that no
PTC.DBConnection.Manager exists in the global registry. Compare
PTC.Base.Manager.GetAllGlobalDefaultManagers with a healthy server and register required managers
through AddGlobalManagerConfigurationEntry.

## A Database-template Thing can execute SQL

SQLCommand and SQLQuery handlers use a service Query table containing sql, maxItems, and timeout.
Import ConnectionInfo first; import blanks password. Encrypt it with
EncryptionServices.EncryptPropertyValue(data), write the result through SetConfigurationTable, and
RestartThing before calls. SQLCommand is one transaction, so failed later statements roll back
earlier ones and CREATE DATABASE needs COMMIT. SQLQuery can modify data unless JDBC is read-only;
PostgreSQL options=-c default_transaction_read_only=on prevents writes. Delete the credential
holding Thing afterwards.

## Importer records no change reason

Importer change history entries have empty changeReason; reason, changeReason, Reason,
changereason, and comment query arguments are ignored. Only an entity REST write accepts a reason.

## REST entity XML is not Exporter XML

GET Things/name with Accept text/xml returns an effective-shape document without an Entities wrapper
and cannot be imported. Exporter output can be imported, including runtime permissions. An unknown
Exporter entity may return HTTP 200 with an empty export, so verify the entity is present.

## Permissions use the PermissionsAsJSON services

Get and Set RunTimePermissionsAsJSON, DesignTimePermissionsAsJSON, and VisibilityPermissionsAsJSON
exchange the complete permissions JSON as text. Visibility principals are Organizations or
OrganizationalUnits, not Groups. GetDifferencesAsJSON returns rows and counts the entity name.

## Menu bars require a root menu row

ptcsmenubar needs a Menu root with menuId and child rows whose parentMenuId references it; items
use Mashup linkType/linkTarget and a mashup linkDestination. Flat rows silently render no bar.

## Responsive layout sizing is directional

A lone widget fills a row container's height but only a column container's width. A menu bar in a
column sidebar remains 68px; stock masters use rows.

## Composer clears responsive-container margins

Although the default Margin is 5, Composer normally clears it. Retaining 5 makes fixed bands
10px short and can hide a one-pixel divider; leaf widgets retain it.

## Mashup import reorders data

Imported ParameterDefinitions sort case-insensitively by name and root UI.Properties sort by key.
Appending without that order creates read-back drift.

## Parameter definitions have several copies

Containers store MashupParameters. Navigation functions store them too and also retain
_currentParameterDefs with bindingDirection. Missing any copy makes bindings silently do nothing;
Composer can leave bindings/events after a removed call. twaco mashup validate finds them.

## Contained mashups signal with a timestamp parameter

Contained mashups have no custom events. Use a BOTH parameter set to Date.now from the child, bind
the container parameter to an events router, and react to Changed in the parent.
`twaco mashup event add` writes both halves.

## Persistent property values live on every Thing

Each Thing export holds every persistent own or inherited property value, including type defaults.
Adding template properties affects all instances and removing a shape removes values. Exports group
them by origin in hash order. Definition changes re-stamp import time and GOOD quality, so compare
without timestamp/quality. A persistent INFOTABLE value is an empty infoTable of its DataShape's
field definitions, and every import re-stamps it; compared without the stamp it matches source
(10.2, 2026-10-30).

## Additional import, entity, and REST observations

For a missing dependency, the response is `406 Not Acceptable`. Nested shapes commonly carry
`aspect.dataShape="Acme.Child_DS"`; an implemented shape is written as
`<ImplementedShape name="Some.Other_TS" type="ThingShape">`. Both must exist before their
consumer. The incoming Project must also be present first: otherwise `projectName="Acme.Orders"`
becomes `"projectName": "PTCDefaultProject"`. A project bundle uses `<Entities>` and should retain
the definition-before-user order recorded by `COLLECTION_ORDER`.

An imported DataShape with `baseDataShape="Some.Other_DS"` is accepted but loses that relation.
Call `/DataShapes/<name>/FieldDefinitions` after import and explicitly repeat inherited fields.
An entity REST read is different from an export: `GET /Things/<name>` with `Accept: text/xml`
returns a `<Thing ...>` effective document, whereas `GET /Exporter/Things/<name>` is the form that
can return through the importer. Test the export contains an entity; a missing export can still be
an `HTTP 200 success` response.

DataShapes and Mashups lack `DeleteDataShape` and `DeleteMashup` services. For their REST delete,
the successful Composer-shaped request includes `DELETE /Thingworx/DataShapes/<name>`, the query
parameters `Accept: application/json` and `Content-Type: application/json`, plus
`X-Requested-With: XMLHttpRequest`; Basic authentication is required. A post-delete GET to
`/Thingworx/<Collection>/<Name>` is the useful confirmation. Similarly, a repository deletion is
`DELETE /Thingworx/Things/<repository Thing>`; save its files before that irreversible action.

An ordinary Group import does not remove `<Members>`. Conversely, Database imports replace the
live connection password with `<password></password>`. A `Database` service uses
`handlerName="SQLCommand"` or `"SQLQuery"`; its Query row contains `SQLQuery`, `maxItems`, and
`timeout`. Encrypt a replacement using `Resources["EncryptionServices"].EncryptPropertyValue`,
then write `ConnectionInfo` and `RestartThing`. Its connection fields include `jDBCConnectionURL`,
`jDBCDriverClass`, `maxConnections`, and `connectionValidationString`. A missing password reports
the SCRAM empty-string failure. To make PostgreSQL reads truly read-only, add
`options=-c%20default_transaction_read_only%3Don` to the JDBC URL.

`SQLCommand` is one transaction: `CREATE TABLE a (...); SELECT 1/0;` rolls back the table and
reports `Execute Update failed`. `CREATE DATABASE` needs `COMMIT;` before it; `CREATE ROLE` does
not, and an idempotent `DO $$ ... $$` block is useful. The encrypted-password-preserving edit is a
JSON GET/PUT, not an import. A `<ResultType>` has its own `BaseType`, name, and ordinal; nesting a
`<FieldDefinition>` under it gives the otherwise opaque 406.

Importer-created history has `changeAction` but an empty `changeReason`: query parameters named
`reason`, `changeReason`, `Reason`, `changereason`, and `comment` are ignored. Permissions instead
go through `*PermissionsAsJSON`: `GetDifferencesAsJSON({ otherEntity })` yields `{"rows": [...]}`;
the Set service receives the complete `{"permissions": [...]}` text. Visibility accepts an
organization or organizational unit, not a Group.

## Additional configuration, JSON, and service-engine observations

The importable configuration form has both `<ConfigurationTable>` and
`<ConfigurationTableDefinitions/>`; a new multi-row table needs a named top-level DataShape and a
definition. A JSON cell is `<json>`-wrapped, for example
`<requiredFieldsJson><![CDATA[[...]]></requiredFieldsJson>` becomes the nested `json` form. JSON
may arrive as `{"array": [...]}`. Use `JSON.stringify(payload)` only when a downstream endpoint
requires text, and normalize bridged values before `Array.isArray()` or `JSON.parse()`.

`SetConfigurationTableRows` uses the primary key and replaces a complete row. Missing fields such
as `RequiredFields` are cleared; `DeleteConfigurationTableRows` requires only the key and
`persistent: true` survives restart. Schema-and-row changes need two passes: the first can produce
`false` for a newly written BOOLEAN value. An individual inline table copy can be found beneath
`Things/...` or `ThingTemplates/`.

DataTable entity XML has `DataThingSettings`, `Indexes`, and `accumulatedDataShape`, not business
Rows. DataShape import rebuilds `accumulatedDataShape`; an old column may remain `isDeleted: true`
and a restored one changes back to `isDeleted: false`. A DataShape row reflects `aspect.dataShape`
and `aspect.isLogged="true"` only logs when the Thing also has
`valueStream="<some ValueStream thing>"`.

Service definition metadata is not enforcement: an `A_DS` declaration can return `B_DS`. A
nested service `return` creates `invalid return`, leaving the definition visible but no handler.
The diagnostic is `ScriptErrorLog.log` and includes `Execution error in service script`; restructure
a `guard clause, a` return into top-level fall-through or a helper function. The engine also makes
`const rows = ...` function-scoped: duplicate declaration becomes
`TypeError: redeclaration of const rows`, while a `const` in `while (cursor < end)` can keep the
first loop value. A bound on every service loop is mandatory.

`Number(undefined)` yields NaN, which fails result serialization; `infoTable.AddRow({ SomeNumber: null })`
omits the value instead of persisting NULL. A reusable ThingShape must call `me.AddOrUpdateDataTableEntry(...)`,
not a fixed Thing. For InfoTables, conversion such as `String(...)` and `new Date(...)` inside
`forEach` avoids Java-backed row views; an unconverted indexed copy can repeat the first row.

## Additional mashup binding and widget observations

For a full table binding, use `SourceDetails: "AllData"`, `SourceProperty: ""`, and `SourceName`.
A runtime-selected source is `EntityType: "DynamicThingShapes"`; its section is named
`DynamicThingShapes_<ThingShape>` and needs a binding with `TargetId` `EntityName`. A contained
`mashupcontainer` requires each `ParmDef`; the container's `parameterDefinitions` and
`mashupContent.UI` copies must agree. Bindings have `isBindingSource: true` and
`isBindingTarget: true` or `isBindingTarget: false` according to direction. One DataBindings item
must contain one PropertyMap.

The dropdown events are `SelectedTextChanged` and `SelectedItemsChanged`, not Changed;
`widgetName:"ptcsdropdown"` exposes `SelectedText` from `src: "selectedValue"`. With
`DisplayField: PeriodDisplayName` and `ValueField: Period`, it returns `Last7Days`, not
`Last 7 Days`. For unreliable parameter events, an `expression2` configured with
`AutoEvaluate: true`, `FireOnFirstValue: true`, and `Expression: "input"` is the reliable bridge.

Collection reordering needs `DragCells: true`, a `div[part="drag-control"]`, and `CellMoved`.
`CellActionsPosition` can be `"bottom"`; `DragCellsBetweenWidgets` enables cross-collection work.
It is disabled by `_isDragAllowed = dragEnable && !_sort && !_group`. `DragActionsData` is
readOnly and only a source; the event supplies `{sourceIdx, targetIdx}`. `CellAdded` and
`CellRemoved` describe cross-widget changes. Support begins in 10.1, not 10.0.

Cache-bust a changed runtime definition with `index.html?cb=<timestamp>#mashup=...`; a normal
`/Thingworx/Runtime/index.html#mashup=...` reload can retain an old definition. Import also sorts
`parameterDefinitions` and root `mashupContent.UI` property maps. Navigation and container copies
include `MashupParameters`, `_currentParameterDefs`, and `bindingDirection`; missing one makes a
binding disappear without an error. A child emits through a BOTH parameter, often `Date.now()`,
rather than a custom event.

Charts require `XAxisType: 'label'` when a categorical axis is needed; absent `XAxisType` can
leave a waterfall blank. `XAxisType: 'date'` cannot repair a stringified DATETIME. Use
`XAxisDateFormatToken` only on appropriate values. Small charts often need `HideValues: true`,
`HorizontalAxisLabelsRotation: -90`, and `HorizontalAxisMaxHeight`. A data point may expose both
the timestamp and numeric series, so pass only plot columns. Schedule Chart inner fields are exact
lowercase `reason`, `info`, `start`, `end`, and `color`; outer selectors are `ResourceField` and
`DataField`.

For `ptcschartcombo`, set `XAxisField`, every `DataField<n>`, `NumberOfSeries`, and
`Series<n>Type`. `"bar"` is the column series and `"line"` the line; `"column"` is ignored.
For a generated `ptcsgrid`, `Title` works while `Header` does not. Its formatting is
`FormatOptions.FormatString`: `"0"` is valid, `""` makes the cell empty, and `"0.##"` prints
the hashes. Sort fields by `ordinal` before `AddField` if column order matters.

## Additional search, localization, identity, and platform observations

`SpotlightSearchV2` receives `types: { items: [...] }`, with singular `"ThingShape"` and
`"ThingTemplate"`; `"ThingShapes"` and `"ThingTemplates"` silently return no rows, while
`["Bogus"]` can broaden to every item. Use `aspects: { isSystemObject: false }` deliberately.
Search `"Card"` is a whole match, `"*Card*"` is a pattern, and `"*_DS"` catches a suffix.
Default results stop at 500; `maxItems` raises that ceiling and `maxSearchItems` limits scanning.
Check `Projects/<name>` when a `projectName` filter is empty.

Localization imports merge tokens. `AddTokenAnnotated` has a different Default-token rule;
`DeleteToken {name}` removes one. Deleting a table is
`DELETE /Thingworx/LocalizationTables/<table>` and needs Content-Type. A user profile's
`UserExtensions` holds `firstName`, `lastName`, `fullName`, and `emailAddress`; obtain it through
`Users[...].GetConfigurationTable` or
`POST /Thingworx/Users/<name>/Services/GetConfigurationTable`, never `Things[... ]`.
`SetUserExtensions` does not exist, and generic setters including `SetMultiRowConfigurationTable`
refuse it. Use an entity XML import only for controlled user setup.

An extension upload via `ExtensionPackageUploader?purpose=import` requires reading
`reportMessage`: HTTP success can mean queued until restart. `GetExtensionPackageList` remains old
until then. After an upgrade, an empty manager lookup can produce `Thing: does not exist.`; compare
the registry and add a `managerName`/`key` entry through the manager configuration service.
Timers require `enabled: true`, a sane `updateRate`, and `runAsUser`. A Thing subscription belongs
inside `<ThingShape>` after `</ServiceImplementations>`, not a later `<Subscriptions>` block.

## Exact contracts and failure signatures

`GetGroupMembers()` returns the member rows. Its signature comes from
`GET /Groups/<name>/Metadata`; `GET /Groups/<name>/ServiceDefinitions` is only descriptive, at
`serviceDefinitions.<Service>.Inputs`. Supplying the wrong `AddMember` arguments gives
`Unable to Invoke Service AddMember on <group>`; a missing user can surface as
`User [null] does not exist`. The expected base values include `STRING`, `USERNAME`, and `LONG`.

Application-key creation is `EntityServices.CreateApplicationKey`; recover an id through
`ApplicationKeys/<name>.GetKeyID`. User language is stored on the User; inspect it through
`Users/<name>/Services/GetPropertyValues`. A test call may use
`twaco call <target> <service> --as <test user> --lang de`; language preferences such as `fr,de`
win over `?language=`. Localization fallback may be written as
`return localizedToken === "???" ? token : localizedToken;`.

The REST mapping rule is also useful for a generic capture: `Resources["X"].Service(...)` is the
script equivalent, while `Resources/EntityServices` names the platform resource collection.
Composer-only request members such as `excludeFilters`, `searchText`, and
`suppressEntityContext` should not be invented as service inputs. Verify server signatures with
`twaco javadoc`, and use `curl` only for an external diagnostic, never from a service.

An import's Project graph includes `dependsOn`. Importing an entity that needs another
project's entity (a Thing implementing its ThingShape, for one) adds that project to the
entity's own Project `dependsOn` on the server; re-importing the Project alone keeps exactly what
its file declares, so a Project file that names every project its entities need is read back as
sent. The bundle command is `twaco bundle`, deployment
is `twaco deploy`, health inspection is `twaco doctor`, localization is `twaco localization`, and
search is `twaco search`. A generated table name uses `dataShapeName=""` only in the inline export
form. Settings sometimes contain JSON fields named `editorSettings` or `someJsonField`; retain
their wrapper rather than treating them as text.

The exact REST form for a live Thing is `GET /Thingworx/Things/<name>`. A generic entity check is
`GET .../<EntityType>/<name>`. Error history is retrieved by `GetConfigurationChangeHistory` and
uses actions including `MODIFY`. Entity deletion services available on other collections include
`DeleteThing`, `DeleteThingShape`, `DeleteThingTemplate`, `DeleteGroup`, `DeleteOrganization`,
`DeleteProject`, and `DeleteMediaEntity`.

Persistent state is written in `<ThingProperties>`; default DATETIME is
`1970-01-01T00:00:00.000Z`. A source-control export may contain `Rows`, but a DataTable's business
data still does not. Composer's distinct one-line format has `<Entities build="b47" ... universal="">`,
`<Owner>`, and `<ConfigurationChanges>`; source-control mashup JSON may be in `<mashupContent>`
or a sidecar `content.json`, with whitespace such as `"key" : value` or `[ { ... } ]` semantically
equivalent. Service source resides in `<code>`.

The complete UserExtensions form is `<ConfigurationTable name="UserExtensions">`; use
`fullName`, then `firstName + " " + lastName`, then username for a display label. Entity XML may
include `passwordHash`, so only use the controlled pathway already described. A REST call sent to
the wrong collection returns `404 Entity Not Found : [Users]`.

For chart diagnosis, inspect `document.querySelector('ptcs-chart-waterfall').data` or
`document.querySelector('ptcs-chart-bar').data`; the latter can look like
`["03:36", ["2000-01-01T03:36:44.092Z", 60.7], null, {}]`. `ptcschartwaterfall`,
`ptcschartpareto`, and `ptcschartline` have different defaults; a waterfall needs its `YAxisType`
and axis type. The refresh widget is `autorefreshfunction`. Collection implementation is
`<ptcs-collection>`; its mousedown rule is
`s.closest("[drag-control]") && this._dragStart(...)`. `DragActionsDataChanged` is not its event.

The pagination helper may be `PageRows(data, pageToLoad, size)` with
`CountRows(data)`. `EventTriggerId` holds a service name while `EventTriggerSection` identifies
the source, the opposite of an event handler. To clone a dynamic table shape, read
`data.dataShape.fields[name].baseType` and coerce it with `String()` before AddField. A one-row
limit is `maxItems: 1`.

Menu data is a `MenuEntry` tree with a root such as `topMenu`. A flex container has
`ResponsiveLayout`; persisted property names may include `propertyName`. Search type input is
`types.items`, for example `{"types":{"items":["Thing"]}}`; exact examples of valid values are
`["ThingTemplate"]` and invalid plural ` ["ThingTemplates"]`. A real permission projection needs
`withPermissions: true`.

History services use `QueryPropertyHistory`; `oldestFirst: true` is not reliable for numeric
history. DataTable values originate with `me.CreateValues()`. Database availability can be checked
with `isConnected`. For post-upgrade registry repairs, the related built-ins can include
`PTC.DBConnection.HistoricalDataManager` and `PTC.DefaultConfiguration.Manager`; use their actual
installed names rather than a project-specific placeholder.

## Import and inspection checklist

Before importing an entity, establish whether every named Project, DataShape, ThingShape,
ThingTemplate, and extension is already installed. A successful HTTP status is not proof that
membership, an inline configuration row, a script handler, localization contents, or a stored
credential survived. In particular, Project assignment happens on first import; inline
ConfigurationTable schemas can lag their rows; a script can have metadata without a compiled
handler; Group membership is deliberately retained; and Database passwords are deliberately
discarded. These outcomes require a read-back of the relevant service or entity, not only an
Importer response.

For metadata discovery, distinguish an entity's own thingShape from effectiveShape, and distinguish
a declared INFOTABLE DataShape from the table produced by a script. For data discovery, distinguish
schema-bearing XML from DataTable business rows, ConfigurationTable rows from property values, and
Exporter's importable document from a REST representation. For JSON, distinguish text from a
runtime JSON value, a normal array from a boxed array, and a Java-backed value from an ordinary
JavaScript object. Each distinction has produced a silent successful-looking operation on a live
server.

For mashups, check the actual widget contract rather than inferring it from a nearby widget:
pagination needs a total as well as the current page; combo charts name their axis and series;
menu bars need a tree root; DynamicThingShapes needs EntityName; and a contained mashup needs every
parameter definition copied into the places that consume it. Composer may normalize order and
clear layout defaults during save, while browsers can retain a cached runtime definition. Compare
the parsed mashup form and test the interaction in the target build.

For server-side services, favour a named contract, bounded work, validated conversions, and a
read-then-write flow for configuration rows. The platform may accept mismatched result metadata,
drop nulls, reject a nested return only after import, and hide extension or database readiness
until a call is made. When a failure names no useful entity, inspect the application and script
logs around the last reload or upgrade. When the operation affects credentials, permissions, a
repository Thing, or an extension, use the narrowest supported endpoint and retain a recovery
path before changing state.

The order of a live investigation matters as much as import order. First identify the persisted
contract: entity collection, project, referenced DataShapes and templates, permission scope, and
whether values are configuration, ordinary persistent properties, DataTable rows, or external
repository data. Then inspect the narrow service that owns that contract. A broad metadata response
can include inherited and base capabilities that a direct entity definition does not own; a broad
search can silently widen or empty a filter when its type spelling is wrong; and a broad XML
document can be an effective REST view rather than an import file. The platform frequently reports
success for these distinctions, which makes a deliberate read-back more useful than a retry.

For deployment, separate changes that the server stages internally. A Project must exist before
its members. A referenced shape must exist before the member that names it. A configuration schema
must exist before rows that use a new column. A major extension must be restarted before dependent
extensions or projects can rely on it. A Database Thing needs its encrypted password written and
its pool restarted before a connection can work. These are independent sequencing rules; satisfying
one does not establish the others.

For review, retain the evidence that makes a platform observation actionable: target version,
operation order, response status, read-back state, and the relevant log line when a handler or
manager is involved. Do not add credentials, request bodies, server URLs, or private entity names
to that evidence. Neutral examples and small reproducible probes make it possible to verify a
quirk on another server without copying production data.
