# Contributing

Thank you for helping. twaco is used against servers people depend on, so the bar is: a
change says what it does, and a test would have caught its absence.

## Report an issue

Check the existing issues first. Include:

- what you ran, what you expected, and what happened, with the exact output;
- `twaco --version`, your operating system, and the ThingWorx version;
- for a problem with an entity, the smallest XML that shows it. Remove anything private
  first: passwords, hostnames, customer names.

Report security issues privately instead; see [SECURITY.md](SECURITY.md).

## Contribute a change

1. Fork the repository and create a branch: `git checkout -b fix/short-description`.
2. Make the change, with tests (see below).
3. Run `cargo test` and `cargo clippy --all-targets`. Neither may add a failure or warning.
4. Open a pull request that says what changed and why, and how you verified it.

For anything larger than a fix, open an issue first, so the design can be agreed before the
work.

## Set up

- Rust stable, via [rustup](https://rustup.rs).
- **Windows:** use the MSVC toolchain: `rustup default stable-msvc`, with the Visual Studio
  Build Tools installed. The GNU toolchain cannot build the TypeScript formatter twaco embeds.
- No ThingWorx server is needed to build or test. [Testing](documentation/TESTING.md)
  explains the suites, including the corpus tests over real repositories.

[Architecture](documentation/ARCHITECTURE.md) explains how the code is organised and how to
add a command.

## What a good change looks like

- **Behaviour lives in `src/core/`.** The CLI (`main.rs`) and the MCP server (`mcp.rs`) only
  parse, call and print.
- **Entities are never re-serialised.** Edit by span, through `scan` and `splice`.
- **A server write plans by default,** on the CLI (`--apply`) and over MCP (`dry_run`). CLI
  `call` is the one exception: it invokes the service it is given.
- **Every behaviour has a test** that fails without it. Fake the server behind a trait.
- **Errors say what to do.** A message names the file, the entity or the flag involved.
- **Match the code around you:** its naming, its comment style (comments say why, not what),
  and its formatting. Do not reformat files you are not otherwise changing.

## Knowledge contributions

The `quirks` topic (`knowledge/quirks.md`) holds ThingWorx behaviours verified on a live
server. A new one is welcome when:

- it is about the platform, not one solution;
- you verified it live, and say on which version;
- it says what you expected, what happens instead, and what to do about it.

Use neutral entity names (`Acme.*`), and add it as one `##` section, since `twaco guide`
searches by section.

## Commit messages

Write the subject as what the change does, in the imperative and under about 72 characters:
`Refuse a deploy when the baseline is missing`. Use the body to say why, and what you
verified.

## Code of conduct

Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md).
