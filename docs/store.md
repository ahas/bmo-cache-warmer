# The store

What the warmer keeps between passes and restarts, and how that shape moves. The things named here —
consumers, targets, subjects, groups, the round, the record — are `docs/api.md`'s; what the pass does
with them is `docs/warming.md`'s.

## The file

- **One SQLite file,** at the path the service is started with. It is created when missing, and the
  migrations are run up to the latest before anything is served or warmed.
- **Times** are epoch milliseconds, durations milliseconds, switches 0 or 1, as on the wire.
- **The store holds no clock.** Every time it writes is handed to it by the caller, so a test drives
  it on a clock of its own.

## The tables

- **Consumers:** each consumer's name and hook URL.
- **Groups:** each group's consumer, name and limit. A group a target names is not retired.
- **Subjects:** each subject's consumer, name, switch (on, off or unset) and the time its pause was
  set, while that stands. A subject is made the first time a target names it or the owner throws its
  switch or pauses it, and stays when its last target is retired, so its switch is kept.
- **Targets:** what each registration declares — kind, label, subject, rank, clocks, group, default,
  failure touches, idle override — beside its record: last use, last warmed answer, last failed
  attempt, and the last attempt's time, outcome, code and reason. Restating a registration changes
  only what it declares; the record stays, and a stated last use is taken only where it is later.
  Retiring a target removes its record and its attempts.
- **Attempts:** the attempt log, one row per ping — consumer, target, time, outcome, code, reason.
  An attempt stays for the policy's `log_days`; each pass, as it ends, removes what is older.
- **The round:** one row, whether the owner has stopped the round. A new store starts with the round
  running.

The store holds each table's own rules where the database can: a clock out of its range, a switch
other than 0 or 1, an outcome not one of the four, a target naming a group or subject its consumer
does not have.

## How the shape moves

- **Only by a migration.** Each migration is one step with its way up and its way down, run in order
  and recorded in the store, so a store opened by a newer service is brought up to it and one step
  can be undone.
- **A released migration is never edited.** A change to the shape is a new migration after the last.
