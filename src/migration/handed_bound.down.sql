-- A target's ping stretch required again, as the shape below this one holds it: a target that declared
-- none takes two minutes.

ALTER TABLE "targets" ADD COLUMN "ping_bound_ms_next" INTEGER NOT NULL DEFAULT 120000
  CHECK ("ping_bound_ms_next" > 0);
UPDATE "targets" SET "ping_bound_ms_next" = COALESCE("ping_bound_ms", 120000);
ALTER TABLE "targets" DROP COLUMN "ping_bound_ms";
ALTER TABLE "targets" RENAME COLUMN "ping_bound_ms_next" TO "ping_bound_ms";
