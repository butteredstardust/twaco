# Testing

```sh
cargo test                        # everything; no server, no network, no TypeScript needed
cargo clippy --all-targets        # lints
cargo fmt --all --check           # formatting; `cargo fmt` fixes it
```

## What runs where

| Suite | Where | Needs |
| --- | --- | --- |
| Unit tests | beside the code, `src/**` | nothing; servers are fakes behind a trait or a local TCP listener |
| `tests/offline.rs` | the built binary | nothing; proves offline commands start with an empty environment |
| `tests/call.rs` | the built binary against a local fake server | nothing |
| `tests/corpus.rs` | the bundled `Acme.Orders` repository, and real ThingWorx repositories when named | nothing; `TWACO_CORPUS` adds real ones |
| `tests/snapshots.rs` | the built binary, in a copy of the bundled repository | nothing; golden files in `tests/fixtures/snapshots/` |
| `tests/properties.rs` | generated documents, edits and scripts | nothing; 256 cases per property, `PROPTEST_CASES=5000` for a deeper run |

## The corpus tests

These are the strongest tests twaco has: they prove, over every entity of real repositories,
that tokens tile each document, that a no-op edit is the identity, that extracting and
syncing changes nothing, and that committed sidecars are reproduced byte for byte.

```sh
# Linux and macOS: separate repositories with ':'
TWACO_CORPUS=~/work/solution-a:~/work/solution-b cargo test --test corpus

# Windows (PowerShell): separate with ';'
$env:TWACO_CORPUS = "C:\work\solution-a;C:\work\solution-b"; cargo test --test corpus
```

They always run over `tests/fixtures/corpus/acme-orders`, a small repository of invented
entities (a project, template, shape, DataShape, DataTable, mashup and three Things) whose scripts
include the awkward cases: a CDATA terminator inside a string, template literals, a division after
a call, a `GetDBInfo` override. `TWACO_CORPUS` adds real repositories to it. The bundled one cannot
show what ThingWorx really writes, so run real ones after touching `scan`, `splice` or a sidecar
module. Each repository's script layout is detected from its own files, as `twaco init` detects
it.

A few real exports are refused by design and the tests expect exactly those refusals: a DataTable
whose accumulated shape the platform stored as invalid JSON, two services of one name in one
entity, and an entity whose name no rename may take.

Run them after any change to `scan`, `splice`, `sidecar`, `datashape`, `datatable` or
`mashup`.

## The command-line snapshots

`tests/snapshots.rs` runs 31 offline scenarios (usage, refusals, plans, JSON lines) and compares the
exit code, standard output and standard error with `tests/fixtures/snapshots/<name>.txt`. The text
is what people and scripts read, so a change to it should be a decision: the test fails until you
run `TWACO_BLESS=1 cargo test --test snapshots` and review `git diff tests/fixtures/snapshots`. The
temporary directory is written `<ROOT>`, path separators are made `/`, and digests and dates are
masked, so one file serves every platform. Every scenario is a plan, a refusal or an offline
command; none contacts a server.

## The property tests

`tests/properties.rs` holds the laws the byte-preserving core rests on, asserted over generated
inputs: tokens tile every document exactly, each start tag finds its own end tag, attribute spans
lie inside their quotes, `splice` with no edits or with each span replaced by its own bytes is the
identity, edit order never matters, overlapping or out-of-bounds edits are refused, and the readers
of files twaco does not write never panic. A failing case shrinks to a small input and is saved in
`tests/properties.proptest-regressions`; commit that file so everyone replays it.

## Against a live server

No automated test touches a server. To try a change against one, use a development server
you own, and throwaway entities you can find and delete afterwards (`ZZ.Test.*`, for
instance). Commands that write plan first: read the plan before adding `--apply`.

## Platform notes

- **Windows:** build with the MSVC toolchain (`rustup default stable-msvc`). The GNU
  toolchain cannot build the TypeScript formatter twaco embeds without MinGW's `dlltool`.
- The tests never run TypeScript: `types --check` is tested with a stand-in compiler, so
  `tsc` need not be installed.
