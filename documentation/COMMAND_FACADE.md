# The command facade

A command that changes something (the workspace, a server, or both) has its policy in one place:
`src/core/commands/<command>.rs`. The command line (`src/main.rs`) and the MCP server
(`src/mcp.rs`) parse their own input into a request, call the executor, and project the outcome
to their own output. Neither decides anything about locking, plan versus apply, profiles or
backups, so the two cannot drift apart. `push.rs` and `delete.rs` are the reference
implementations.

## What a command module holds

- **A request**: a plain struct of what a caller can say (names, flags, a profile name, a
  `Mode`). No `Args`, no `serde_json::Value`, no output formatting.
- **An outcome**: a typed result that reuses the domain's own types (`push::Decision`,
  `entity_delete::Report`, ...). A plan and an applied result are different variants. Each
  carries accurate `Effects` (workspace and server access: none, read, write). A partial failure
  that the domain already reports row by row (a delete that failed for one entity) is an
  outcome, not an error.
- **An error** that wraps the domain's own errors without flattening them, with a `Coded` impl so
  every failure keeps its stable `ErrorCode`, and a `Display` that is the message both front ends
  show today.
- **An executor**: `execute(solution, &request, open, &mut Notices) -> Result<Outcome, Error>`,
  where `open` builds the remote from a loaded profile so a test supplies a fake. It owns:
  - **the lock**: taken through `commands::lock_workspace`, only when applying and only when the
    command writes the workspace, before the command discovers or reads anything it will change;
    a command whose lock need depends on what it finds takes it when it knows, then prepares
    again; a plan takes none. What taking the lock swept or recovered goes into `Notices`.
  - **the profile**: loaded once, by name, here.
  - **plan versus apply**, and the backup that precedes a risky apply.

## What an adapter holds

- The command line: flags to a request, the executor call, `print_notices`, then the outcome as
  the lines and exit status the command has always printed. The route is removed from
  `writes_workspace` in `main.rs` once the executor takes the lock.
- MCP: arguments to a request, the executor call, then the outcome as the JSON the tool has
  always returned, with `notices` added when there are any. The tool takes no lock itself.

Output is not the outcome's job: the outcome is not a wire format, and a field is not added to it
because one front end prints it.

## Rules for moving a command onto the facade

- Output is byte-identical: `tests/fixtures/snapshots/*` and `tests/fixtures/mcp_tools.json` do
  not change, and no existing test is weakened.
- Every behaviour moved keeps a test that fails without it: a plan takes no lock; an apply locks
  before it discovers; the order of backup, change and record; each refusal keeps its error code;
  a fake remote covers the plan/apply and force/no-force combinations.
- A migrated command's entry in `MUTATION_CLASSES.md` is checked, and its code pointer updated to
  the executor if the function it names moved.
