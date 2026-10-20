# Boundary API

Every interface by which cache-warmer meets anything outside its own process: the HTTP routes it
serves and the one contract it consumes, the ping hook a consumer answers. Other services match
against this document; the code is one realisation of it. How a pass runs and when a target is due
is `docs/warming.md`.

The warmer is a keepalive scheduler and nothing else. It knows no provider, no model, no account and
no bmo service. A consumer tells it what to keep warm and on which clock, tells it each time a target
is used, and answers when the warmer asks it to warm one; the warmer decides when to ask, holds the
owner's switches and keeps the record of every ping.

## Versioning

The version is the package's in `Cargo.toml` (semver); `VERSION` at the root holds the same. This
document changes in the same commit as the boundary it states.

- **Before the first release the version is `0.1.0` and counts revisions.** The first version, `0.1.0`,
  is the base. Every later version bump is one revision, written `0.1.0-rev.<n>` with `n` counting the
  bumps since the base from 1. Major, minor and patch do not move before the first release, whatever
  the change, a breaking one included.
- **The first release drops the suffix,** and from then on the version moves by semver: a change to a
  contract an existing consumer reads moves the major version, an addition the minor, anything else
  the patch.

## Counterparts

Each contract names today's counterpart by role, not by its code. Any service that registers and
answers the ping hook is a consumer; the warmer needs nothing else of it.

| Role | Today | Uses |
| --- | --- | --- |
| Room consumer | bmo-receiver | registers itself and the chat touch target, reports each turn of the chat agent's session, answers the ping hook |
| Console | bmo-web | lists targets and subjects, throws the round, subject switch and pause |
| Command line | bmo-cli | the same as the console, from a shell |
| Supervisor | systemd, the testbed | `GET /health`; the testbed's backup runs `snapshot` and `verify` |

## Starting the service

The supervisor starts the binary with two settings, each given as an argument or, where the argument
is absent, in the environment. Neither has a default.

| Argument | Environment | Meaning |
| --- | --- | --- |
| `--listen` | `CACHE_WARMER_LISTEN` | the IP address and port to listen on, on loopback |
| `--store` | `CACHE_WARMER_STORE` | the path of the store's SQLite file (`docs/store.md`), created where missing |

A setting missing or not read, an address not on loopback, a store that does not open or a port that
cannot be bound stops the service before it serves anything, with an English sentence on standard
error and a non-zero exit. Once the store is open and the port bound, the routes are served and the
ticker starts (`docs/warming.md`, 「The pass」).

### Snapshot and verify

The same binary takes and checks a snapshot of the store when it is started with a subcommand; with
none, it is the service. What a snapshot holds and how it is judged is `docs/store.md`, 「The
snapshot」. Each setting is given as an argument or, where the argument is absent, in the environment.

| Subcommand | Argument | Environment | Meaning |
| --- | --- | --- | --- |
| `snapshot` | `--store` | `CACHE_WARMER_STORE` | the store's SQLite file to copy |
| `snapshot` | `--into` | `BMO_BACKUP_SNAPSHOT_DIR` | the directory the copy and its manifest are written into, made where missing |
| `verify` | `--from` | `BMO_BACKUP_SNAPSHOT_DIR` | the directory holding a snapshot to check |

- **Each exits 0 when it has done what it says,** with one English sentence on standard output.
- **Anything else exits non-zero** with an English sentence on standard error saying what failed: a
  store that does not open or is not a migrated store, a directory already holding a snapshot, a
  copy that cannot be written; for `verify`, a missing or unreadable manifest, a missing copy, or a
  copy that does not hold to its manifest. A setting missing or not read is refused with the command
  line's usage and exit code 2.
- **Neither serves nor warms, and neither changes the store:** `snapshot` opens it without creating it
  or moving its shape, and `verify` opens the copy read-only.

### The deployed service

- **It listens on `127.0.0.1:9023`.** Consumers, the console and the command line reach it there; no
  sibling repository declares the port.
- **It runs as the user unit `bmo-cache-warmer.service`,** the repository's one service, from the
  release binary in its deploy tree under `~/.local/share/bmo-cache-warmer`.
- **Its store is `warmer.sqlite` in its own data root,** `~/.local/share/bmo-cache-warmer/data`, inside
  no checkout and no release, so a release leaves it alone. The unit makes the folder where it is
  missing.
- **A deployment takes a backup before anything changes,** into
  `~/.local/share/bmo-cache-warmer/backups`, outside the data root. The backup holds a consistent
  snapshot of the store, taken by the live release binary's `snapshot` and held to its manifest by
  that binary's `verify`, beside the rest of the data root packed as it stands. The store file and the
  journal files SQLite keeps beside it are left out of that pack, since a copy read while the service
  writes could be torn; the snapshot is the store's copy in the backup. A deployment that finds no
  release binary, as the first does, takes no snapshot, and its backup passes only while no store
  stands either. Putting a snapshot back is `docs/store.md`, 「Putting a snapshot back」.
- **A deployment waits on `GET /health`** answering 200 before it watches the service stay up.

The unit in `deploy/systemd/` and the testbed declaration's `[deploy]` table state these. Tests in
`tests/` hold the three to the same address, and the backup to the unit's binary and store.

## Conventions

- The service listens on the address it is started with, on loopback.
- Bodies are JSON with snake_case keys. Times are epoch milliseconds; durations are milliseconds and
  their names end in `_ms`. A field marked `?` may be absent or null. A body whose every field may be
  absent may itself be left out.
- A name — a consumer, a target, a subject, a group, in a path or in a body — is any non-empty UTF-8
  string of at most 256 bytes, percent-encoded in a path. Names are compared byte for byte. A target,
  a subject and a group belong to one consumer: the same name under two consumers is two things.
- 200 answers with a body, 201 answers a create with a body, 204 answers with none.
- A word in snake_case is lower-case ASCII letters, digits and underscores, beginning with a letter,
  at most 64 bytes.

### A refused request

A refusal is an HTTP status and a JSON body of exactly two fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `error` | string | one English sentence saying what was wrong |
| `code` | string | a stable snake_case word, listed under 「Codes」 |

A consumer matches on `code`, never on `error`.

## The things it holds

- **A consumer** is a service that has targets warmed. It is known by its name and the hook URL the
  warmer posts to.
- **A target** is one cache prefix the consumer wants kept warm: an agent's session, a prompt line
  several runs share. The consumer names it and declares its clocks.
- **A subject** is what the owner switches: an agent. Each target names its subject; several
  targets may share one (an agent's sessions on several providers). A subject exists once a target
  names it or the owner throws its switch.
- **A group** is a set of targets of which only so many are warmed in one pass (places that share a
  limited number of seats). A target is in at most one group.
- **The round** is the owner's one switch over every target at once.
- **The record** of a target is its last use, its last warmed ping, its last failed attempt, and its
  last attempt with its outcome.

## Served routes

### GET `/health`

| Field | Type | Meaning |
| --- | --- | --- |
| `ok` | bool | true once the store is open |
| `service` | string | `cache-warmer` |
| `version` | string | the package version |

500 `database` when the store cannot be read.

### PUT `/consumers/{consumer}`

Registers a consumer or moves its hook.

| Field | Type | Meaning |
| --- | --- | --- |
| `hook_url` | string | an absolute `http` URL the warmer posts pings to |

Answers 201 on a new consumer, 200 on an existing one, with `consumer` and `hook_url`. 400
`invalid_hook_url` when the URL does not parse or is not `http`. Consumers are services on the same
host, so the warmer speaks plain HTTP only.

### PUT `/consumers/{consumer}/groups/{group}`

Declares a group or changes its limit.

| Field | Type | Meaning |
| --- | --- | --- |
| `limit` | int | how many of the group's targets one pass may ping, 0 or more |

Answers 201 or 200 with `consumer`, `group` and `limit`. 404 `unknown_consumer`; 400 `invalid_body`
for a limit below 0.

### DELETE `/consumers/{consumer}/groups/{group}`

Retires a group. 204. 404 `unknown_group`; 409 `group_in_use` while a target names it.

### PUT `/consumers/{consumer}/targets/{target}`

Registers a target or restates one. Restating changes what the registration declares and keeps the
record and the subject's switches; the clocks the next judgement reads are the restated ones.

| Field | Type | Meaning |
| --- | --- | --- |
| `kind` | string | the consumer's snake_case word for what this is; handed back in each ping and shown in the list |
| `label` | string | a line the console shows; the consumer's wording |
| `subject` | string? | the subject the owner switches it by; the target's own name when absent |
| `rank` | int | its place in the pass: lower goes first |
| `idle_ms` | int | how long it may stand idle before a ping is due, above 0 |
| `lifetime_ms` | int? | how long the provider keeps the prefix after its last touch; none when no lifetime is declared |
| `margin_ms` | int | taken off `lifetime_ms` before judging, 0 or more; 0 when absent |
| `ping_bound_ms` | int | how long the consumer may take to answer one ping, above 0 |
| `group` | string? | the group it is in |
| `warm_by_default` | bool? | whether it warms while its subject's switch is unset; the policy's default, off, when absent |
| `failure_touches` | bool | whether a failed attempt counts as a touch; false when absent |
| `honours_idle_override` | bool | whether a pass run with an idle override applies it here; true when absent |
| `last_used_at` | int? | when it was last used, where the consumer knows; taken only where later than the record's |

Answers 201 on a new target, 200 on a restated one, with the target as `GET /targets` lists it. 404
`unknown_consumer`; 409 `unknown_group` when `group` names no group of the consumer; 400
`invalid_clock` when `idle_ms` or `ping_bound_ms` is not above 0, or `lifetime_ms` or `margin_ms` is
below 0; 400 `invalid_kind` when `kind` is not snake_case.

A lifetime no larger than its margin is accepted: such a target is never due.

### DELETE `/consumers/{consumer}/targets/{target}`

Retires a target and its record. 204. 404 `unknown_target`. A subject left with no target keeps its
switches.

### POST `/consumers/{consumer}/targets/{target}/used`

Reports that the target's prefix was used.

| Field | Type | Meaning |
| --- | --- | --- |
| `at` | int? | when the prefix was last written; the moment the report arrives when absent |
| `started_at` | int? | when the work that used it began; `at` when absent |

The record's last use moves to `at` where that is later than it. A pause on the target's subject set
before `started_at` is cleared; one set after it stands, so work already running when the owner
paused does not undo the pause. 204. 404 `unknown_target`.

### GET `/targets`

Answers 200 with `targets`, every registered target, ordered by rank, then consumer, then target
name. Query `consumer` and `subject` narrow it; 400 `invalid_name` when the query does not read.

Each entry:

| Field | Type | Meaning |
| --- | --- | --- |
| `consumer` | string | |
| `target` | string | |
| `kind`, `label`, `subject`, `rank`, `group?` | | as registered |
| `idle_ms`, `lifetime_ms?`, `margin_ms`, `ping_bound_ms` | int | as registered |
| `warm_by_default` | bool | as registered, or the policy's default |
| `failure_touches`, `honours_idle_override` | bool | as registered |
| `on` | bool | whether it warms now: the subject's switch where set, else `warm_by_default` |
| `paused_at` | int? | when its subject was paused, while that stands |
| `last_used_at` | int? | the record's last use |
| `last_warmed_at` | int? | when its last warmed answer arrived |
| `last_failed_at` | int? | when its last failed attempt ended, whatever came after it |
| `last_attempt` | object? | `at`, `outcome`, `code?`, `reason?` of its last attempt |
| `due_at` | int? | when it is next due by its own clocks; the moment of the list when it was never touched; null when it is off, paused, past its lifetime until it is used again, or when its lifetime less its margin leaves no room for its idle stretch |

`due_at` reads the target alone: a group's limit and the round may still pass it over.

### GET `/consumers/{consumer}/subjects`

Answers 200 with `subjects`, the consumer's subjects by name, each with `subject`, `switch` (`on`,
`off`, or null while unset), `paused_at?`, and `targets`, how many targets name it. 404
`unknown_consumer`.

### PUT `/consumers/{consumer}/subjects/{subject}/switch`

The owner's persistent switch for a subject.

| Field | Type | Meaning |
| --- | --- | --- |
| `on` | bool? | true or false throws it; null returns every target of the subject to its registration's default |

Answers 200 with the subject as listed. A subject not yet named by any target is created, so an
agent can be turned on before its first session is registered. 404 `unknown_consumer`.

Turning a subject off leaves its targets' records where they stood; turning it back on judges them
from those records.

### POST `/consumers/{consumer}/subjects/{subject}/pause`

Turns the subject off until its next use: sets `paused_at` to now, and answers 200 with the subject
as listed. Only a use report clears it (`POST …/used` above); it never changes the switch, which
still wins while it is off. Pausing again moves `paused_at`. 404 `unknown_consumer`.

### GET `/round`

`stopped`, bool: whether the owner has stopped every warming at once.

### PUT `/round`

| Field | Type | Meaning |
| --- | --- | --- |
| `stopped` | bool | true stops the round, false lets it run |

Answers 200 with `stopped`. A pass already running sees it before its next ping.

### POST `/pass`

Runs one pass now and answers once it ends.

| Field | Type | Meaning |
| --- | --- | --- |
| `idle_override_ms` | int? | used instead of `idle_ms` for every target that honours it, above 0 |

Answers 200 with `stopped`, whether the pass found the round stopped, at its start or before one of
its pings, so what remained was not pinged, and `attempts`, one entry per ping in the order they
went: `consumer`, `target`, `outcome`, `code?`, `reason?`. 409
`pass_running` while a pass, the ticker's or another caller's, is running; 400 `invalid_clock` for an
override not above 0.

## The ping hook

The one contract the warmer consumes. To warm a target, the warmer posts to its consumer's
`hook_url` directly, through no proxy, and follows no redirect:

| Field | Type | Meaning |
| --- | --- | --- |
| `consumer` | string | the consumer's name |
| `target` | string | the target's name |
| `kind` | string | as registered |
| `subject` | string | as registered |
| `bound_ms` | int | the target's `ping_bound_ms`: how long the consumer has to answer |

The warmer waits `bound_ms` plus the policy's slack. The consumer does the warming — puts the ping on
the line the target's work runs on — and answers 200 with:

| Field | Type | Meaning |
| --- | --- | --- |
| `outcome` | string | `warmed`, `busy`, `skipped` or `failed` |
| `code` | string? | for `skipped`, required: a snake_case word saying why; for `failed`, optional |
| `reason` | string? | for `failed`, required: an English sentence |

- **`warmed`:** the prefix was read and written again. The record's last warmed time becomes the
  moment the answer arrived; this is the only answer that restarts the target's clock.
- **`busy`:** work was running on the target, which keeps the prefix warm itself, so no ping went.
- **`skipped`:** the consumer chose not to ping now — its provider is off, its account is resting or
  closed, its record is gone. The `code` is the consumer's own word; the warmer stores it unread.
- **`failed`:** a ping went and did not warm. The reason is cut to the policy's length, counted in
  UTF-16 units.

Busy and skipped answers move nothing but the last attempt. A failed one moves the target's touch
only where it registered `failure_touches` (`docs/warming.md`).

When the hook does not answer in time, cannot be reached, answers another status, or answers a body
that does not read as above, the warmer records `failed` with the matching code below and an English
reason of its own.

## Codes

Refusals:

| Code | Status | Meaning |
| --- | --- | --- |
| `invalid_body` | 400 | the body is not JSON, misses a required field, or has a field of the wrong type |
| `invalid_name` | 400 | a name in the path or the body is empty or longer than 256 bytes, or the query naming them does not read |
| `invalid_hook_url` | 400 | the hook URL does not parse or is not `http` |
| `invalid_kind` | 400 | the kind is not snake_case |
| `invalid_clock` | 400 | a duration is out of its range |
| `unknown_consumer` | 404 | no consumer by that name |
| `unknown_target` | 404 | no target by that name under that consumer |
| `unknown_group` | 404 or 409 | no group by that name: 404 retiring it, 409 when a target names it |
| `group_in_use` | 409 | a target still names the group |
| `pass_running` | 409 | a pass is already running |
| `unknown_route` | 404 or 405 | no route serves the path, 404, or none serves it under the method, 405 |
| `database` | 500 | the store could not be read or written |

Outcomes recorded on an attempt: `warmed`, `busy`, `skipped`, `failed`. Codes the warmer writes on a
failed attempt of its own judging:

| Code | Meaning |
| --- | --- |
| `hook_timeout` | the hook did not answer within its bound and the slack |
| `hook_unreachable` | the hook URL could not be connected to, or the request broke off before an answer came |
| `hook_status` | the hook answered a status other than 200, a redirect included |
| `hook_answer` | the hook's body did not read as an answer, or a skipped answer carried no snake_case code, or a failed one no reason |

## Consumers and their targets

What each consumer registers today. The consumer owns every value in this table: it reads them from
its own policy and its providers' declarations and states them in each registration, so a consumer
that comes later adds its own rows and the warmer does not change.

| Kind | Consumer | Subject | Clocks | Bound | Group | Default | Failure touches | Idle override |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Chat touch: the chat agent's one session, in the room it last stood in | receiver | the chat agent | idle 40 min, lifetime 60 min, margin 0 | 30 min | none | on | no | honoured |

Ranks order the pass across every consumer's targets; the chat touch registers 90.

What stays the consumer's, answered through the hook or through what it registers:

- **Which subjects warm by default.** A registration states its own default; the owner turns a
  subject on or off by its switch.
- **Providers and accounts.** A ping on a provider or an account the consumer will not use now
  answers `skipped`. A provider's switch is the consumer's, not the warmer's.
- **Locks.** The consumer takes whatever lock keeps a ping off a running turn, without waiting, and
  answers `busy` when it is held.
- **The chat's state.** A chat touch while any room is in a conversation or a compaction answers
  `busy`; one while the chat agent keeps a session per room, or Claude Code is off, resting or down,
  answers `skipped`.
- **What a ping says and where it goes.** The keepalive prompt, the line it goes into and the login it
  goes out on are the consumer's.
- **An expired record.** A target past its lifetime is never due; the consumer forgets its own
  record and retires the target when it reads the record expired.

## What is deliberately not here

- **Logins.** Walking, watching and warming a provider's accounts is the consumer's.
- **Cache lifetimes and alerts.** What a provider publishes is the consumer's declaration; watching
  cache reads and alerting on them is cache-siren's.
- **Wording.** Every line a person reads about a target is the console's language pack; the warmer
  holds only the consumer's `label` and codes.
- **The ping itself.** What a ping says, which program it goes into and how its answer is read is the
  consumer's; the warmer only asks.
