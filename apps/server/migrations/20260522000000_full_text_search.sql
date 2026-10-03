-- Full-text search: add tsvector columns with GIN indexes to issues and comments.
-- Postgres triggers keep vectors up-to-date on INSERT/UPDATE automatically.

-- ─────────────────────────────────────────────────────────────────────────────
-- 1. Issues: tsvector column + GIN index
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE public.issues ADD COLUMN IF NOT EXISTS search_vector tsvector;

CREATE INDEX IF NOT EXISTS idx_issues_search_vector
    ON public.issues USING GIN (search_vector);

-- ─────────────────────────────────────────────────────────────────────────────
-- 2. Issues: trigger function + trigger
-- ─────────────────────────────────────────────────────────────────────────────

CREATE OR REPLACE FUNCTION issues_search_vector_update() RETURNS trigger AS $$
BEGIN
    NEW.search_vector := to_tsvector('english', coalesce(NEW.title, '') || ' ' || coalesce(NEW.description, ''));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS issues_search_vector_trigger ON public.issues;
CREATE TRIGGER issues_search_vector_trigger
    BEFORE INSERT OR UPDATE OF title, description ON public.issues
    FOR EACH ROW EXECUTE FUNCTION issues_search_vector_update();

-- ─────────────────────────────────────────────────────────────────────────────
-- 3. Comments: tsvector column + GIN index
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE public.comments ADD COLUMN IF NOT EXISTS search_vector tsvector;

CREATE INDEX IF NOT EXISTS idx_comments_search_vector
    ON public.comments USING GIN (search_vector);

-- ─────────────────────────────────────────────────────────────────────────────
-- 4. Comments: trigger function + trigger
-- ─────────────────────────────────────────────────────────────────────────────

CREATE OR REPLACE FUNCTION comments_search_vector_update() RETURNS trigger AS $$
BEGIN
    NEW.search_vector := to_tsvector('english', coalesce(NEW.body, ''));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS comments_search_vector_trigger ON public.comments;
CREATE TRIGGER comments_search_vector_trigger
    BEFORE INSERT OR UPDATE OF body ON public.comments
    FOR EACH ROW EXECUTE FUNCTION comments_search_vector_update();

-- ─────────────────────────────────────────────────────────────────────────────
-- 5. Backfill existing data
-- ─────────────────────────────────────────────────────────────────────────────

UPDATE issues SET search_vector = to_tsvector('english', coalesce(title, '') || ' ' || coalesce(description, ''));
UPDATE comments SET search_vector = to_tsvector('english', coalesce(body, ''));
