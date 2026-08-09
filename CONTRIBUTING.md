# Contributing to Trakkt

Thanks for your interest in contributing to Trakkt! This document covers what you need to know.

## Contributor License Agreement

Before your first contribution can be merged, you must sign the [Contributor License Agreement](CLA.md). This is required because Trakkt uses a dual-license model (AGPL-3.0 + Commercial), and the CLA ensures Alytic Pty Ltd can continue to offer both licensing options.

To sign:

1. Read [CLA.md](CLA.md)
2. Add your name and details to `signatures/cla.json`
3. Include the CLA signature commit in your first pull request

## Development Setup

### Prerequisites

- Rust (stable)
- PostgreSQL 16+
- Redis (optional — in-memory KV used if not configured)
- [tailwindcss CLI](https://tailwindcss.com/blog/standalone-cli) v4

### Running Locally

```bash
# Clone the repo
git clone https://github.com/kyomi-ai/trakkt.git
cd trakkt

# Copy environment config
cp .env.example .env
# Edit .env with your database credentials

# Run database migrations
cargo run --package trakkt-server -- migrate

# Build the WASM frontend (in one terminal)
cd crates/trakkt-ui && trunk build --watch

# Run the server (in another terminal)
cargo run --package trakkt-server
```

The app runs at `http://localhost:3100` by default.

### Running Tests

```bash
cargo test --workspace
```

### Adding a Migration

Every schema change needs two files with the **same filename** — one in
`apps/server/migrations/` (PostgreSQL) and one in `apps/server/migrations-sqlite/`
(SQLite). sqlx keys applied migrations by the numeric version prefix, so a version
reused by two files, or present in only one directory, breaks startup on a database
that has already applied part of the pair. Check before you push:

```bash
scripts/check-migrations.sh
```

CI runs the same script and fails the build on any mismatch.

### WASM Browser Tests

`crates/trakkt-ui`'s `wasm-bindgen-test` suite runs in a real headless Firefox.
It needs Firefox and wasm-pack on your PATH; wasm-pack fetches a matching
geckodriver itself on first run.

```bash
scripts/wasm-test-isolated.sh
```

That runs the suite as one binary and then once per module on its own. The
per-module half is the point: `any_spawner`'s executor is initialized once per
test binary, so a test that spawns without initializing it passes whenever some
earlier test happened to do it first. A whole-binary run produces one order and
only one, so on its own it cannot see that dependency at all — which is how the
suite stayed green while the dependency was in it.

Running a module by itself takes away the neighbours a test could have been
leaning on, so a dependency that crosses a module boundary fails the check. A
dependency contained entirely within one module will not be caught: the
granularity is per source file, so a module's own tests still run together and
can still lean on each other. Do not treat a green sweep as proof that a new test
stands alone — if it spawns, call `crate::wasm_test_support::boot_leptos_executor()`
in the test itself. See `docs/CODING_STANDARDS.md` for that rule. CI runs the
same script.

### E2E Tests

```bash
cd e2e
npm install
npx playwright test
```

## Pull Requests

- Keep PRs focused on a single change
- Include tests for new functionality
- Make sure `cargo clippy` and `cargo test` pass
- Reference the relevant issue number if one exists

## Reporting Issues

Use [GitHub Issues](https://github.com/kyomi-ai/trakkt/issues) for bug reports and feature requests. Include reproduction steps for bugs.

## License

By contributing, you agree that your contributions will be licensed under the [AGPL-3.0-or-later](LICENSE) license, subject to the terms of the [CLA](CLA.md).
