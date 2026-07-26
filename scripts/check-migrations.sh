#!/usr/bin/env bash
# check-migrations.sh — guards the migration filenames that sqlx depends on.
#
# Why this exists: sqlx keys applied migrations by the numeric version prefix of
# the filename. Two files sharing a version means one of them is either skipped
# or errors at startup, depending on which one the target database has already
# applied. A fresh CI database applies both happily, so the failure only appears
# on a long-lived database — i.e. production, after CI has gone green. That is
# exactly how PR #244 stopped prod from booting.
#
# The two migration directories must stay in lockstep. Every schema change ships
# to both Postgres and SQLite (docs/CODING_STANDARDS.md), so a version present in
# only one directory — or the same version under two different filenames — yields
# a schema that boots on one backend and dies on the other. If a dialect-only
# migration is ever genuinely needed, the fix is to add an empty no-op .sql with
# the matching filename in the other directory, not to weaken this check: sqlx
# tracks versions per database, and a gap in one dialect's sequence is a
# permanent divergence that no later migration can reconcile.
#
# Run it locally before pushing:  scripts/check-migrations.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Must match the paths compiled into crates/trakkt-core/src/db.rs.
PG_DIR="apps/server/migrations"
SQLITE_DIR="apps/server/migrations-sqlite"

failed=0

fail() {
    echo "::error::$1"
    failed=1
}

# Bare filenames of the .sql migrations in a directory, byte-order sorted.
list_migrations() {
    find "$REPO_ROOT/$1" -maxdepth 1 -type f -name '*.sql' -exec basename {} \; \
        | LC_ALL=C sort
}

# Filenames in $2 (newline-separated) whose version prefix is $1.
files_for_version() {
    printf '%s\n' "$2" | grep "^$1_" | tr '\n' ' ' | sed 's/ *$//'
}

# ---------------------------------------------------------------------------
# Per-directory checks: the directory is usable, filenames parse, versions are
# unique.
# ---------------------------------------------------------------------------
check_directory() {
    local dir="$1"
    local files malformed dupes version

    if [ ! -d "$REPO_ROOT/$dir" ]; then
        fail "Migration directory not found: $dir — does crates/trakkt-core/src/db.rs still point here?"
        return 1
    fi

    files="$(list_migrations "$dir")"

    if [ -z "$files" ]; then
        fail "No .sql migrations found in $dir — this check cannot guard a directory it can't see."
        return 1
    fi

    # sqlx ignores files it can't parse as <version>_<description>.sql, so a
    # malformed name means the migration silently never runs.
    malformed="$(printf '%s\n' "$files" | grep -Ev '^[0-9]+_.+\.sql$' || true)"
    if [ -n "$malformed" ]; then
        while IFS= read -r name; do
            fail "Malformed migration filename in $dir: $name (sqlx expects <version>_<description>.sql)"
        done <<<"$malformed"
        # Version extraction below is meaningless for these, so stop here.
        return 1
    fi

    dupes="$(printf '%s\n' "$files" | cut -d_ -f1 | uniq -d)"
    if [ -n "$dupes" ]; then
        while IFS= read -r version; do
            fail "Duplicate migration version $version in $dir: $(files_for_version "$version" "$files")"
        done <<<"$dupes"
    fi

    # Explicit: without this the function would return the status of the `if`
    # above, which is non-zero precisely when there are NO duplicates.
    return 0
}

# A directory that is missing, empty, or full of unparseable names cannot be
# meaningfully compared against the other one — running the cross-check anyway
# reports every file in the healthy directory as unpaired, burying the one line
# that says what is actually wrong.
pg_usable=1
sqlite_usable=1
check_directory "$PG_DIR" || pg_usable=0
check_directory "$SQLITE_DIR" || sqlite_usable=0

# ---------------------------------------------------------------------------
# Cross-directory check: Postgres and SQLite must carry the same migrations
# under the same filenames.
# ---------------------------------------------------------------------------
if [ "$pg_usable" -eq 1 ] && [ "$sqlite_usable" -eq 1 ]; then
    pg_files="$(list_migrations "$PG_DIR")"
    sqlite_files="$(list_migrations "$SQLITE_DIR")"

    if [ "$pg_files" != "$sqlite_files" ]; then
        # Filenames on one side only. Comparing full filenames rather than bare
        # versions also catches a version that was renamed in one directory and
        # not the other.
        pg_only="$(comm -23 <(printf '%s\n' "$pg_files") <(printf '%s\n' "$sqlite_files"))"
        sqlite_only="$(comm -13 <(printf '%s\n' "$pg_files") <(printf '%s\n' "$sqlite_files"))"

        pg_only_versions="$(printf '%s\n' "$pg_only" | grep -o '^[0-9]*' | LC_ALL=C sort -u || true)"
        sqlite_only_versions="$(printf '%s\n' "$sqlite_only" | grep -o '^[0-9]*' | LC_ALL=C sort -u || true)"

        # A version on both "only" lists exists in both directories under
        # different filenames — a rename applied to one side only.
        renamed="$(comm -12 <(printf '%s\n' "$pg_only_versions") <(printf '%s\n' "$sqlite_only_versions"))"

        if [ -n "$renamed" ]; then
            while IFS= read -r version; do
                fail "Migration version $version has different filenames in each directory: $PG_DIR/$(files_for_version "$version" "$pg_files") vs $SQLITE_DIR/$(files_for_version "$version" "$sqlite_files")"
            done <<<"$renamed"
        fi

        # Everything else on an "only" list is a migration with no counterpart
        # at all. Reported per filename, since a directory can hold several
        # files for one version when it also has a duplicate.
        report_unpaired() {
            local only_list="$1" present_in="$2" absent_from="$3" name version
            while IFS= read -r name; do
                [ -n "$name" ] || continue
                version="${name%%_*}"
                if [ -n "$renamed" ] && printf '%s\n' "$renamed" | grep -qx "$version"; then
                    continue
                fi
                fail "Migration $name exists in $present_in but has no counterpart in $absent_from"
            done <<<"$only_list"
        }

        report_unpaired "$pg_only" "$PG_DIR" "$SQLITE_DIR"
        report_unpaired "$sqlite_only" "$SQLITE_DIR" "$PG_DIR"
    fi
fi

if [ "$failed" -ne 0 ]; then
    echo ""
    echo "Migration filename check failed. Postgres and SQLite migrations must carry"
    echo "identical, unique version prefixes — see the comment at the top of"
    echo "scripts/check-migrations.sh for why."
    exit 1
fi

echo "✅ Migrations are consistent: $(list_migrations "$PG_DIR" | wc -l | tr -d ' ') versions, unique and matched across $PG_DIR and $SQLITE_DIR."
