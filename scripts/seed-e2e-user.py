#!/usr/bin/env python3
"""
Seed an E2E test user into the Trakkt database.

Creates a verified user with a known password, adds them to the first
workspace, and prints the credentials. Idempotent — safe to run multiple times.

Requirements:
  pip3 install argon2-cffi psycopg2-binary

Usage:
  python3 scripts/seed-e2e-user.py
  # Or with custom DB URL:
  DATABASE_URL=postgresql://... python3 scripts/seed-e2e-user.py
"""

import os
import sys
import json
import psycopg2
from argon2 import PasswordHasher

# Port 5435 is Trakkt's *development* rung. That is the right one for a seeding
# script: no Playwright config here declares a `webServer`, so the E2E suites
# point `BASE_URL` at a server the developer started, which is the one running
# on the development database. Not 5436 — that is the dialect suite's
# maintenance server, where every test creates and drops its own throwaway
# `trakkt_test_*` database, so a user seeded there would be read by nobody. The
# full ladder is recorded on
# `test_helpers::dual_backend::DEFAULT_PG_TEST_URL`.
#
# It read `trakkt:password@localhost:5432` until TRA-10002 — the stock Postgres
# default, which is on no rung at all and so resolves to whichever server a
# machine happens to be running. That matters more here than in a config
# default: this script connects on the next line and then writes, so a wrong
# port is a user with a published password inserted into someone else's
# database rather than a value that sits unread.
DATABASE_URL = os.environ.get("DATABASE_URL", "postgresql://trakkt:trakkt@localhost:5435/trakkt")

EMAIL = "e2e-test@trakkt.dev"
PASSWORD = "E2eTestPass123!"
NAME = "E2E Test User"
USER_ID = "user-e2e-test-001"


def main():
    conn = psycopg2.connect(DATABASE_URL)
    conn.autocommit = True
    cur = conn.cursor()

    # Check if user exists
    cur.execute("SELECT user_id FROM users WHERE email = %s", (EMAIL,))
    row = cur.fetchone()

    if row:
        user_id = row[0]
        print(f"[seed] User {EMAIL} already exists (id={user_id})")
    else:
        user_id = USER_ID
        cur.execute(
            """INSERT INTO users (user_id, email, name, verified, active)
               VALUES (%s, %s, %s, true, true)""",
            (user_id, EMAIL, NAME),
        )
        print(f"[seed] Created user {EMAIL} (id={user_id})")

    # Ensure verified
    cur.execute("UPDATE users SET verified = true WHERE user_id = %s", (user_id,))

    # Upsert password auth method
    ph = PasswordHasher()
    password_hash = ph.hash(PASSWORD)
    auth_data = json.dumps({"hash": password_hash})

    cur.execute(
        """INSERT INTO user_auth_methods (user_id, auth_type, auth_data, active)
           VALUES (%s, 'password', %s::json, true)
           ON CONFLICT (user_id, auth_type)
           DO UPDATE SET auth_data = EXCLUDED.auth_data, active = true""",
        (user_id, auth_data),
    )
    print(f"[seed] Password set for {EMAIL}")

    # Add to first workspace if not already a member
    cur.execute("SELECT workspace_id FROM workspaces ORDER BY created_at LIMIT 1")
    ws_row = cur.fetchone()
    if ws_row:
        ws_id = ws_row[0]
        cur.execute(
            """INSERT INTO workspace_users (workspace_id, user_id, role, active)
               VALUES (%s, %s, 'workspace_admin', true)
               ON CONFLICT (workspace_id, user_id) DO NOTHING""",
            (ws_id, user_id),
        )
        cur.execute(
            "UPDATE users SET last_workspace_id = %s WHERE user_id = %s",
            (ws_id, user_id),
        )
        print(f"[seed] Added to workspace {ws_id}")

    cur.close()
    conn.close()

    print(f"\n[seed] Ready — login with:")
    print(f"  Email:    {EMAIL}")
    print(f"  Password: {PASSWORD}")


if __name__ == "__main__":
    main()
