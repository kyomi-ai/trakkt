// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared setup for the crate's browser tests.
//!
//! This lives at the crate root rather than beside any one caller because its
//! callers are in two different subtrees — `cache/` and `pages/settings/` — so
//! the crate root is the only module that is an ancestor of all of them.
//!
//! Run the tests this supports with:
//! `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`

/// Boot the executor Leptos spawns its async work onto.
///
/// Production gets this from `mount_to_body` / `hydrate_body`, which a
/// `wasm-bindgen-test` never calls — so without it, anything that spawns
/// panics, or, where the spawn is what the test was watching for, never runs
/// and the test passes for the wrong reason. `Effect::new`, `Resource::new` and
/// `LocalResource::new` all spawn the moment they are constructed, so a test
/// that builds any of them before it mounts anything must call this first.
///
/// It is the same executor production uses either way: `init_wasm_bindgen`
/// installs `wasm_bindgen_futures::spawn_local`, which is what
/// `leptos::task::spawn_local` resolves to on this target.
///
/// # Why every test calls it
///
/// The executor is global and set once per page, and every test in a
/// `wasm-bindgen-test` binary shares one page. So a test that spawns without
/// calling this does not fail reliably — it fails only when it happens to run
/// before whichever test did call it. That is an ordering dependency, and it
/// is invisible locally right up until a different runner picks a different
/// order. Calling this unconditionally is what makes each test stand alone;
/// a second caller being told the executor is already set is the answer it
/// wanted, which is why `AlreadySet` is a success here rather than an error.
pub fn boot_leptos_executor() {
    match any_spawner::Executor::init_wasm_bindgen() {
        Ok(()) | Err(any_spawner::ExecutorError::AlreadySet) => {}
    }
}
