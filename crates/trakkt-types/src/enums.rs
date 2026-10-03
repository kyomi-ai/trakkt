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

/// How a user-attributable action was initiated.
///
/// Tracks whether an action came from a browser session (`User`), an automated
/// agent such as MCP or OAuth (`Agent`), a bare API token (`Api`), or a GitHub
/// webhook event such as a commit or pull request (`Github`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ActionSource {
    User,
    Agent,
    Api,
    Github,
}

impl ActionSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
            Self::Api => "api",
            Self::Github => "github",
        }
    }
}

impl std::fmt::Display for ActionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ActionSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "agent" => Ok(Self::Agent),
            "api" => Ok(Self::Api),
            "github" => Ok(Self::Github),
            other => Err(format!("unknown action source: {other}")),
        }
    }
}

/// Declares [`FavoriteTarget`] and everything derived from its variant list.
///
/// The variants, `ALL`, `as_str` and `from_wire` all come from one macro input,
/// so none of them can fall behind the others — the same reason
/// `trakkt_types::sync::declare_entity_types!` exists.
macro_rules! declare_favorite_targets {
    ($($variant:ident = $wire:literal;)+) => {
        /// What a `favorites` row can point at.
        ///
        /// `favorites.target_id` is polymorphic — one TEXT column naming a row
        /// in whichever table `target_type` selects — so no single foreign key
        /// can express it, and neither dialect's schema deletes a favorite when
        /// its target goes. Each parent's delete path removes them instead
        /// (TRA-10025), which is only sound while the set of parents is
        /// *enumerable*. This enum is that enumeration.
        ///
        /// It is also why `target_type` is no longer a free string.
        /// `favorite_service::add_favorite` takes this type rather than a
        /// `&str`, so a row whose type no delete path handles cannot be written
        /// in the first place — before TRA-10025 the column accepted anything
        /// the HTTP caller sent.
        ///
        /// # Adding a variant
        ///
        /// One line in the `declare_favorite_targets!` invocation below gives
        /// the variant its wire string and puts it in [`FavoriteTarget::ALL`].
        /// What the compiler cannot do for you is delete the rows: that lives in
        /// the parent's own delete path, via
        /// `favorite_service::doomed_favorites_tx`.
        ///
        /// `every_favorite_target_is_deleted_with_its_target`
        /// (`apps/server/tests/postgres_dialect.rs`) is what makes that step
        /// non-optional. It walks [`FavoriteTarget::ALL`] and dispatches through
        /// an exhaustive `match` of its own, so a new variant does not compile
        /// until someone writes the arm that deletes one — and the arm has to
        /// run the real service function, because the assertions are on the rows
        /// and on the `sync_log` entries afterwards.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum FavoriteTarget {
            $($variant),+
        }

        impl FavoriteTarget {
            /// Every target type a favorite can name, in declaration order.
            ///
            /// Emitted from the same macro input as the variants themselves, so
            /// unlike a hand-maintained array it cannot omit one.
            pub const ALL: &'static [FavoriteTarget] = &[$(FavoriteTarget::$variant),+];

            /// The string stored in `favorites.target_type` and sent on the wire.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }

            /// Parse a `favorites.target_type` value.
            ///
            /// `None` for anything else, which is what closes the set: the
            /// server function that takes a client-supplied string rejects the
            /// request rather than writing a row nothing will ever clean up.
            pub fn from_wire(wire: &str) -> Option<Self> {
                match wire {
                    $($wire => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

declare_favorite_targets! {
    Issue = "issue";
    Project = "project";
    Team = "team";
    View = "view";
}

impl std::fmt::Display for FavoriteTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod favorite_target_tests {
    use std::collections::BTreeSet;

    use super::FavoriteTarget;

    /// Two variants sharing a wire string would make `from_wire` return one of
    /// them for both, so a favorite of the shadowed type would be deleted by the
    /// wrong parent's delete path — or by none.
    #[test]
    fn every_target_has_a_distinct_wire_string() {
        let unique: BTreeSet<&str> = FavoriteTarget::ALL.iter().map(|t| t.as_str()).collect();

        assert_eq!(
            unique.len(),
            FavoriteTarget::ALL.len(),
            "`FavoriteTarget::ALL` holds a duplicate wire string: {:?}",
            FavoriteTarget::ALL
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>()
        );
    }

    /// `from_wire` is the only way a client-supplied string becomes a
    /// `FavoriteTarget`, so a variant it cannot produce is a type the product
    /// can never favorite.
    #[test]
    fn every_target_round_trips_through_its_wire_string() {
        for target in FavoriteTarget::ALL {
            assert_eq!(
                FavoriteTarget::from_wire(target.as_str()),
                Some(*target),
                "{target} has a wire string `from_wire` does not accept"
            );
        }
    }

    /// The rejection is the point of the type: before TRA-10025 this string went
    /// straight into `favorites.target_type` and produced a row no delete path
    /// would ever remove.
    #[test]
    fn an_unknown_wire_string_is_rejected() {
        assert_eq!(
            FavoriteTarget::from_wire("milestone"),
            None,
            "`milestone` is not a favorite target — accepting it would write a \
             row no parent delete path removes, which is exactly the dangling \
             favorite TRA-10025 fixed"
        );
    }
}
