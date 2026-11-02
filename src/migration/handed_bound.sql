-- A target's ping stretch is the consumer's own, declared or not (docs/store.md, "The tables"): the
-- column is made again beside the old without its requirement, its values carried, the old dropped and
-- the new renamed.

ALTER TABLE "targets" ADD COLUMN "ping_bound_ms_next" INTEGER CHECK ("ping_bound_ms_next" > 0);
UPDATE "targets" SET "ping_bound_ms_next" = "ping_bound_ms";
ALTER TABLE "targets" DROP COLUMN "ping_bound_ms";
ALTER TABLE "targets" RENAME COLUMN "ping_bound_ms_next" TO "ping_bound_ms";
