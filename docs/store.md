# The store

What the warmer keeps between passes and restarts, how that shape moves, and how a copy of it is
taken, checked and put back. The things named here —
consumers, targets, subjects, groups, the round, the record — are `docs/api.md`'s; what the pass does
with them is `docs/warming.md`'s.

## The file

- **One SQLite file,** at the path the service is started with. It is created when missing, and the
  migrations are run up to the latest before anything is served or warmed.
- **Times** are epoch milliseconds, durations milliseconds, switches 0 or 1, as on the wire.
- **The store holds no clock.** Every time it writes is handed to it by the caller, so a test drives
  it on a clock of its own.

## The tables

- **Ids.** Each consumer, group, subject and target row has an integer id the store gives it as it
  is made, which only grows and never moves, so a page read by ids reads the same across restarts
  and across a snapshot put back. Restating a member keeps its id.
- **Consumers:** each consumer's name, hook URL and the time it was first registered.
- **Groups:** each group's consumer, name and limit. A group a target names is not retired.
- **Subjects:** each subject's consumer, name, switch (on, off or unset) and its pause, as the time it
  was set and the time it ends, while that stands. A subject is made the first time a target names it
  or a switch or pause is set on it, and stays when its last target is retired, so its switch is kept.
- **Targets:** what each registration declares — kind, label, subject, rank, clocks, group, default,
  failure touches, idle override — beside its record: last use, last warmed answer, last failed
  attempt, and the last attempt's time, outcome, code and reason. Restating a registration changes
  only what it declares; the record stays, and a stated last use is taken only where it is later.
  Retiring a target removes its record and its attempts.
- **Attempts:** the attempt log, one row per ping — consumer, target, time, outcome, code, reason.
  An attempt stays for the policy's `log_days`; each pass, as it ends, removes what is older, a pass
  that found the round stopped included.
- **Events:** the event window, one row per event — its number, kind, time and the member it carries
  as JSON. Numbers only grow, including across events removed. An event stays for the policy's
  `event_days`; each pass, as it ends, removes what is older.
- **The round:** one row, whether the round is running and the interval set, none while the policy's
  tick stands. A new store starts with the round running and no interval set.

The store holds each table's own rules where the database can: a clock out of its range, a switch
other than 0 or 1, an outcome not one of the four, a target naming a group or subject its consumer
does not have.

## How the shape moves

- **Only by a migration.** Each migration is one step with its way up and its way down, run in order
  and recorded in the store, so a store opened by a newer service is brought up to it and one step
  can be undone.
- **A released migration is never edited.** A change to the shape is a new migration after the last.

## The snapshot

A snapshot is a consistent copy of the store with a manifest beside it, taken and checked by the
binary's `snapshot` and `verify` (`docs/api.md`, 「Snapshot and verify」). A deployment's backup
carries one (`docs/api.md`, 「The deployed service」).

- **How it is taken:** the store is opened through the service's own SQLite library, without creating
  it or running a migration, and SQLite's `VACUUM INTO` writes the copy, `warmer.sqlite`, into the
  snapshot directory. `VACUUM INTO` reads the store inside one read transaction, so the copy is the
  store as one moment left it, while the service goes on serving; no `sqlite3` program is needed. A
  directory that already holds a copy or a manifest is refused, and nothing is written into it.
- **What the manifest holds:** `manifest.json`, written after the copy, from the copy: the latest
  migration recorded in it, the row count of each of its tables by name, and the SHA-256 of the copy
  file in hex.
- **A store that does not stand** is not created: the manifest then says no store stood, and no copy
  is written. A file that holds no migration record is not a store and is refused.
- **How `verify` judges it:** the manifest must read; where it says no store stood, no copy may stand
  either. Otherwise the copy must stand, hash to the manifest's SHA-256, pass SQLite's
  `integrity_check` opened read-only, and record the same latest migration and the same tables with
  the same row counts. The first thing that does not hold is named in the refusal.

## Putting a snapshot back

A snapshot is put back in place of the store, never merged into it:

1. Take the snapshot directory out of the backup and run `verify` on it; a snapshot that does not
   verify is not put back.
2. Stop the service, so nothing holds the store open.
3. Replace the store file with the snapshot's copy, and remove the journal files SQLite keeps beside
   the store (`-journal`, `-wal`, `-shm`), which belong to the file replaced.
4. Start the release whose migrations reach the manifest's latest migration, the one deployed when
   the backup was taken or a newer one; a newer release moves the copy's shape up as it opens it. A
   release older than the copy is not started on it, since its migrations cannot read a shape they
   do not know.
