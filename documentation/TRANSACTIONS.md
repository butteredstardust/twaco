# Transactions

twaco changes several local files in one operation when it renames an entity, moves a service or
creates a building block. The transaction module (`src/core/transaction/`) makes such an
operation survive a crash: a killed process or a power cut between two writes no longer leaves the
workspace half changed. The next twaco command that takes the workspace lock finishes the
operation or undoes it.

`new building-block` uses it. The other multi-file commands (`relocate`, `rename`, ...) move onto
it one at a time, and each one's entry in [MUTATION_CLASSES.md](MUTATION_CLASSES.md) changes only
when it does.

## What it covers

- Local files only: create a file, replace a file, delete a file. A folder is created for a new
  file and removed again if the operation is undone.
- A server call is never part of a transaction. An import or a delete can succeed on the server
  while a read-back or a local record fails, and no local journal can undo that. Commands that
  talk to a server stay `server-partial`; only their local bookkeeping can use a journal.
- The workspace lock is held for the whole operation, so no other twaco is writing.

## The protocol

1. Every path is checked: it must lie inside the workspace (no `..`, no drive or root) and no
   part of it may be a symbolic link or, on Windows, any reparse point such as a junction. Every
   file the operation expects to find is read and compared with the bytes it was planned against;
   a file that changed since is refused and nothing is written.
2. A journal in the `staging` state is written to `.twaco/transactions/<operation>.json`. The new
   bytes of every file are staged beside its destination (`.<file>.<operation>.twaco-stage`) and a
   copy of every original is kept (`.<file>.<operation>.twaco-backup`), each synced and read back.
3. The journal moves to `applying`. The destinations change one at a time: a staged file is
   renamed over its destination, or the destination is removed. The journal notes each step.
4. The journal moves to `committed`; the stages, backups and journal are removed.

Stages and backups are not `*.twaco-tmp` files, so the lock's sweep for stale temporaries never
touches them.

## Recovery

When a command takes the lock it first reads every journal. Where an operation stands is read from
the files, not from the journal's marks: each destination holds its **before** state, its
**after** state, or something else. A step that became visible just before its mark was written
(the rename consumed its stage) is recognised by its digest.

| What is found | What happens |
| --- | --- |
| No journal | Nothing; the sweep for stale temporaries runs as before. |
| `committed` | Only the artifacts are removed. A later edit of a file is never rolled back. |
| `staging`, or every file in its before state | Nothing was visible: artifacts and the folders it made are removed. |
| Every file before or after, and the bytes still to install are staged and match | The operation is finished, then committed and cleaned up. |
| The staged bytes are missing or damaged, and every installed file has a matching backup | The installed steps are undone in reverse order, then cleaned up. |
| Any file holds neither state | Refused. Every such path is named with the digest found and the digests expected. Nothing is changed. |
| Neither finishing nor undoing is possible | Refused, as above. |
| More than one `applying` journal | Refused: no order between them is safe to guess. |
| A journal from a newer twaco, or one that cannot be read | Refused; it is not deleted. |
| A journal that names a path outside the workspace or behind a link | Refused before any file is opened, replaced or removed. |

A refusal fails the lock with the error code `rollback_failed` and a message that is the repair
plan: the paths involved, what was expected, and where the remaining copies are. A person keeps
the files as they are, merges or restores them deliberately so that each matches either its state
before the operation (to undo it) or after it (to finish it), and runs twaco again; it then
finishes the recovery.

If a step fails while the operation is running, it is undone straight away by the same code. If
that cannot finish, the error says so and names the journal; the next command recovers or refuses.

## Not covered

- A journal file that is deleted by hand while its stages remain: nothing records what they were
  for, so they stay where they are. They are hidden files ending `.twaco-stage` or
  `.twaco-backup` and can be deleted once the workspace is in a state a person trusts.
- Directory moves: a caller expresses one as file steps.
- Durability of directory entries on Windows: files and the journal are synced; on other
  platforms the containing directory is synced too. Recovery reads digests, so a rename that was
  not made durable is found in whichever state the filesystem kept.

## Tests

`src/core/transaction/tests.rs` covers the recovery table with hand-built crash states.
`src/core/transaction/tests/failpoints.rs` runs a plan in a child process that aborts at every
point of the protocol (after the journal, after staging, after each step is visible and after each
mark, after the commit) and has the parent recover it through the real lock; the building-block
tests do the same for a real `new building-block` run; it also kills the
child, edits a file, and checks that recovery refuses and leaves the edit alone. Those tests need
`--features test-failpoints`, which compiles the abort points in; a normal build contains none.

```sh
cargo test --features test-failpoints
```
