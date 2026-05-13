// SPDX-License-Identifier: AGPL-3.0-or-later

//! Type-safe enums for all enum-like database columns.
//!
//! All enum columns are VARCHAR/TEXT storing snake_case strings.
//!
//! VARCHAR enums use manual `sqlx::Type + Encode + Decode` implementations
//! (via `impl_sqlx_varchar_enum!`) that delegate to `String`, which is
//! compatible with both PostgreSQL `TEXT` and `VARCHAR` column types.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Implements `sqlx::Type`, `sqlx::Encode`, and `sqlx::Decode` for a
/// VARCHAR enum by delegating to `String`, for both Postgres and SQLite.
///
/// This works with both `TEXT` and `VARCHAR` columns because
/// `String::compatible()` accepts both OIDs (Postgres) and SQLite stores
/// all enum values as TEXT natively.
///
/// Requires: `AsRef<str>` + `FromStr` on the enum.
macro_rules! impl_sqlx_varchar_enum {
    ($enum_type:ty) => {
        // ── Postgres ─────────────────────────────────────────────
        impl sqlx::Type<sqlx::Postgres> for $enum_type {
            fn type_info() -> sqlx::postgres::PgTypeInfo {
                <String as sqlx::Type<sqlx::Postgres>>::type_info()
            }
            fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
                <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::Postgres> for $enum_type {
            fn encode_by_ref(
                &self,
                buf: &mut sqlx::postgres::PgArgumentBuffer,
            ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync>>
            {
                <&str as sqlx::Encode<'q, sqlx::Postgres>>::encode_by_ref(&self.as_ref(), buf)
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Postgres> for $enum_type {
            fn decode(
                value: sqlx::postgres::PgValueRef<'r>,
            ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
                let s = <&str as sqlx::Decode<'r, sqlx::Postgres>>::decode(value)?;
                s.parse::<Self>().map_err(|e| e.into())
            }
        }

        // ── SQLite ───────────────────────────────────────────────
        impl sqlx::Type<sqlx::Sqlite> for $enum_type {
            fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
                <String as sqlx::Type<sqlx::Sqlite>>::type_info()
            }
            fn compatible(ty: &sqlx::sqlite::SqliteTypeInfo) -> bool {
                <String as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::Sqlite> for $enum_type {
            fn encode_by_ref(
                &self,
                args: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
            ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync>>
            {
                let s = self.as_ref().to_owned();
                <String as sqlx::Encode<'q, sqlx::Sqlite>>::encode_by_ref(&s, args)
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for $enum_type {
            fn decode(
                value: sqlx::sqlite::SqliteValueRef<'r>,
            ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
                let s = <&str as sqlx::Decode<'r, sqlx::Sqlite>>::decode(value)?;
                s.parse::<Self>().map_err(|e| e.into())
            }
        }
    };
}

// ─── Workspace role (VARCHAR) ──────────────────────────────────────────────

/// Workspace membership role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRole {
    WorkspaceAdmin,
    WorkspaceUser,
}

impl_sqlx_varchar_enum!(WorkspaceRole);

impl fmt::Display for WorkspaceRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for WorkspaceRole {
    fn as_ref(&self) -> &str {
        match self {
            Self::WorkspaceAdmin => "workspace_admin",
            Self::WorkspaceUser => "workspace_user",
        }
    }
}

impl FromStr for WorkspaceRole {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "workspace_admin" => Ok(Self::WorkspaceAdmin),
            "workspace_user" => Ok(Self::WorkspaceUser),
            _ => Err(format!("unknown WorkspaceRole: {s}")),
        }
    }
}

// ─── Workspace status (VARCHAR) ────────────────────────────────────────────

/// Workspace status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Active,
    Trial,
    Suspended,
}

impl_sqlx_varchar_enum!(WorkspaceStatus);

impl fmt::Display for WorkspaceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for WorkspaceStatus {
    fn as_ref(&self) -> &str {
        match self {
            Self::Active => "active",
            Self::Trial => "trial",
            Self::Suspended => "suspended",
        }
    }
}

impl FromStr for WorkspaceStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "trial" => Ok(Self::Trial),
            "suspended" => Ok(Self::Suspended),
            _ => Err(format!("unknown WorkspaceStatus: {s}")),
        }
    }
}

// ─── Invitation status (VARCHAR) ───────────────────────────────────────────

/// Invitation status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationStatus {
    Pending,
    Accepted,
    Declined,
    Cancelled,
    Expired,
}

impl_sqlx_varchar_enum!(InvitationStatus);

impl fmt::Display for InvitationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for InvitationStatus {
    fn as_ref(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}

impl FromStr for InvitationStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "declined" => Ok(Self::Declined),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            _ => Err(format!("unknown InvitationStatus: {s}")),
        }
    }
}

// ─── Transfer status (VARCHAR) ─────────────────────────────────────────────

/// Ownership transfer status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Pending,
    Accepted,
    Declined,
    Cancelled,
    Expired,
}

impl_sqlx_varchar_enum!(TransferStatus);

impl fmt::Display for TransferStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for TransferStatus {
    fn as_ref(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }
}

impl FromStr for TransferStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "declined" => Ok(Self::Declined),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            _ => Err(format!("unknown TransferStatus: {s}")),
        }
    }
}
