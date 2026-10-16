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

- **The warmer calls no other unit.** Consumers register with it and answer its ping hook; it knows no
  provider, model, account or service of theirs. Anything that differs per consumer or per kind of
  target is stated in the consumer's registration, never in a table here.
- **Dependencies:** crates from the registry pinned in the manifest, and nothing from another
  repository, the base libraries included. The dependency check in `tests/` holds this.
- **No lookup table standing in for polymorphism.** A value that differs per member belongs to that
  member.
- **No hand-written parser for a general format.** TOML, JSON, command lines and their kin are read by
  a maintained library.

## Language

- Everything in this repository is English: code, comments, strings, tests, documents, commit messages
  and branch names. No Korean in code or tests; the Hangul check in `tests/` and the testbed's
  `korean` check hold this.
- What the service answers is an English sentence for a person, or a status and a snake_case code for
  a machine. Words a person reads in another language are the web console's language packs.
- No personal data: no user names, home directories, machine names or names of any agent. Names the
  service needs at runtime come from its deployment's environment or arguments, never from the code.

## Layout

- One Cargo package at the root, no other build tool's manifest, and no directory named after its
  parent; the layout check in `tests/` holds these.
- Tests live in `tests/`, never beside the code they test.

## The stages of a change

1. **Writing:** the only check is `cargo check --all-targets`.
2. **Review:** read the whole diff against the document governing it, then `cargo fmt --check` and
   `cargo clippy --all-targets -- -D warnings`.
3. **Verification:** the test targets touched, then `cargo test` once, output written to a file and
   the file read.

A change is not done before stage 3 has passed.
