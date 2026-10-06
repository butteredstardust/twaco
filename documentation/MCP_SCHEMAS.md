# MCP schemas

Every MCP tool is defined once, in `src/mcp/registry.rs`: its name, description, whether it only
reads, the request object its arguments are read into, and the adapter that runs it. The published
`inputSchema` is generated from the request object, the arguments are read through the same
object, and the adapter receives it typed, so a schema cannot drift from what an adapter reads.

## Request objects

They live in `src/mcp/requests/`, one file per command family. They are not core command requests:
they express JSON defaults, optionality and action spellings that only MCP has, and the adapter
converts them to the core request.

To add a tool:

1. Declare a `Deserialize`, `Serialize` and `JsonSchema` struct with `#[serde(deny_unknown_fields)]`.
   Field documentation is the published description of that argument. Put defaults on the serde
   field, use an enum for a finite choice, and use `Absent<T>` for a field that may be omitted but
   must reject JSON `null`; `Option<T>` is nullable in a schema, which is not what an argument is.
   An argument the adapter must tell apart from its default (`logs.since`) is `Absent<T>` with the
   default published through `#[schemars(extend("default" = ...))]`.
2. Write the adapter in the family file in `src/mcp/`: `fn(&Solution, Request) -> Result<Value, ToolError>`.
   It does not read JSON by hand. A text the tool cannot do without is checked with `nonempty` or
   `required_text`, which keep the "is required" refusal for an empty value.
3. Add one entry to `TOOLS` in `registry.rs`, in the place the tool is listed. `solution_tool` runs
   the adapter in the solution found from the root, `root_tool` gives it only the root, and
   `bare_tool` gives it neither.

The generator uses Schemars' deserialize contract and Draft 2020-12 internally, inlines subschemas,
and removes the keywords it adds that clients need not see. A tool's own property called `format` or
`title` is kept.

## Error messages

The arguments are checked against the published schema by `mcp/schema.rs` before they are read, so a
client sees the same messages whatever the request type, then read into the request. Anything the
schema allows and the tool still refuses (an empty text, `give file or sql`) is the adapter's.

## How it is tested

- `tests/fixtures/mcp_tools_hand_written.json` freezes the tool schemas as they were written by hand,
  before they were generated. One test builds argument objects from each (every property fitting and
  misfitting, a required one missing, an unknown one) and checks that the typed tool accepts and
  rejects exactly what the hand-written schema did, with the same message, and that what it reads can
  be written back within the published schema. Another checks each generated schema means what the
  hand-written one did, for the same tools in the same order. They guard a change to a request object:
  when an argument is deliberately added or changed, change the frozen file with it.
- Every published schema is validated against the Draft 2020-12 metaschema, must be an object, and
  may hold no generated-only keyword.
- `tests/fixtures/mcp_tools.json` pins `tools/list`; re-bless it with `TWACO_BLESS=1`.

## Output schemas

`mcp/outputs.rs` describes the successful result of the tools whose shape is stable: `projects`,
`check`, `status`, `sync`, `extract`, `fmt`, `types`, `push` and `deploy`. The types describe the JSON
the adapters return; they do not build it. A member every result has is required, anything a result
has only sometimes is optional, and the objects stay open so that `notices` and `duration_ms` are
not refused. They are published only to clients that negotiated the `2025-06-18` revision or later.
Tools whose result carries arbitrary server or document data (`call`, `db_query`, `repo`, the help
tools and so on) publish none: a schema of "anything" tells a client nothing.

Under test, the registry checks every successful result of a tool that publishes an output schema
against it, and tests drive each of the nine tools, including the shapes a plan, an applied run and a
refusal take. Add a case there when a result gains a variant.
