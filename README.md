**English** | [한국어](README.ko.md) | [日本語](README.ja.md) | [简体中文](README.zh-CN.md)

# bmo-cache-warmer

A keepalive scheduler for prompt caches. A provider keeps a prompt prefix cached for a while after its
last use and then drops it; a prefix about to be used again is cheaper kept warm than rebuilt. The
warmer keeps registered prefixes warm without knowing what they are: a consumer registers each target
with the clocks it runs on, records every use, and serves a target-ping hook; the warmer runs the pass,
judges which targets are due, asks their consumers to ping them, holds the switches and keeps the
record of every ping. It calls no other service. Beside the service it publishes its client, its
OpenAPI document, the harness plugin (an MCP server and a skill) and the schedule rule as a crate
with no dependency.

## Layout

| Folder | Holds |
|---|---|
| `src/` | The service package: the service, its client, OpenAPI document and MCP server, and the binary |
| `schedule/` | The schedule crate: the due rule and the plan, with its own tests |
| `examples/` | The program that prints the OpenAPI document the repository keeps |
| `plugin/` | The harness plugin's skill |
| `policy/` | The warmer's own fixed numbers, as a TOML file built into the binary |
| `tests/` | The repository checks and the service's tests |
| `deploy/` | The service's systemd user unit, which a deployment links from the deploy tree |
| `docs/` | The canonical statements of what the service does, and its OpenAPI document |

At the root stand the workspace's Cargo manifest, which is also the service package's, the version,
`testbed.toml` with its Korean ledger, `install.toml`, `rustfmt.toml`, `CLAUDE.md` and the MIT licence in `LICENSE`.

## Documents

| Document | States |
|---|---|
| `docs/api.md` | The boundary: the wire format, the routes, the events, the target-ping hook, the codes, the client, the schedule crate, the harness plugin, the binary's commands and the deployed service |
| `docs/openapi.json` | The contract as an OpenAPI 3.1 document, written from the code |
| `docs/warming.md` | The pass, when a target is due, groups, what each answer leaves behind, the switches, and the policy |
| `docs/store.md` | What the store keeps, how its shape moves, and how a snapshot of it is taken, checked and put back |
| `docs/PERSONAL-DATA.md` | The personal data no file of the repository may hold, what goes in its place, and the check that reads every file for it |

## Version

- **The version is the set's,** read off the git tag when a build is made; the workspace's version, which
  every member package inherits, is a placeholder that never moves.
- **How it is read and answered** is stated in `docs/api.md`, 「Versioning」.

## Commands

```sh
cargo check --workspace --all-targets
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --example openapi > docs/openapi.json
```
