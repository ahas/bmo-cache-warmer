# bmo-cache-warmer

A keepalive scheduler for prompt caches. A provider keeps a prompt prefix cached for a while after its
last use and then drops it; a prefix about to be used again is cheaper kept warm than rebuilt. The
warmer keeps registered prefixes warm without knowing what they are: a consumer registers each target
with the clocks it runs on, records every use, and serves a target-ping hook; the warmer runs the pass,
judges which targets are due, asks their consumers to ping them, holds the switches and keeps the
record of every ping. It calls no other service. Beside the service it publishes its client, its
OpenAPI document, the harness plugin (an MCP server and a skill) and the schedule rule as a library.

## Layout

| Folder | Holds |
|---|---|
| `src/` | The package: the schedule library, the service, its client, OpenAPI document and MCP server, and the binary |
| `examples/` | The program that prints the OpenAPI document the repository keeps |
| `plugin/` | The harness plugin's skill |
| `policy/` | The warmer's own fixed numbers, as a TOML file built into the binary |
| `tests/` | The repository checks and the service's tests |
| `deploy/` | The service's systemd user unit, which a deployment links from the deploy tree |
| `docs/` | The canonical statements of what the service does, and its OpenAPI document |

At the root stand the Cargo manifest, the version, `testbed.toml` with its Korean ledger,
`rustfmt.toml` and `CLAUDE.md`.

## Documents

| Document | States |
|---|---|
| `docs/api.md` | The boundary: the wire format, the routes, the events, the target-ping hook, the codes, the client, the harness plugin, the binary's commands and the deployed service |
| `docs/openapi.json` | The contract as an OpenAPI 3.1 document, written from the code |
| `docs/warming.md` | The pass, when a target is due, groups, what each answer leaves behind, the switches, and the policy |
| `docs/store.md` | What the store keeps, how its shape moves, and how a snapshot of it is taken, checked and put back |

## Version

- **One version for the repository,** the package's. `VERSION` holds it and the package's version
  equals it.
- **How a change moves it** is stated in `docs/api.md`, 「Versioning」.

## Commands

```sh
cargo check --all-targets
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run --example openapi > docs/openapi.json
```
