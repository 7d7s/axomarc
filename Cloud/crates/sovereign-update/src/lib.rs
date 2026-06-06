#![allow(missing_docs)]

pub mod http_update;
pub mod mock_update;

pub use http_update::HttpUpdate;
pub use mock_update::MockUpdate;

pub use sovereign_core::ports::{
    update::Binary, Release, Sha256, UpdateChannel, UpdateError, UpdatePort, UpdateRecord, Version,
};
