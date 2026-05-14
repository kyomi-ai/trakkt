// SPDX-License-Identifier: AGPL-3.0-or-later

//! API parameter structs shared between MCP and REST surfaces.
//!
//! Each operation defines its input parameters here so that schema generation
//! (via `schemars::JsonSchema`) and deserialization work identically regardless
//! of the transport.
