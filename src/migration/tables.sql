-- The store's tables (docs/store.md, "The tables"). Times are epoch milliseconds, switches 0 or 1.

CREATE TABLE "consumers" (
  "name" TEXT NOT NULL PRIMARY KEY,
  "hook_url" TEXT NOT NULL
);

CREATE TABLE "groups" (
  "consumer" TEXT NOT NULL REFERENCES "consumers" ("name"),
  "name" TEXT NOT NULL,
  "limit" INTEGER NOT NULL CHECK ("limit" >= 0),
  PRIMARY KEY ("consumer", "name")
);

CREATE TABLE "subjects" (
  "consumer" TEXT NOT NULL REFERENCES "consumers" ("name"),
  "name" TEXT NOT NULL,
  "switch" INTEGER CHECK ("switch" IN (0, 1)),
  "paused_at" INTEGER,
  PRIMARY KEY ("consumer", "name")
);

CREATE TABLE "targets" (
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
);

INSERT INTO "round" ("only", "stopped") VALUES (1, 0);
