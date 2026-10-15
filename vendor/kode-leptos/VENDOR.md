# kode-leptos selection ownership patch

This directory contains the published crates.io `kode-leptos` 0.5.5 source.
The package checksum in the original Trakkt lockfile is
`278ee072799d54f10c2e7d9e5aa741e4997b84652d4c30535fb1166a9c5ad2c7`;
its `.cargo_vcs_info.json` identifies upstream commit
`04a38056966543b36ac61fb9d5f15958c3a27b33` and path `kode-leptos`.
The normalized published Cargo.toml keeps kode-core, kode-doc and kode-markdown
on their registry dependencies. The package's own Cargo.lock and Cargo cache
metadata are intentionally excluded. The published package omitted its MIT
license file; LICENSE retains the upstream repository's copyright and notice.

The only source change is in `src/wysiwyg/tree_editor.rs`, at the rendering
Effect's animation-frame selection restoration:

- Readonly renderers never schedule a selection restoration.
- The callback reads the node and composition signal without panicking after
  disposal, then checks that the node is connected, the editor root still owns
  focus, and no composition is active **when the callback executes**.
- The keydown-handled flag is cleared even when restoration is skipped.

This prevents an unfocused/readonly editor from moving the browser's global
Selection (which can also reset a focused native input's selectionStart), and
prevents a callback scheduled before disposal from reading a disposed node ref.
No DOM remount or forced focus is used.

Trakkt's UI uses this directory as a direct version-constrained path dependency,
so local-development patch stripping does not remove the production fix. The
workspace excludes this third-party crate from first-party workspace lint/test
selection. Dockerfile.ci copies vendor source with the repository before build;
Dockerfile packages the already-built binary and frontend assets.

Remove this vendor directory and restore the registry dependency when a published
kode-leptos release includes these ownership, composition and disposal guards,
after running the editor autosave and cross-surface focus regressions against it.
