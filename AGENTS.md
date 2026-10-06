# AGENTS.md

Guidance for coding agents working on twaco itself. (For agents working on a ThingWorx
solution *with* twaco, see `twaco guide workflow` and the AGENTS.md that `twaco init` writes.)

## What this is

twaco is a Rust CLI and MCP server that keeps a ThingWorx solution in source control: service
scripts and other parts of entity XML as sidecar files, gates, safe deploys, server tools, and
knowledge for agents. Read [documentation/ARCHITECTURE.md](documentation/ARCHITECTURE.md)
before changing code; it names the principles and where everything lives.

## Rules that are not negotiable

- **Never re-serialise an entity file.** Change bytes through `core::scan` spans and
  `core::splice`. A no-op edit must be the identity, byte for byte; the corpus tests prove it.
- **A server write plans by default.** CLI: nothing is sent without `--apply`. MCP: `dry_run`
  defaults to `true`. `twaco call` on the CLI is the one exception, by design.
- **Credentials are never printed, logged, or written,** and `[[check]]` hooks do not receive
  them unless they declare `needs_credentials`.
- **Behaviour lives in `src/core/`.** `src/main.rs` with `src/cli/`, and `src/mcp/`, parse, call and print. A
  feature exists in both, or there is a reason it does not.
- **Every behaviour has a test that fails without it.** Servers are fakes behind a `Remote`
  trait or a local TCP listener; no test contacts a real server or the network.

## Commands

```sh
cargo test                       # all tests; no server or network needed
cargo clippy --all-targets       # no new warnings in files you touch
cargo fmt --all --check          # formatting; `cargo fmt` fixes it
TWACO_CORPUS=<repo>[:<repo>] cargo test --test corpus   # after touching scan, splice or sidecars
```

On Windows, build with the MSVC toolchain (`rustup default stable-msvc`).

## Style

- Match the surrounding code: naming, error types per module, comment density.
- Comments say why, not what. Doc comments state the contract and what is refused.
- Format with `cargo fmt` before you commit; CI runs `cargo fmt --all --check`. The tree was
  formatted in one commit (listed in `.git-blame-ignore-revs`), so formatting touches only what you
  changed.
- Results lead with a summary; detail is opt-in (`--detail`, `detail: true`).
- Error messages name the file, entity or flag, and say what to do.

## When you add a command

Follow "Adding a command" in ARCHITECTURE.md: core module, CLI route and usage line, MCP tool
and the `tools/list` test, the workspace lock if it writes, and documentation. When the usage text
changes, regenerate `documentation/COMMANDS.md` with `python scripts/commands_doc.py`.

## Knowledge topics

`knowledge/*.md` is compiled into the binary and served by `twaco guide`. A quirk is one `##`
section, verified on a live server, with neutral entity names (`Acme.*`).
