// SPDX-License-Identifier: AGPL-3.0-or-later

//! WASM-safe enums for the Trakkt issue tracker.
//!
//! These enums are shared between the server (trakkt-auth) and the UI
//! (trakkt-ui, compiled to WASM). They depend only on serde — no sqlx,
//! no server-side crates.

use serde::{Deserialize, Serialize};

/// Status category — groups custom statuses into workflow stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusCategory {
    Backlog,
    Unstarted,
    Started,
    Completed,
    Cancelled,
}

impl StatusCategory {
    pub fn all() -> &'static [StatusCategory] {
        &[
            Self::Backlog,
            Self::Unstarted,
            Self::Started,
            Self::Completed,
            Self::Cancelled,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Unstarted => "unstarted",
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Backlog => "status_backlog",
            Self::Unstarted => "status_unstarted",
            Self::Started => "status_started",
            Self::Completed => "status_completed",
            Self::Cancelled => "status_cancelled",
        }
    }
}

impl std::fmt::Display for StatusCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Issue priority level. Stored as an integer in the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum Priority {
    None = 0,
    Urgent = 1,
    High = 2,
    Medium = 3,
    Low = 4,
}

impl Priority {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Urgent,
            2 => Self::High,
            3 => Self::Medium,
            4 => Self::Low,
            _ => Self::None,
        }
    }

    pub fn as_i32(&self) -> i32 {
        *self as i32
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::None => "No priority",
            Self::Urgent => "Urgent",
            Self::High => "High",
            Self::Medium => "Medium",
            Self::Low => "Low",
        }
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}
