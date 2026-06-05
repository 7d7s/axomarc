#![allow(missing_docs)]

pub mod check;
pub mod checks;
pub mod fix;
pub mod level;
pub mod report;

pub use check::{Check, CheckCategory, CheckContext, CheckResult, CheckStatus};
pub use fix::{DoctorFix, FixError, FixOutcome};
pub use level::DoctorLevel;
pub use report::{Doctor, DoctorReport};

/// Default minimum free memory in megabytes for the basic `System` checks.
pub const MIN_FREE_MEMORY_MB: u64 = 512;

/// Default minimum free disk space on the data directory in megabytes.
pub const MIN_FREE_DISK_MB: u64 = 5 * 1024;

/// Default number of checks per category the V0 basic level ships.
pub const BASIC_CHECKS_PER_CATEGORY: usize = 3;
