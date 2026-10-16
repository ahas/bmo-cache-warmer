# bmo-cache-warmer

A keepalive scheduler for prompt caches. A provider keeps a prompt prefix cached for a while after its
last use and then drops it; a prefix about to be used again is cheaper kept warm than rebuilt. The
warmer keeps registered prefixes warm without knowing what they are: a consumer registers each target
with the clocks it runs on, reports every use, and answers a ping hook; the warmer runs the pass,
judges which targets are due, asks their consumers to ping them, holds the owner's switches and keeps
the record of every ping. It calls no other service.

## Layout

| Folder | Holds |
|---|---|
| `src/` | The service's package: the library and the binary that serves it |
| `tests/` | The repository checks and the service's tests |
| `docs/` | The canonical statements of what the service does |

At the root stand the Cargo manifest, the version, `testbed.toml` with its Korean ledger,
`rustfmt.toml` and `CLAUDE.md`.

## Documents

| Document | States |
|---|---|
| `docs/api.md` | The boundary: the routes served, the ping hook a consumer answers, the codes, what each consumer registers, and how the keepalive contracts bmo reads are served |
| `docs/warming.md` | The pass, when a target is due, groups, what each answer leaves behind, and the owner's switches |

## Version

- **One version for the repository,** the package's, in semver. `VERSION` holds it and the package's
  version equals it.
- **How a change moves it** is stated in `docs/api.md`, 「Versioning」.

## Commands

```sh
cargo check --all-targets
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
