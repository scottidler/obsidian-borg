#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

pub mod config;
pub mod error;
pub mod logging;
pub mod types;

pub use config::load_config;
pub use error::BorgError;
pub use logging::setup_logging;
