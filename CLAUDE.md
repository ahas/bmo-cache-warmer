# Working in bmo-cache-warmer

These rules hold every change to this repository.

## The documents come first

- `docs/` is the canonical statement of what the service does; the code is one realisation of it.
  Before changing code, read the document covering that area: `docs/api.md` for the routes, the ping
  hook and the codes, `docs/warming.md` for the pass and the due rule, `docs/store.md` for the
  tables, their migrations and the snapshot.
- A change that would make a document wrong or incomplete repairs the document first, stating what is
  true now, and the code is written to match. A change to the boundary moves the version as
  `docs/api.md` says, in the same commit.
- A document states what is true now: no change history, no rationale told through an earlier
  design, no pasted source, signature or config fragment, and no file-and-line pins.

## Boundaries

- **It runs on its own.** Building, running and testing the warmer needs no context from another
  repository: no checkout beside it, and no document, policy file or setting of another service.
- **Only documented APIs.** The warmer meets another service only through a contract that service's
  documents state, and offers others only what `docs/api.md` states.
- **References run one way.** A consumer depends on the warmer, never the warmer on a consumer. A
  policy value several services share is owned by the lowest of them, and each repository holds only
  what it uses.
- **The warmer calls no other unit.** Consumers register with it and answer its target-ping hook; it
  knows no provider, model, account or service of theirs, and no name of a caller stands in its code or
  documents. Anything that differs per consumer or per kind of target is stated in the consumer's
  registration, never in a table here.
- **It is an upstream.** A harness can take it as its first entry point through its MCP server, with no
  other unit running; its API names nothing only one caller uses. It publishes its client and its
  harness plugin from this repository, both built on the one OpenAPI document `docs/openapi.json`.
- **The schedule rule is this repository's.** The due rule and the plan are the crate
  `bmo-cache-warmer-schedule` in `schedule/`, which has no dependency of any kind so that another
  repository may take it alone; the service re-exports it as its `schedule` module. The dependency
  check in `tests/` holds that it has none.
- **Dependencies:** crates from the registry pinned in the manifest, and nothing from another
  repository, the base libraries included: nothing by path out of the repository and nothing by git.
  The dependency check in `tests/` holds this.
- **No lookup table standing in for polymorphism.** A value that differs per member belongs to that
  member.
- **No hand-written parser for a general format.** TOML, JSON, command lines and their kin are read by
  a maintained library.

## Language

- Everything in this repository is English: code, comments, strings, tests, documents, commit messages
  and branch names. No Korean in code or tests; the Hangul check in `tests/` and the testbed's
  `korean` check hold this.
- What the service answers is an English sentence for a person, or a status and a kebab-case code for
  a machine. Words a person reads in another language are the web console's language packs.
- No personal data: no user names, home directories, machine names or names of any agent. Names the
  service needs at runtime come from its deployment's environment or arguments, never from the code.

## Layout

- One Cargo workspace at the root, made of the service package at the root and the schedule crate in
  `schedule/`, each member inheriting the workspace's one version; no other build tool's manifest, and
  no directory named after its parent. The layout check in `tests/` holds these.
- Each package's tests live in its own `tests/`, never beside the code they test.

## The stages of a change

1. **Writing:** the only check is `cargo check --workspace --all-targets`.
2. **Review:** read the whole diff against the document governing it, then `cargo fmt --all --check`
   and `cargo clippy --workspace --all-targets -- -D warnings`.
3. **Verification:** the test targets touched, then `cargo test --workspace` once, output written to a
   file and the file read.

A change is not done before stage 3 has passed.
