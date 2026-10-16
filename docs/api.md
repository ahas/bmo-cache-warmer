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

The version is the package's in `Cargo.toml` (semver), starting at 0.1.0; `VERSION` at the root holds
the same. This document changes in the same
commit as the boundary it states. A change to a contract an existing consumer reads — a route, a
field, its type or meaning, a code — moves the major version; an addition no existing consumer reads
moves the minor; anything else moves the patch.

## Counterparts

Each contract names today's counterpart by role, not by its code.

| Role | Today | Uses |
| --- | --- | --- |
| Session consumer | bmo-agency | registers itself and its targets (worker sessions, stand-in roads, launch lines, editors, gates, pool places), reports their use, answers the ping hook |
| Room consumer | bmo-receiver | registers itself and the chat touch target, reports each turn of the chat agent's session, answers the ping hook |
| Console | bmo-web | lists targets and subjects, throws the round, subject switch and pause |
| Command line | bmo-cli | the same as the console, from a shell |
| Supervisor | systemd, the testbed | `GET /health` |

## Conventions

- The service listens on the address it is started with, on loopback; the testbed declaration names
  the deployed port.
- Bodies are JSON with snake_case keys. Times are epoch milliseconds; durations are milliseconds and
  their names end in `_ms`. A field marked `?` may be absent or null.
- A name in a path — a consumer, a target, a subject, a group — is any non-empty UTF-8 string of at
  most 256 bytes, percent-encoded in the path. Names are compared byte for byte. A target, a subject
  and a group belong to one consumer: the same name under two consumers is two things.
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
- **A target** is one cache prefix the consumer wants kept warm: a worker's session, a launch line's
  session, a pool place. The consumer names it and declares its clocks.
- **A subject** is what the owner switches: an agent. Each target names its subject; several
  targets may share one (a worker's sessions on several providers). A subject exists once a target
  names it or the owner throws its switch.
- **A group** is a set of targets of which only so many are warmed in one pass (a pool reservation's
  places, limited to its seats). A target is in at most one group.
- **The round** is the owner's one switch over every target at once.
- **The record** of a target is its last use, its last warmed ping, and its last attempt with its
  outcome.

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
| `hook_url` | string | an absolute `http` or `https` URL the warmer posts pings to |

Answers 201 on a new consumer, 200 on an existing one, with `consumer` and `hook_url`. 400
`invalid_hook_url` when the URL does not parse or is not `http` or `https`.

### PUT `/consumers/{consumer}/groups/{group}`

Declares a group or changes its limit.

| Field | Type | Meaning |
| --- | --- | --- |
| `limit` | int | how many of the group's targets one pass may ping, 0 or more |

Answers 201 or 200 with `consumer`, `group` and `limit`. 404 `unknown_consumer`.

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

Every registered target, ordered by rank, then consumer, then target name. Query `consumer` and
`subject` narrow it.

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
| `last_attempt` | object? | `at`, `outcome`, `code?`, `reason?` of its last attempt |
| `due_at` | int? | when it is next due by its own clocks; null when it is off, paused, or past its lifetime until it is used again |

`due_at` reads the target alone: a group's limit and the round may still pass it over.

### GET `/consumers/{consumer}/subjects`

The consumer's subjects, by name: `subject`, `switch` (`on`, `off`, or null while unset),
`paused_at?`, and `targets`, how many targets name it. 404 `unknown_consumer`.

### PUT `/consumers/{consumer}/subjects/{subject}/switch`

The owner's persistent switch for a subject.

| Field | Type | Meaning |
| --- | --- | --- |
| `on` | bool? | true or false throws it; null returns every target of the subject to its registration's default |

Answers 200 with the subject as listed. A subject not yet named by any target is created, so a
worker can be turned on before its first session is registered. 404 `unknown_consumer`.

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

Answers 200 with `stopped`, whether the round was stopped so nothing was judged, and `attempts`, one
entry per ping in the order they went: `consumer`, `target`, `outcome`, `code?`, `reason?`. 409
`pass_running` while a pass, the ticker's or another caller's, is running; 400 `invalid_clock` for an
override not above 0.

## The ping hook

The one contract the warmer consumes. To warm a target, the warmer posts to its consumer's
`hook_url`:

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
| `invalid_name` | 400 | a name in the path is empty or longer than 256 bytes |
| `invalid_hook_url` | 400 | the hook URL does not parse or is not `http` or `https` |
| `invalid_kind` | 400 | the kind is not snake_case |
| `invalid_clock` | 400 | a duration is out of its range |
| `unknown_consumer` | 404 | no consumer by that name |
| `unknown_target` | 404 | no target by that name under that consumer |
| `unknown_group` | 404 or 409 | no group by that name: 404 retiring it, 409 when a target names it |
| `group_in_use` | 409 | a target still names the group |
| `pass_running` | 409 | a pass is already running |
| `database` | 500 | the store could not be read or written |

Outcomes recorded on an attempt: `warmed`, `busy`, `skipped`, `failed`. Codes the warmer writes on a
failed attempt of its own judging:

| Code | Meaning |
| --- | --- |
| `hook_timeout` | the hook did not answer within its bound and the slack |
| `hook_unreachable` | the hook URL could not be connected to |
| `hook_status` | the hook answered a status other than 200 |
| `hook_answer` | the hook's body did not read as an answer, or a skipped answer carried no snake_case code, or a failed one no reason |

## Consumers and their targets

What each consumer registers, so every kind of prefix the keepalive pass keeps warm is kept warm on
the same clocks. The consumer owns every value in this table; it reads them from its own policy and
provider declarations and states them in each registration. Claude Code's declaration keeps a prefix
60 minutes and pings after 40 idle; Codex's keeps GPT-5.6 and later 30 minutes and pings after 20;
OpenRouter's states both per upstream, and a run on an upstream it does not list is not registered.
A registered model of the owner's own declares no lifetime and is never registered. A native pool
place names no prefix and is never registered.

| Kind | Consumer | Subject | Clocks | Bound | Group | Default | Failure touches | Idle override |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Worker sessions, Claude | agency | the worker | idle 40 min, no lifetime: a held session is resumed as it stands | 120 s | none | off | yes | honoured |
| Worker stand-in roads, Codex and OpenRouter | agency | the worker | the provider's idle and lifetime for the model or upstream | 120 s | none | off | no | honoured |
| Gate stand-in roads | agency | the gate | the provider's idle and lifetime | 90 s | none | on | no | honoured |
| Chat stand-in roads | agency | the chat agent | the provider's idle and lifetime | 90 s | none | on | no | honoured |
| Launch lines | agency | the line's agent | idle 40 min, lifetime 60 min, margin 0 | 90 s | none | on | yes | honoured |
| Editors | agency | the editor, one target per model and effort | idle 40 min, lifetime 60 min, margin 0 | 90 s | none | on | yes | honoured |
| Gates: the output gate, the three reference gates, the situation-rules matcher | agency | the gate | idle 40 min, lifetime 60 min, margin 0 | 90 s | none | on | yes | honoured |
| Pool places, co-work branch and research branch | agency | the reservation's owner | the provider's idle; its lifetime with a 5-minute margin | 120 s branch, 90 s research | the provider, model and effort the places share, its limit the seats of every reservation standing on it | on | no | not honoured |
| Chat touch: the chat agent's one session, in the room it last stood in | receiver | the chat agent | idle 40 min, lifetime 60 min, margin 0 | 30 min | none | on | no | honoured |

Ranks, so the pass keeps its order: pool places 10, worker stand-in roads 20, gate stand-in roads 30,
chat stand-in roads 40, launch lines 50, editors 60, gates 70, worker sessions 80, chat touch 90.

What stays the consumer's, answered through the hook or through what it registers:

- **Which workers warm.** A worker's sessions default off; the owner turns a worker on by its
  subject's switch. Of a base worker's support workers agency registers only the first as many as the
  support keepalive count says, by slot; the rest it never registers.
- **A worker's default model.** A worker with no session on its default model registers that model's
  target with no last use, so it is due at once and the ping brings the session up. A record on
  another model that has expired is retired by agency and left for the next job.
- **Which pool places are candidates.** Agency registers a place only while a reservation stands on
  it and its prefix's fingerprint matches, restates the group's limit as seats change, and retires
  places when reservations release. A place being written by a run answers `busy`.
- **Providers and accounts.** A ping on a provider turned off, a Claude Code ping during a rest, or a
  ping on an account that is closed or resting answers `skipped`; Codex and OpenRouter stand-ins warm
  through a Claude Code rest. A provider's switch is agency's, not the warmer's.
- **Locks.** The consumer takes whatever lock keeps a ping off a running turn, without waiting, and
  answers `busy` when it is held.
- **The chat's state.** A chat touch while any room is in a conversation or a compaction answers
  `busy`; one while the chat agent keeps a session per room, or Claude Code is off, resting or down,
  answers `skipped`.
- **What a ping says and where it goes.** The keepalive prompt, the line it goes into and the login it
  goes out on are the consumer's.
- **An expired record.** A stand-in run or session past its lifetime is never due; the consumer
  forgets its own record and retires the target when it reads the record expired.

## Switch-over

Every contract on the keepalive pass that another part of bmo reads, and what serves it once the
warmer runs.

| Contract | Served by |
| --- | --- |
| `Keepalive::of`, the server's start of the pass | the warmer's own ticker |
| `Keepalive::new`, the one-pass verb | `POST /pass` |
| `ChatTouch::There`, the pass reaching the chat's conversations | the receiver's ping hook for its chat touch targets |
| `TOUCH_PATH`, the chat's touch route | the receiver's ping hook |
| `POST /conversation/touch` with `idleOverrideMs` | `POST /pass` with `idle_override_ms` |
| `keepalive::logins` in the Claude login reader | agency's account watch |
| `keepalive::logins` in the speech of a login's state | agency's account watch |
| `keepalive::logins` in a job's Claude turn | agency's account watch |
| `keepalive::logins` in the account walk | agency's account watch |
| `pool::usable`, the lifetime less margin a pool place reads | agency, reading its own provider declaration; the warmer applies it as `margin_ms` |
| `GET /keepalive`, answering `stopped` | `GET /round` |
| `POST /keepalive` with `stopped` | `PUT /round` |
| `POST /keepalive` with `agent` and `stopped` | `PUT /consumers/{consumer}/subjects/{subject}/switch` |
| `POST /keepalive` with `agent` and `pause` | `POST /consumers/{consumer}/subjects/{subject}/pause` |
| agents column `last_keepalive_at` | the target's record, `last_warmed_at` and `last_attempt` |
| agents column `keepalive_stopped` | the subject's switch |
| agents column `keepalive_paused_at` | the subject's `paused_at` |
| agents column `support_count` | agency: the worker's slots |
| agents column `support_keepalive` | agency: which support workers it registers |
| agents column `support_of` | agency: the worker's slots |
| agents column `support_slot` | agency: the worker's slots |
| `KEEPALIVE`, the pass clocks | the warmer's policy, start delay and tick |
| `KEEPALIVE`, the prompt and ping bounds, read by the chat touch | the receiver's own policy and its `ping_bound_ms` |
| `KEEPALIVE`, read by the gates | agency's policy and its `ping_bound_ms` |
| `KEEPALIVE`, read by the worker sessions | agency's policy and its `ping_bound_ms` |
| `POOL_WARM`, the margin and the place bounds | agency's policy, stated as `margin_ms` and `ping_bound_ms` |
| the `keepalive_stopped` setting | `GET /round` and `PUT /round` |
| the `--pause-keepalive` verb | bmo-cli, calling `POST …/subjects/{subject}/pause` |
| `KeepaliveView` and `KeepaliveChanged` in the console | bmo-web, reading `GET /round`, `GET /targets` and the subject routes |
| `workers::pause_keepalive` | `POST …/subjects/{subject}/pause` |

The `keepalive_stopped` setting is read in three places — the pass, the run list and the console's
switch — and all three read `GET /round`. `KEEPALIVE` is read in four: the pass, the chat touch, the
gates and the worker sessions; each reader is one row above.

## Contract gaps

Statements in sibling documents the warmer makes untrue, each for its owner to restate:

- **bmo-agency's documents** describe the keepalive pass as agency's server's own, and its login watch
  as running on that pass. The pass is the warmer's; the login watch stays agency's and runs on a
  clock of agency's own.
- **bmo-agency's worker switch, pause columns and verbs** — the row switch, `--keepalive off` and
  `--pause-keepalive` — write agency's agents row. They move onto the warmer's subject routes, and
  agency registers its targets and answers the hook.
- **bmo-agency's worker slots** state a base worker's keepalive is on unless the owner turns it off;
  a worker's sessions warm by default off, and the owner turns them on by the switch.
- **bmo-agency's `keepalive.toml`** holds the pass's start delay and tick; those are the warmer's,
  and agency keeps only the prompt and its ping bounds.
- **bmo-receiver** has no boundary document yet; its rooms' chat touch is a consumer of this one.

## What is deliberately not here

- **Logins.** Walking, watching and warming Claude Code accounts is agency's.
- **Cache lifetimes and alerts.** What a provider publishes is the consumer's declaration; watching
  cache reads and alerting on them is cache-siren's.
- **Wording.** Every line a person reads about a target is the console's language pack; the warmer
  holds only the consumer's `label` and codes.
- **The ping itself.** What a ping says, which program it goes into and how its answer is read is the
  consumer's; the warmer only asks.
