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
| `tests/corpus.rs` | real ThingWorx repositories | `TWACO_CORPUS` |
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

Without `TWACO_CORPUS` they skip and pass. Set `TWACO_REQUIRE_CORPUS=1` to make a missing
corpus fail instead. Each repository's script layout is detected from its own files, as
`twaco init` detects it.

Run them after any change to `scan`, `splice`, `sidecar`, `datashape`, `datatable` or
`mashup`.

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
