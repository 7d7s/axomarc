// `User` — a principal that can authenticate. V0 ships a local user
// table; V1 adds OIDC; V2+ uses the role for RBAC enforcement.

use super::id::UserId;
use super::timestamp::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum UserRole {
    /// The bootstrap principal. Singleton.
    Owner,
    /// Full read/write across all apps.
    Admin,
    /// Read/write on apps they own.
    Developer,
    /// Read-only. No mutations.
    Readonly,
}

impl UserRole {
    pub const ALL: &'static [&'static str] = &["owner", "admin", "developer", "readonly"];

    pub fn can(self, other: UserRole) -> bool {
        use UserRole::*;
        matches!(
            (self, other),
            (Owner, _) | (Admin, Admin) | (Admin, Developer) | (Admin, Readonly)
        )
    }
}

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Developer => "developer",
            Self::Readonly => "readonly",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub role: UserRole,
    pub created_at: Timestamp,
    pub last_seen: Option<Timestamp>,
}

#[derive(Debug, Clone)]
pub struct NewUser {
    pub email: String,
    pub role: UserRole,
}

/// An API token record (never stores the plaintext token).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct ApiToken {
    pub id: Vec<u8>,         // BLOB (16 bytes)
    pub user_id: Vec<u8>,    // BLOB (16 bytes)
    pub name: String,
    pub hash: String,        // sha256(token), never plaintext
    pub scopes: String,      // CSV
    pub created_at: Timestamp,
    pub expires_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_can() {
        use UserRole::*;
        // Owner can do anything.
        for r in [Owner, Admin, Developer, Readonly] {
            assert!(Owner.can(r), "owner should be able to assume {r:?}");
        }
        // Admin can manage non-owner roles.
        assert!(Admin.can(Admin));
        assert!(Admin.can(Developer));
        assert!(Admin.can(Readonly));
        // Admin cannot create another owner.
        assert!(!Admin.can(Owner));
        // Developer/Readonly cannot assume other roles.
        assert!(!Developer.can(Admin));
        assert!(!Readonly.can(Developer));
    }
}
