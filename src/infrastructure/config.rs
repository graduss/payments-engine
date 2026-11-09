//! Settings of the CLI and the log.

use std::path::PathBuf;

/// Settings of the CLI and the log, filled in by `main` from the environment.
/// Values are taken as they are; the engine does not see them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppConfig {
    /// `LOG_FILE`: the log goes to stderr when it is not set.
    pub log_file: Option<PathBuf>,
    /// `RUST_LOG`: `error` when it is not set.
    pub log_filter: Option<String>,
}
