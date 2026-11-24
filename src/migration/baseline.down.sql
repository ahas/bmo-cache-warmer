-- The store's shape at 1.0.0 taken down whole: every table, dependents first.

PRAGMA foreign_keys = OFF;

DROP TABLE "events";
DROP TABLE "targets";
DROP TABLE "subjects";
DROP TABLE "groups";
DROP TABLE "consumers";
DROP TABLE "round";
DROP TABLE "attempts";

PRAGMA foreign_keys = ON;
