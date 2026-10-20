-- The ids, the pause's end, the round's interval and the event window taken down again, each table made
-- again in its earlier shape the way the way up makes the new one. A pause comes back as standing until the next use,
-- a kind word comes back in snake_case, and a target with no kind or label takes `target` and its own name.

PRAGMA foreign_keys = OFF;

BEGIN;

CREATE TABLE "consumers_prev" (
  "name" TEXT NOT NULL PRIMARY KEY,
  "hook_url" TEXT NOT NULL
);
INSERT INTO "consumers_prev" ("name", "hook_url") SELECT "name", "url" FROM "consumers";

CREATE TABLE "groups_prev" (
  "consumer" TEXT NOT NULL REFERENCES "consumers" ("name"),
  "name" TEXT NOT NULL,
  "limit" INTEGER NOT NULL CHECK ("limit" >= 0),
  PRIMARY KEY ("consumer", "name")
);
INSERT INTO "groups_prev" ("consumer", "name", "limit") SELECT "consumer", "name", "limit" FROM "groups";

CREATE TABLE "subjects_prev" (
  "consumer" TEXT NOT NULL REFERENCES "consumers" ("name"),
  "name" TEXT NOT NULL,
  "switch" INTEGER CHECK ("switch" IN (0, 1)),
  "paused_at" INTEGER,
  PRIMARY KEY ("consumer", "name")
);
INSERT INTO "subjects_prev" ("consumer", "name", "switch", "paused_at")
  SELECT "consumer", "name", "switch", "paused_at" FROM "subjects";

CREATE TABLE "targets_prev" (
  "consumer" TEXT NOT NULL REFERENCES "consumers" ("name"),
  "name" TEXT NOT NULL,
  "kind" TEXT NOT NULL,
  "label" TEXT NOT NULL,
  "subject" TEXT NOT NULL,
  "rank" INTEGER NOT NULL,
  "idle_ms" INTEGER NOT NULL CHECK ("idle_ms" > 0),
  "lifetime_ms" INTEGER CHECK ("lifetime_ms" >= 0),
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
  PRIMARY KEY ("consumer", "name"),
  FOREIGN KEY ("consumer", "subject") REFERENCES "subjects" ("consumer", "name"),
  FOREIGN KEY ("consumer", "group") REFERENCES "groups" ("consumer", "name")
);
INSERT INTO "targets_prev" ("consumer", "name", "kind", "label", "subject", "rank", "idle_ms", "lifetime_ms",
    "margin_ms", "ping_bound_ms", "group", "warm_by_default", "failure_touches", "honours_idle_override",
    "last_used_at", "last_warmed_at", "last_failed_at", "attempt_at", "attempt_outcome", "attempt_code",
    "attempt_reason")
  SELECT "consumer", "name", COALESCE(replace("kind", '-', '_'), 'target'), COALESCE("label", "name"),
         "subject", "rank", "idle_ms", "lifetime_ms", "margin_ms", "ping_bound_ms", "group",
         "warm_by_default", "failure_touches", "honours_idle_override", "last_used_at", "last_warmed_at",
         "last_failed_at", "attempt_at", "attempt_outcome", replace("attempt_code", '-', '_'),
         "attempt_reason"
  FROM "targets";

UPDATE "attempts" SET "code" = replace("code", '-', '_');

DROP TABLE "targets";
DROP TABLE "subjects";
DROP TABLE "groups";
DROP TABLE "consumers";
ALTER TABLE "consumers_prev" RENAME TO "consumers";
ALTER TABLE "groups_prev" RENAME TO "groups";
ALTER TABLE "subjects_prev" RENAME TO "subjects";
ALTER TABLE "targets_prev" RENAME TO "targets";

ALTER TABLE "round" DROP COLUMN "interval_ms";

DROP TABLE "events";

COMMIT;

PRAGMA foreign_keys = ON;
