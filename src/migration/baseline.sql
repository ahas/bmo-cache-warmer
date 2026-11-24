-- The store's shape at 1.0.0, whole (docs/store.md, "How the shape moves"), and the round's one row.

CREATE TABLE "attempts" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "consumer" TEXT NOT NULL,
  "target" TEXT NOT NULL,
  "at" INTEGER NOT NULL,
  "outcome" TEXT NOT NULL CHECK ("outcome" IN ('warmed', 'busy', 'skipped', 'failed')),
  "code" TEXT,
  "reason" TEXT,
  FOREIGN KEY ("consumer", "target") REFERENCES "targets" ("consumer", "name") ON DELETE CASCADE
);

CREATE INDEX "attempts_by_target" ON "attempts" ("consumer", "target");

CREATE INDEX "attempts_by_time" ON "attempts" ("at");

CREATE TABLE "round" (
  "only" INTEGER NOT NULL PRIMARY KEY CHECK ("only" = 1),
  "stopped" INTEGER NOT NULL CHECK ("stopped" IN (0, 1))
, "interval_ms" INTEGER CHECK ("interval_ms" > 0));

CREATE TABLE "consumers" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "name" TEXT NOT NULL UNIQUE,
  "url" TEXT NOT NULL,
  "created_at" INTEGER NOT NULL
);

CREATE TABLE "groups" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "consumer" TEXT NOT NULL REFERENCES "consumers" ("name"),
  "name" TEXT NOT NULL,
  "limit" INTEGER NOT NULL CHECK ("limit" >= 0),
  UNIQUE ("consumer", "name")
);

CREATE TABLE "subjects" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "consumer" TEXT NOT NULL REFERENCES "consumers" ("name"),
  "name" TEXT NOT NULL,
  "switch" INTEGER CHECK ("switch" IN (0, 1)),
  "paused_at" INTEGER,
  "paused_until" INTEGER,
  CHECK (("paused_at" IS NULL) = ("paused_until" IS NULL)),
  UNIQUE ("consumer", "name")
);

CREATE TABLE "targets" (
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
  "attempt_reason" TEXT, "ping_bound_ms" INTEGER CHECK ("ping_bound_ms" > 0),
  UNIQUE ("consumer", "name"),
  FOREIGN KEY ("consumer", "subject") REFERENCES "subjects" ("consumer", "name"),
  FOREIGN KEY ("consumer", "group") REFERENCES "groups" ("consumer", "name")
);

CREATE TABLE "events" (
  "id" INTEGER PRIMARY KEY AUTOINCREMENT,
  "kind" TEXT NOT NULL,
  "at" INTEGER NOT NULL,
  "data" TEXT NOT NULL
);

CREATE INDEX "events_by_time" ON "events" ("at");

INSERT INTO "round" ("only", "stopped") VALUES (1, 0);
