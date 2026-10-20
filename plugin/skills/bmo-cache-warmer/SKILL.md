---
name: bmo-cache-warmer
description: Keep a long prompt prefix's provider cache warm between requests with the bmo-cache-warmer MCP server: register the prefix as a target, record each use, and read when it is pinged next. Use when a harness or a service reuses a long cached prompt prefix across requests spaced further apart than the provider keeps its cache, or when asked what the warmer keeps warm and when it pings next.
---

# bmo-cache-warmer

A provider keeps a prompt prefix cached for a while after its last read or write and then drops it. The cache
warmer is a keepalive scheduler: whoever owns a prefix registers it as a **target** under a **consumer** name,
records each use, and serves a hook at the URL the consumer registered; before the cache runs out, the warmer
posts a `target-ping` to that hook, and the consumer sends one cheap request on the prefix to keep it warm. The
warmer knows no provider and sends nothing itself: it only decides when to ask.

## Starting the server

Register the server with the harness as a stdio MCP server:

```
bmo-cache-warmer mcp
```

It reaches the service at `http://127.0.0.1:9023`, or the address `--address` or `CACHE_WARMER_ADDRESS` names.
The service's bearer token goes in `CACHE_WARMER_TOKEN` or `--token`; where neither is set, the server reads the
file `CACHE_WARMER_TOKEN_FILE` or `--token-file` names. It does not start without one. A wrong token answers
every tool with a 401 `unauthorized` error.

## What to do first

1. `warmer_consumers`: the consumers registered, each with the `url` its hook is served at. A target is
   registered under one of them; registering a consumer is its own service's work, through the service's API.
2. `warmer_target_put` with `consumerName`, `targetName` and `ttlMs`, the provider's cache lifetime for the
   prefix in milliseconds. Leave the rest out unless you know better: `idleMs` (how long it may stand idle
   before a ping) defaults to a share of the lifetime, `pingBoundMs` to the warmer's bound, `subjectName` to
   the target's own name. Give `warmByDefault: true` where the target should warm without a switch being
   thrown first; the warmer's default is off. Restating a target keeps its record.
3. `warmer_target_used` after each request that read or wrote the prefix, with `consumerName` and
   `targetName`. `at` (when the prefix was last written) and `startedAt` (when the work began) default to now.
   A use moves the target's clock, so the warmer does not ping a prefix that is in use.
4. `warmer_targets` to see each target's `nextPingAt`, whether it is `on`, and its `lastAttempt`; narrow it with
   `consumerName` or `subjectName`. `nextPingAt` is null while the target is off, paused, or past its lifetime
   until it is used again.

## Reading the switches

- `warmer_subjects` with `consumerName`: each subject's switch `on` (true, false, or null while unset), and its
  pause, `pausedUntil`, while one stands. A subject groups the targets switched together.
- `warmer_round`: the round's `intervalMs`, when the next pass runs (`nextAt`), and whether it is `running` at
  all. While it is not running, nothing is pinged.

Switching a subject, pausing it, and changing the round are the service's routes, not tools; a person's
screen or the consumer's own service makes those changes.

## Lists

A list answers `{"items": [...], "more": boolean}` in id order. Ask the next page with `after` set to the last
item's `id`; `limit` is 1 to 1000, 100 where absent.

## When a tool answers an error

A tool error's text is JSON: `status`, `code` and `error`, the service's English sentence. Match on `code`:

- `not-found`: no consumer or target stands at the names given; list the consumers or targets again.
- `bad-request`: the arguments are not the tool's shape, or a name is empty or over 256 bytes; the sentence
  names what is wrong.
- `invalid-clock`: `ttlMs` or `marginMs` is below 0, or `idleMs` or `pingBoundMs` is not above 0.
- `invalid-kind`: `kind` is not a kebab-case word.
- `unknown-group`: `groupName` names no group of the consumer.

An error with no `code` means the service could not be reached; check the address and that the service runs.

The routes behind each tool are stated in `docs/api.md` and `docs/openapi.json`, whose `x-tools` lists every
tool's input and output; when a target is due is `docs/warming.md`.
