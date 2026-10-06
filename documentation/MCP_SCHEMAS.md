# MCP schemas

MCP tool inputs live at the adapter boundary in `src/mcp/requests/`. They are not core command
requests: they express JSON defaults, optionality and action spellings that only MCP has.

To add a typed tool, declare a `Deserialize` and `JsonSchema` request object in the family module,
with `#[serde(deny_unknown_fields)]`. Field documentation is the published schema description, so
copy the existing description exactly. Put defaults on the serde field, use an enum for a finite
choice, and use `Absent<T>` for a field that may be omitted but must reject JSON `null`. Do not
use `Option<T>` for such fields: JSON Schema correctly treats it as nullable.

Register the request with its description, generated definition and typed route in
`mcp::registry`. The route parses once, converts mechanically to the core command request, and
does not use the loose JSON helpers. Tools not yet migrated continue through `schema.rs` during
the rollout.

The generator uses Schemars' deserialize contract and Draft 2020-12 internally, inlines
subschemas, and removes generated-only keywords before publishing. MCP tests validate every
published schema against the Draft 2020-12 metaschema and refuse those removed keywords.

`tests/fixtures/mcp_input_contract.json` records accepted and rejected inputs for each migrated
tool. Its test checks the old validator and typed deserialization agree, including generated
unknown-field mutations. Add cases for defaults, `null`, wrong types, enum and numeric boundaries,
and action combinations whenever a request changes.

Output schemas are attached only by a registry entry with a stable, owned projection, and only
after the `2025-06-18` protocol revision. Successful projections must be validated against their
schema before their MCP envelope is made; arbitrary server payloads do not get an `outputSchema`.
