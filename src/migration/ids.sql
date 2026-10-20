-- Ids for every member, the pause's end, the round's interval and the event window (docs/store.md, "The
-- tables"). A table takes an integer primary key only by being made again, so each is made anew beside
-- the old, its rows carried, the old dropped and the new renamed, with foreign keys off for the swap.
-- A pause carried over stood until the next use, so it ends at the end of time and a use lifts it. A
-- target that declared no lifetime has none to carry, since a registration now declares one; it is
-- retired with its attempts, and its consumer registers it again.

PRAGMA foreign_keys = OFF;

BEGIN;

CREATE TABLE "consumers_next" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "name" TEXT NOT NULL UNIQUE,
  "url" TEXT NOT NULL,
  "created_at" INTEGER NOT NULL
);
INSERT INTO "consumers_next" ("name", "url", "created_at")
  SELECT "name", "hook_url", 0 FROM "consumers" ORDER BY "name";

CREATE TABLE "groups_next" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "consumer" TEXT NOT NULL REFERENCES "consumers" ("name"),
  "name" TEXT NOT NULL,
  "limit" INTEGER NOT NULL CHECK ("limit" >= 0),
  UNIQUE ("consumer", "name")
);
INSERT INTO "groups_next" ("consumer", "name", "limit")
  SELECT "consumer", "name", "limit" FROM "groups" ORDER BY "consumer", "name";

CREATE TABLE "subjects_next" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "consumer" TEXT NOT NULL REFERENCES "consumers" ("name"),
  "name" TEXT NOT NULL,
  "switch" INTEGER CHECK ("switch" IN (0, 1)),
  "paused_at" INTEGER,
  "paused_until" INTEGER,
  CHECK (("paused_at" IS NULL) = ("paused_until" IS NULL)),
  UNIQUE ("consumer", "name")
);
INSERT INTO "subjects_next" ("consumer", "name", "switch", "paused_at", "paused_until")
  SELECT "consumer", "name", "switch", "paused_at",
         CASE WHEN "paused_at" IS NULL THEN NULL ELSE 9223372036854775807 END
  FROM "subjects" ORDER BY "consumer", "name";

CREATE TABLE "targets_next" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "consumer" TEXT NOT NULL REFERENCES "consumers" ("name"),
  "name" TEXT NOT NULL,
  "kind" TEXT,
  "label" TEXT,
  "subject" TEXT NOT NULL,
  "rank" INTEGER NOT NULL,
  "idle_ms" INTEGER NOT NULL CHECK ("idle_ms" > 0),
  "lifetime_ms" INTEGER NOT NULL CHECK ("lifetime_ms" >= 0),
  "margin_ms" INTEGER NOT NULL CHECK ("margin_ms" >= 0),
  "ping_bound_ms" INTEGER NOT NULL CHECK ("ping_bound_ms" > 0),
  "group" TEXT,
  "warm_by_default" INTEGER CHECK ("warm_by_default" IN (0, 1)),
  "failure_touches" INTEGER NOT NULL CHECK ("failure_touches" IN (0, 1)),
  "honours_idle_override" INTEGER NOT NULL CHECK ("honours_idle_override" IN (0, 1)),
  "last_used_at" INTEGER,
  "last_warmed_at" INTEGER,
  "last_failed_at" INTEGER,
  "attempt_at" INTEGER,
  "attempt_outcome" TEXT CHECK ("attempt_outcome" IN ('warmed', 'busy', 'skipped', 'failed')),
  "attempt_code" TEXT,
  "attempt_reason" TEXT,
  UNIQUE ("consumer", "name"),
  FOREIGN KEY ("consumer", "subject") REFERENCES "subjects" ("consumer", "name"),
  FOREIGN KEY ("consumer", "group") REFERENCES "groups" ("consumer", "name")
);
INSERT INTO "targets_next" ("consumer", "name", "kind", "label", "subject", "rank", "idle_ms", "lifetime_ms",
    "margin_ms", "ping_bound_ms", "group", "warm_by_default", "failure_touches", "honours_idle_override",
    "last_used_at", "last_warmed_at", "last_failed_at", "attempt_at", "attempt_outcome", "attempt_code",
    "attempt_reason")
  SELECT "consumer", "name", replace("kind", '_', '-'), "label", "subject", "rank", "idle_ms", "lifetime_ms",
         "margin_ms", "ping_bound_ms", "group", "warm_by_default", "failure_touches", "honours_idle_override",
         "last_used_at", "last_warmed_at", "last_failed_at", "attempt_at", "attempt_outcome",
         replace("attempt_code", '_', '-'), "attempt_reason"
  FROM "targets" WHERE "lifetime_ms" IS NOT NULL ORDER BY "consumer", "name";

DELETE FROM "attempts" WHERE NOT EXISTS (
  SELECT 1 FROM "targets_next" t WHERE t."consumer" = "attempts"."consumer" AND t."name" = "attempts"."target"
);
UPDATE "attempts" SET "code" = replace("code", '_', '-');

DROP TABLE "targets";
DROP TABLE "subjects";
DROP TABLE "groups";
DROP TABLE "consumers";
ALTER TABLE "consumers_next" RENAME TO "consumers";
ALTER TABLE "groups_next" RENAME TO "groups";
ALTER TABLE "subjects_next" RENAME TO "subjects";
ALTER TABLE "targets_next" RENAME TO "targets";

ALTER TABLE "round" ADD COLUMN "interval_ms" INTEGER CHECK ("interval_ms" > 0);

CREATE TABLE "events" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "kind" TEXT NOT NULL,
  "at" INTEGER NOT NULL,
  "data" TEXT NOT NULL
);
CREATE INDEX "events_by_time" ON "events" ("at");

COMMIT;

PRAGMA foreign_keys = ON;
