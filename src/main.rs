use std::{env, path::PathBuf};

use payments_engine::interfaces::cli::{self, AppConfig};

fn main() {
    let config = AppConfig {
        log_file: env::var_os("LOG_FILE").map(PathBuf::from),
        log_filter: env::var("RUST_LOG").ok(),
    };

    // process::exit skips destructors; the log writes synchronously, so
    // nothing is lost (spec 3.1).
    std::process::exit(cli::run(env::args_os(), config) as i32);
}
