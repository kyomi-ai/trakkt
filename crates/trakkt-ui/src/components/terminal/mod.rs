// SPDX-License-Identifier: AGPL-3.0-or-later

//! Terminal emulator components — grid model, VTE handler, renderer, and
//! input processing.  Compiles to WASM for use in the browser.

pub mod grid;
pub mod input;
pub mod renderer;
pub mod tab_manager;
pub mod vte_handler;

pub use grid::{CellAttrs, Color, CursorState, Grid, Cell, StyledSpan, TerminalModes};
pub use renderer::TerminalRenderer;
pub use vte_handler::TerminalHandler;
