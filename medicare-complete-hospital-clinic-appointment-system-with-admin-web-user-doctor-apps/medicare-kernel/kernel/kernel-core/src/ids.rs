//! Identity & money newtypes (10-GUIDELINES §3.2) + privacy wrappers.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
            pub fn from_uuid(u: Uuid) -> Self {
                Self(u)
            }
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(u: Uuid) -> Self {
                Self(u)
            }
        }
    };
}

id_type!(TenantId);
id_type!(UserId);
id_type!(IdentityId);
id_type!(NodeId);
id_type!(SlotId);
id_type!(AppointmentId);
id_type!(PatientRefId);

/// Money as integer minor units + ISO currency. f64 for money is banned (10-GUIDELINES §3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoneyCents {
    pub minor: i64,
    pub currency: String,
}

impl MoneyCents {
    pub fn inr(minor: i64) -> Self {
        Self { minor, currency: "INR".into() }
    }
}

/// Wrapper that implements neither Debug nor Display: impossible to log accidentally.
pub struct Secret<T>(pub T);

impl<T> Secret<T> {
    pub fn expose_for_verification_only(&self) -> &T {
        &self.0
    }
}

/// PHI wrapper: same compile-time logging protection as Secret.
pub struct Phi<T>(pub T);

impl<T> Phi<T> {
    pub fn masked_last4(&self) -> String
    where
        T: AsRef<str>,
    {
        let s = self.0.as_ref();
        if s.len() <= 4 {
            return "***".to_string();
        }
        format!("***{}", &s[s.len() - 4..])
    }
}