# Boundary API

Every interface by which cache-warmer meets anything outside its own process: the HTTP routes it
serves, the event stream, the target-ping hook a consumer serves, the client and the harness plugin
it publishes, and the commands of its one binary. Other services match against this document and
`docs/openapi.json`; the code is one realisation of them. How a pass runs and when a target is due
is `docs/warming.md`; what the store keeps is `docs/store.md`.

The warmer is a keepalive scheduler and nothing else. It knows no provider, no model, no account and
no caller. A consumer registers what to keep warm and on which clock, tells it each time a target is
used, and serves the hook the warmer posts to when a target is due; the warmer decides when to ask,
holds the switches and keeps the record of every ping. It calls no other service: the only calls it
makes are pings to the URLs consumers registered.

## Versioning

The version is the package's in `Cargo.toml` (semver); `VERSION` at the root holds the same. The
health route, the OpenAPI document and the MCP server all answer it. This document and
`docs/openapi.json` change in the same commit as the boundary they state.

- **Before the first release the version is `0.1.0` and counts revisions.** The first version, `0.1.0`,
  is the base. Every later version bump is one revision, written `0.1.0-rev.<n>` with `n` counting the
  bumps since the base from 1. Major, minor and patch do not move before the first release, whatever
  the change, a breaking one included.
- **The first release drops the suffix,** and from then on the version moves by semver: a change to a
  contract an existing caller reads moves the major version, an addition the minor, anything else
  the patch.

## The contract

- **`docs/openapi.json` is the contract,** an OpenAPI 3.1 document written from the service's own
  types. A test fails where the committed document differs from what the code writes; the example
  `openapi` prints it again (`cargo run --example openapi`).
- **Its `x-tools` are the MCP server's tools,** each naming the route it calls; `x-harness` names the
  plugin, the command that starts its MCP server and what a harness does first with it; `x-events`
  lists the event kinds; `webhooks` states the target-ping hook.

## Wire format

- **Bodies are JSON with camelCase keys.** Times are one integer of epoch milliseconds and end in `At`;
  spans are milliseconds and end in `Ms`; counts end in `Count`. A field marked `?` may be absent or
  null.
- **Every route takes the bearer token,** `/health` included: `Authorization: Bearer <token>`. A
  request without it, or with another, is refused 401 `unauthorized`.
- **A name** (a consumer, a target, a subject, a group) in a path or a body is a non-empty UTF-8 string
  of at most 256 bytes, percent-encoded in a path, compared byte for byte. A target, a subject and a
  group belong to one consumer: the same name under two consumers is two things.
- **Each member also has an `id`,** a signed 64-bit integer the store gives it, which only grows. A
  path names a member by its names; a list pages by the ids.
- **A list is a page,** `{"items": [...], "more": boolean}`, in id order. `after` is the id of the last
  item read, and the page starts past it; `limit` is 1 to 1000, 100 where absent. `more` says whether
  anything stands past the last item.
- **A kind word** is kebab-case: lower-case ASCII letters, digits and hyphens, beginning with a
  letter, at most 64 bytes.
- **A PATCH body is a JSON merge patch:** a field left out stays as it is, and a field given `null`
  returns to its unset state.
- **A refusal** is a status and a body of two keys: `code`, a kebab-case word listed under
  「Codes」, and `error`, one English sentence saying what was wrong. A caller matches on `code`,
  never on `error`.

## The things it holds

- **A consumer** is whatever has targets warmed: known by its name and the URL of its target-ping hook.
- **A target** is one cache prefix the consumer wants kept warm. The consumer names it and declares its
  clocks.
- **A subject** is what a switch turns on and off. Each target names its subject, its own name where it
  names none; several targets may share one. A subject exists once a target names it or a switch or
  pause is set on it.
- **A group** is a set of targets of which only so many are pinged in one pass. A target names the one
  group it is in, if any.
- **The round** is the one switch over every target at once, and the interval the pass comes round at.
- **The record** of a target is its last use, its last warmed ping, its last failed attempt, and its
  last attempt with its outcome.

## Routes

### GET `/health`

Answers 200 with `ready` (bool, true once the store reads), `reason` (string, null when ready: why
not) and `version`.

### GET `/events`

The service's one event stream (「Events」).

### GET `/consumers`

A page of consumers. Each:

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | int | |
| `name` | string | |
| `url` | string | the absolute `http` URL its target-ping hook is served at |
| `createdAt` | int | when it was first registered |

### PUT `/consumers/{consumerName}`

Registers a consumer or moves its hook. Body: `url`, an absolute `http` URL; consumers are services on
the same host, so the warmer speaks plain HTTP only. Answers 201 with the consumer when it is new, 200
when it stood. 422 `invalid-url` when the URL does not parse or is not `http`.

### PUT `/consumers/{consumerName}/groups/{groupName}`

Declares a group or changes its limit. Body: `limit`, how many of the group's targets one pass may
ping, 0 or more. Answers 201 or 200 with the group:

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | int | |
| `consumerName`, `name` | string | |
| `limit` | int | |
| `targetNames` | string[] | the targets that name it, by name |

A target joins a group by naming it in its own registration; the group lists them. 404 `not-found`
when the consumer is not registered.

### DELETE `/consumers/{consumerName}/groups/{groupName}`

Retires a group. 204. 404 `not-found`; 409 `conflict` while a target names it.

### GET `/targets`

A page of every target. Query `consumerName` and `subjectName` narrow it. Each:

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | int | |
| `consumerName`, `name` | string | |
| `kind?`, `label?` | string | as registered |
| `subjectName` | string | as registered, or the target's own name |
| `rank` | int | its place in the pass: lower goes first |
| `groupName?` | string | as registered |
| `ttlMs`, `idleMs`, `marginMs`, `pingBoundMs` | int | its clocks as registered, or as the policy filled them in |
| `warmByDefault` | bool | as registered, or the policy's default |
| `failureTouches`, `honoursIdleOverride` | bool | as registered |
| `on` | bool | whether it warms now: its subject's switch where set, else `warmByDefault` |
| `pausedUntil?` | int | when its subject's pause ends, while one stands |
| `usedAt?` | int | the record's last use |
| `warmedAt?` | int | when its last warmed answer arrived |
| `failedAt?` | int | when its last failed attempt ended, whatever came after it |
| `lastAttempt?` | object | `at`, `outcome`, `code?`, `reason?` of its last attempt |
| `nextPingAt?` | int | when it is next due by its own clocks; the moment of the answer when it was never touched; null when it is off, paused, past its lifetime until it is used again, or when its lifetime less its margin leaves no room for its idle stretch |

`nextPingAt` reads the target alone: a group's limit and the round may still pass it over.

### PUT `/consumers/{consumerName}/targets/{targetName}`

Registers a target or restates one. Restating changes what the registration declares and keeps the
record and the subject's switches; the clocks the next judgement reads are the restated ones.

| Field | Type | Meaning |
| --- | --- | --- |
| `ttlMs` | int | how long the provider keeps the prefix after its last touch, 0 or more |
| `idleMs?` | int | how long it may stand idle before a ping is due, above 0; the policy's share of its usable lifetime when absent |
| `marginMs?` | int | taken off `ttlMs` before judging, 0 or more; 0 when absent |
| `pingBoundMs?` | int | how long the consumer may take to answer one ping, above 0; the policy's bound when absent |
| `subjectName?` | string | the subject it is switched by; the target's own name when absent |
| `groupName?` | string | the group it is in |
| `rank?` | int | its place in the pass; 0 when absent |
| `kind?` | string | the consumer's kind word for what this is; handed back in each ping |
| `label?` | string | a line a screen shows; the consumer's wording |
| `warmByDefault?` | bool | whether it warms while its subject's switch is unset; the policy's default when absent |
| `failureTouches?` | bool | whether a failed attempt counts as a touch; false when absent |
| `honoursIdleOverride?` | bool | whether a pass run with an idle override applies it here; true when absent |
| `usedAt?` | int | when it was last used, where the consumer knows; taken only where later than the record's |

Answers 201 on a new target, 200 on a restated one, with the target as listed. 404 `not-found` when
the consumer is not registered; 422 `unknown-group` when `groupName` names no group of the consumer;
422 `invalid-clock` when a clock is out of its range; 422 `invalid-kind` when `kind` is not a kind
word. A lifetime no larger than its margin is accepted: such a target is never due.

### DELETE `/consumers/{consumerName}/targets/{targetName}`

Retires a target and its record. 204. 404 `not-found`. A subject left with no target keeps its
switches.

### POST `/consumers/{consumerName}/targets/{targetName}/uses`

Records that the target's prefix was used. The body may be left out.

| Field | Type | Meaning |
| --- | --- | --- |
| `at?` | int | when the prefix was last written; the moment the report arrives when absent |
| `startedAt?` | int | when the work that used it began; `at` when absent |

The record's last use moves to `at` where that is later than it. A pause on the target's subject set
before `startedAt` is lifted; one set after it stands, so work already running when the pause was set
does not undo it. Answers 201 with the use: `consumerName`, `targetName`, `at`, `startedAt`. 404
`not-found`.

### GET `/consumers/{consumerName}/subjects`

A page of the consumer's subjects. Each:

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | int | |
| `consumerName`, `name` | string | |
| `on?` | bool | its switch: true or false where set, null while unset |
| `pausedAt?`, `pausedUntil?` | int | when its pause was set and when it ends, while one stands |
| `targetCount` | int | how many targets name it |

404 `not-found` when the consumer is not registered.

### PATCH `/consumers/{consumerName}/subjects/{subjectName}`

Throws a subject's switch or sets its pause, as a merge patch:

| Field | Type | Meaning |
| --- | --- | --- |
| `on?` | bool | true or false throws the switch; null returns every target of the subject to its registration's default |
| `pausedUntil?` | int | pauses the subject from now until then, moving a pause already standing; null lifts the pause |

Answers 200 with the subject. A subject not yet named by any target is made, so a subject can be
switched before its first target is registered. 404 `not-found` when the consumer is not registered;
422 `invalid-clock` for a `pausedUntil` not after now.

Turning a subject off leaves its targets' records where they stood; turning it back on judges them
from those records.

### GET `/round`

Answers 200 with the round:

| Field | Type | Meaning |
| --- | --- | --- |
| `intervalMs` | int | how often a pass comes round: the one set, or the policy's tick while none is |
| `nextAt` | int | when the next pass of the ticker is due |
| `running` | bool | false while every warming is stopped at once |

### PATCH `/round`

Changes the round, as a merge patch: `intervalMs?` (above 0; null returns it to the policy's tick)
and `running?`. Answers 200 with the round. A new interval moves the next pass to the last pass's
time plus the new interval, or now where that has passed. A pass already running sees `running`
before its next ping. 422 `invalid-clock` for an interval not above 0.

### POST `/passes`

Runs one pass now and answers once it ends. The body may be left out: `idleOverrideMs?`, used instead
of `idleMs` for every target that honours it, above 0. Answers 201 with the pass:

| Field | Type | Meaning |
| --- | --- | --- |
| `at` | int | when it ended |
| `pingedCount` | int | how many pings went |
| `stopped` | bool | whether it found the round stopped, at its start or before one of its pings, so what remained was not pinged |
| `attempts` | object[] | one per ping in the order they went: `consumerName`, `targetName`, `at`, `outcome`, `code?`, `reason?` |

409 `conflict` while a pass, the ticker's or another caller's, is running; 422 `invalid-clock` for an
override not above 0.

## Events

`GET /events` is the one stream, as server-sent events. Each event's SSE `event` is its kind, its
`id` the event's number, and its `data` `{"id", "kind", "at", "data"}`, the inner `data` being the
member as a route answers it.

| Kind | When | `data` |
| --- | --- | --- |
| `target.changed` | a target is registered or restated, a use is recorded, a ping's attempt is recorded | the target |
| `target.deleted` | a target is retired | its `id`, `consumerName` and `name` |
| `subject.changed` | a subject's switch or pause is set, or a use lifts its pause | the subject |
| `round.changed` | the round is changed | the round |
| `pass.created` | a pass ends, the ticker's or one asked for | the pass |

- **A reader resumes** with `Last-Event-ID` or `after`, the number of the last event read, and
  narrows with `kinds`, a comma-separated list. The stream sends what stands past the cursor, then
  each event as it is published, and a comment line every fifteen seconds. A reader that gives no
  cursor reads from the start of the window.
- **The window:** events are kept for the policy's `event_days`. A resume from before the window is
  refused 410 `gone`, and the reader reads the collections again.
- **A page:** a request asking `Accept: application/json` is answered a page of events past `after`,
  by `limit`, for a caller that polls.

## The target-ping hook

The one contract the warmer consumes, stated under `webhooks` as `target-ping`. To warm a target, the
warmer posts to its consumer's `url` directly, through no proxy, and follows no redirect:

| Field | Type | Meaning |
| --- | --- | --- |
| `consumerName`, `targetName` | string | |
| `subjectName` | string | as registered |
| `kind?` | string | as registered |
| `dueAt` | int | when the target fell due |
| `boundMs` | int | the target's `pingBoundMs`: how long the consumer has to answer |

The warmer waits `boundMs` plus the policy's slack. The consumer does the warming, puts the ping on
the line the target's work runs on, and answers 204 when the prefix was read and written again, or
200 with:

| Field | Type | Meaning |
| --- | --- | --- |
| `outcome` | string | `warmed`, `busy`, `skipped` or `failed` |
| `code?` | string | for `skipped`, required: a kind word saying why; for `failed`, optional |
| `reason?` | string | for `failed`, required: an English sentence |

- **`warmed`,** or 204: the prefix was read and written again. The record's last warmed time becomes
  the moment the answer arrived; this is the only answer that restarts the target's clock.
- **`busy`:** work was running on the target, which keeps the prefix warm itself, so no ping went.
- **`skipped`:** the consumer chose not to ping now. The `code` is the consumer's own word; the warmer
  stores it unread.
- **`failed`:** a ping went and did not warm. The reason is cut to the policy's length, counted in
  UTF-16 units.

Busy and skipped answers move nothing but the last attempt. A failed one moves the target's touch
only where it registered `failureTouches` (`docs/warming.md`).

When the hook does not answer in time, cannot be reached, answers another status, or answers a body
that does not read as above, the warmer records `failed` with one of these codes and an English
reason of its own:

| Code | Meaning |
| --- | --- |
| `hook-timeout` | the hook did not answer within its bound and the slack |
| `hook-unreachable` | the hook URL could not be connected to, or the request broke off before an answer came |
| `hook-status` | the hook answered a status other than 200 or 204, a redirect included |
| `hook-answer` | the hook's body did not read as an answer, or a skipped answer carried no kind word as its code, or a failed one no reason |

## Codes

| Code | Status | Meaning |
| --- | --- | --- |
| `bad-request` | 400 | the body, the query or a name in the path is not the shape the route takes |
| `unauthorized` | 401 | the bearer token is missing or wrong |
| `no-route` | 404 | no route answers this method and path |
| `not-found` | 404 | nothing stands at the names the path gives |
| `conflict` | 409 | a target still names the group, or a pass is already running |
| `gone` | 410 | the events past the cursor asked for are no longer kept |
| `invalid-url` | 422 | the hook URL does not parse or is not `http` |
| `invalid-clock` | 422 | a span or a time is out of its range |
| `invalid-kind` | 422 | the kind is not a kind word |
| `unknown-group` | 422 | the registration names no group of the consumer |
| `failed` | 500 | the store could not be read or written |

## The client

The module `client` of the package is the service's client: one method for each route, sending and
reading the bodies `docs/openapi.json` states, a refusal read back as its status, code and sentence.
It carries the contract and no caller's policy. It is built with the address and the token, and
reaches the service through no proxy.

## The harness plugin

The plugin `bmo-cache-warmer`: the MCP server `bmo-cache-warmer mcp` starts on standard input and
output, and the skill `plugin/skills/bmo-cache-warmer/SKILL.md`, which tells a harness when and how
to use it.

- **Its tools** are `warmer_consumers`, `warmer_targets`, `warmer_target_put`, `warmer_target_used`,
  `warmer_subjects` and `warmer_round`, each a thin call of one route through the client. Each input
  takes the route's path names, query and body as one object; each output is the route's answer.
- **A refusal is a tool error** whose text is JSON: `status`, `code` and `error` as the service
  answered them, `code` `bad-request` for arguments that are not the tool's shape, and `error` alone
  where the service could not be reached.

## The binary

`bmo-cache-warmer` with no subcommand is the service. Each setting is given as an argument or, where
the argument is absent, in the environment.

| Command | Argument | Environment | Meaning |
| --- | --- | --- | --- |
| the service | `--listen` | `CACHE_WARMER_LISTEN` | the IP address and port to listen on, on loopback |
| the service | `--store` | `CACHE_WARMER_STORE` | the path of the store's SQLite file (`docs/store.md`), created where missing |
| the service | `--token-file` | `CACHE_WARMER_TOKEN_FILE` | the file holding the bearer token every route takes |
| `mcp`, `health` | `--address` | `CACHE_WARMER_ADDRESS` | the service's address; `http://127.0.0.1:9023` when absent |
| `mcp`, `health` | `--token` | `CACHE_WARMER_TOKEN` | the bearer token itself |
| `mcp`, `health` | `--token-file` | `CACHE_WARMER_TOKEN_FILE` | the file holding the token, where no token is given |
| `snapshot` | `--store` | `CACHE_WARMER_STORE` | the store's SQLite file to copy |
| `snapshot` | `--into` | `BMO_BACKUP_SNAPSHOT_DIR` | the directory the copy and its manifest are written into, made where missing |
| `verify` | `--from` | `BMO_BACKUP_SNAPSHOT_DIR` | the directory holding a snapshot to check |

### The service

None of its settings has a default. A setting missing or not read, an address not on loopback, a
token file that does not read or holds nothing, a store that does not open or a port that cannot be
bound stops the service before it serves anything, with an English sentence on standard error and a
non-zero exit. Once the store is open and the port bound, the routes are served and the ticker starts
(`docs/warming.md`, 「The pass」).

### `mcp` and `health`

- **`mcp`** serves the harness plugin's MCP server on standard input and output until its input ends.
- **`health`** asks `GET /health` and exits 0 when the service answers ready, with one English sentence
  on standard output; anything else exits 1 with a sentence on standard error saying what it found.
- **Neither starts without a token:** neither `--token` nor `--token-file` given, or a token file that
  does not read or holds nothing, is refused with the command line's usage or a sentence, and a
  non-zero exit.

### `snapshot` and `verify`

What a snapshot holds and how it is judged is `docs/store.md`, 「The snapshot」.

- **Each exits 0 when it has done what it says,** with one English sentence on standard output.
- **Anything else exits non-zero** with an English sentence on standard error saying what failed: a
  store that does not open or is not a migrated store, a directory already holding a snapshot, a
  copy that cannot be written; for `verify`, a missing or unreadable manifest, a missing copy, or a
  copy that does not hold to its manifest. A setting missing or not read is refused with the command
  line's usage and exit code 2.
- **Neither serves nor warms, and neither changes the store:** `snapshot` opens it without creating it
  or moving its shape, and `verify` opens the copy read-only.

## The deployed service

- **It listens on `127.0.0.1:9023`.** Callers reach it there; no sibling repository declares the port.
- **It runs as the user unit `bmo-cache-warmer.service`,** the repository's one service, from the
  release binary in its deploy tree under `~/.local/share/bmo-cache-warmer`. A deployment links the
  binary into the folder holding `bmo`, so `bmo-cache-warmer mcp` starts by its name.
- **Its store is `warmer.sqlite` in its own data root,** `~/.local/share/bmo-cache-warmer/data`, inside
  no checkout and no release, so a release leaves it alone. The unit makes the folder where it is
  missing.
- **Its bearer token is the file `token` in the data root.** A deployment makes it once where none
  stands and keeps it across deployments; a caller is given the same token through its own settings.
- **A deployment takes a backup before anything changes,** into
  `~/.local/share/bmo-cache-warmer/backups`, outside the data root. The backup holds a consistent
  snapshot of the store, taken by the live release binary's `snapshot` and held to its manifest by
  that binary's `verify`, beside the rest of the data root packed as it stands. The store file and the
  journal files SQLite keeps beside it are left out of that pack, since a copy read while the service
  writes could be torn; the snapshot is the store's copy in the backup. A deployment that finds no
  release binary, as the first does, takes no snapshot, and its backup passes only while no store
  stands either. Putting a snapshot back is `docs/store.md`, 「Putting a snapshot back」.
- **A deployment waits on the binary's `health`** answering ready before it watches the service stay
  up. systemd supervises the unit.

The unit in `deploy/systemd/` and the testbed declaration's `[deploy]` table state these. Tests in
`tests/` hold the three to the same address and token file, and the backup to the unit's binary and
store.

## What stays the consumer's

Anything that differs per consumer or per kind of target is stated in the consumer's registration or
answered through its hook, never held here:

- **Which targets exist,** their clocks, their ranks, their groups and which warm by default.
- **Providers and accounts.** A ping on a provider or an account the consumer will not use now
  answers `skipped`.
- **Locks.** The consumer takes whatever lock keeps a ping off running work, without waiting, and
  answers `busy` when it is held.
- **What a ping says and where it goes.** The prompt, the line it goes into and the login it goes out
  on are the consumer's; the warmer only asks.
- **An expired record.** A target past its lifetime is never due; the consumer forgets its own record
  and retires the target when it reads the record expired.
- **Wording.** Every line a person reads about a target is the reading screen's; the warmer holds only
  the consumer's `label` and codes.
- **Cache lifetimes and alerts.** What a provider publishes is the consumer's declaration; watching
  cache reads is not the warmer's.
