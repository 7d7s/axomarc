use clap::ValueEnum;
use serde::Serialize;

/// The diagnostic level the operator requested.
///
/// V0 ships `Basic` only. `Standard`, `Full`, `Paranoid`, and `Custom`
/// are reserved for V1+; the CLI returns a clear error rather than
/// silently downgrading.
#[derive(Debug, Clone, Copy, Serialize, ValueEnum, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DoctorLevel {
    Basic,
    Standard,
    Full,
    Paranoid,
    Custom,
}

impl DoctorLevel {
    /// `true` iff this level is implemented in the V0 binary.
    pub fn is_shipped_in_v0(self) -> bool {
        matches!(self, DoctorLevel::Basic)
    }

    /// Human label (used in error messages and the JSON report).
    pub fn label(self) -> &'static str {
        match self {
            DoctorLevel::Basic => "basic",
            DoctorLevel::Standard => "standard",
            DoctorLevel::Full => "full",
            DoctorLevel::Paranoid => "paranoid",
            DoctorLevel::Custom => "custom",
        }
    }
}

impl std::fmt::Display for DoctorLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_is_shipped() {
        assert!(DoctorLevel::Basic.is_shipped_in_v0());
    }

    #[test]
    fn higher_levels_not_shipped() {
        assert!(!DoctorLevel::Standard.is_shipped_in_v0());
        assert!(!DoctorLevel::Full.is_shipped_in_v0());
        assert!(!DoctorLevel::Paranoid.is_shipped_in_v0());
        assert!(!DoctorLevel::Custom.is_shipped_in_v0());
    }

    #[test]
    fn label_matches_value() {
        assert_eq!(DoctorLevel::Basic.label(), "basic");
        assert_eq!(DoctorLevel::Standard.label(), "standard");
    }
}
