-- Full-text search on SQLite uses LIKE fallback — no schema changes required.
-- Postgres uses tsvector columns with GIN indexes (see Postgres migration).
SELECT 1;
