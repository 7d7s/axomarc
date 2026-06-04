// Strongly-typed identifiers. Every entity has its own ID type so the
// compiler refuses to mix `AppId` and `DeploymentId`. Backed by `Uuid`
// for collision resistance; rendered to SQLite as the hyphenated form.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! id_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash,
            Serialize, Deserialize, sqlx::Type,
        )]
        #[serde(transparent)]
        #[sqlx(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            #[inline]
            pub const fn new() -> Self {
                Self(Uuid::nil())
            }

            #[inline]
            pub fn generate() -> Self {
                Self(Uuid::new_v4())
            }

            #[inline]
            pub fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(Uuid::from_bytes(bytes))
            }

            #[inline]
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }

            #[inline]
            pub fn is_nil(&self) -> bool {
                self.0.is_nil()
            }
        }

        impl Default for $name {
            #[inline]
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            #[inline]
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<Uuid> for $name {
            #[inline]
            fn from(u: Uuid) -> Self {
                Self(u)
            }
        }

        impl From<$name> for Uuid {
            #[inline]
            fn from(id: $name) -> Uuid {
                id.0
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;
            #[inline]
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }
    };
}

id_newtype!(
    /// Unique identifier for an [`crate::domain::App`].
    AppId
);

id_newtype!(
    /// Unique identifier for a [`crate::domain::Deployment`].
    DeploymentId
);

id_newtype!(
    /// Unique identifier for a [`crate::domain::Domain`].
    DomainId
);

id_newtype!(
    /// Unique identifier for a [`crate::domain::Secret`].
    SecretId
);

id_newtype!(
    /// Unique identifier for a [`crate::domain::Server`].
    ServerId
);

id_newtype!(
    /// Unique identifier for a [`crate::domain::Backup`].
    BackupId
);

id_newtype!(
    /// Unique identifier for a [`crate::domain::User`].
    UserId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_roundtrip_uuid() {
        let id = AppId::generate();
        let u: Uuid = id.into();
        let back = AppId::from(u);
        assert_eq!(id, back);
    }

    #[test]
    fn app_id_from_str() {
        let id = AppId::generate();
        let s = id.to_string();
        let back: AppId = s.parse().unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn distinct_id_types_are_distinct() {
        // Compile-time: AppId != DeploymentId. Runtime sanity check that
        // generated UUIDs are unique.
        let a = AppId::generate();
        let d = DeploymentId::generate();
        assert_ne!(a.to_string(), d.to_string());
    }
}
