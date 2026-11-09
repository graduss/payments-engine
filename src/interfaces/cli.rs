//! The command line: `payments-engine <transactions.csv> > accounts.csv`.

use std::{
    ffi::OsString,
    fmt,
    fs::File,
    io::{self, BufReader},
    path::{Path, PathBuf},
};

use clap::Parser;

use crate::{
    application::{Outcome, PaymentEngine, process},
    infrastructure::{
        csv_reader::CsvTransactionReader,
        csv_writer::write_accounts,
        logging::{self, LogTarget},
    },
};

pub use crate::infrastructure::config::AppConfig;

/// Applies a CSV file of client transactions and prints the final state of
/// every account as CSV to stdout.
///
/// Settings come from the environment: LOG_FILE (log file instead of
/// stderr) and RUST_LOG (log filter, `error` by default).
#[derive(Debug, Parser)]
#[command(name = "payments-engine", version)]
struct Args {
    /// CSV file with columns type, client, tx, amount.
    input: PathBuf,
}

/// Exit codes (spec 2). 64 and above follow `sysexits.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitStatus {
    Ok = 0,
    BusinessRejects = 3,
    DataRejects = 4,
    Usage = 64,
    NoInput = 66,
    CantCreate = 73,
    IoError = 74,
}

impl From<Outcome> for ExitStatus {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Clean => Self::Ok,
            Outcome::BusinessRejects => Self::BusinessRejects,
            Outcome::DataRejects => Self::DataRejects,
        }
    }
}

/// Runs the program. The first step that fails sets the exit code (spec 2.1).
pub fn run(args: impl IntoIterator<Item = OsString>, config: AppConfig) -> ExitStatus {
    let args = match Args::try_parse_from(args) {
        Ok(args) => args,
        Err(error) => {
            // --help and --version are not errors and go to stdout.
            let status = if error.use_stderr() {
                ExitStatus::Usage
            } else {
                ExitStatus::Ok
            };
            let _ = error.print();
            return status;
        }
    };
    let path = args.input;

    let target = match logging::init(&config) {
        Ok(target) => target,
        Err(error) => {
            let log_file = config.log_file.unwrap_or_default();
            let message = format_args!("cannot open log file {}: {error}", log_file.display());
            return fail(LogTarget::Stderr, ExitStatus::CantCreate, message);
        }
    };

    let input = match open_input(&path) {
        Ok(input) => input,
        Err(error) => {
            let message = format_args!("cannot open input {}: {error}", path.display());
            return fail(target, ExitStatus::NoInput, message);
        }
    };

    let mut engine = PaymentEngine::new();
    let reader = CsvTransactionReader::new(BufReader::new(input));
    let summary = match process(&mut engine, reader, logging::rejected, logging::accepted) {
        Ok(summary) => summary,
        Err(error) => {
            let message = format_args!("cannot read input {}: {error}", path.display());
            return fail(target, ExitStatus::IoError, message);
        }
    };

    if let Err(error) = write_accounts(io::stdout().lock(), engine.accounts()) {
        let message = format_args!("cannot write accounts: {error}");
        return fail(target, ExitStatus::IoError, message);
    }

    logging::summary(&summary);
    summary.outcome().into()
}

fn fail(target: LogTarget, status: ExitStatus, message: fmt::Arguments<'_>) -> ExitStatus {
    logging::report_fatal(target, &message);
    status
}

/// A directory opens fine on Unix but cannot be read. It is rejected here
/// as "cannot open" (66) rather than as a read error (74) later.
fn open_input(path: &Path) -> io::Result<File> {
    let file = File::open(path)?;
    if file.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::IsADirectory,
            "is a directory",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn takes_exactly_one_argument() {
        let parsed = Args::try_parse_from(args(&["prog", "in.csv"])).unwrap();
        assert_eq!(parsed.input, PathBuf::from("in.csv"));

        assert!(Args::try_parse_from(args(&["prog"])).is_err());
        assert!(Args::try_parse_from(args(&["prog", "a.csv", "b.csv"])).is_err());
    }

    #[test]
    fn wrong_argument_count_is_a_usage_error() {
        let config = AppConfig::default;
        assert_eq!(run(args(&["prog"]), config()), ExitStatus::Usage);
        assert_eq!(
            run(args(&["prog", "a.csv", "b.csv"]), config()),
            ExitStatus::Usage
        );
    }

    #[test]
    fn clap_definition_is_valid() {
        use clap::CommandFactory;
        Args::command().debug_assert();
    }

    #[test]
    fn outcome_maps_to_exit_status() {
        assert_eq!(ExitStatus::from(Outcome::Clean), ExitStatus::Ok);
        assert_eq!(
            ExitStatus::from(Outcome::BusinessRejects),
            ExitStatus::BusinessRejects
        );
        assert_eq!(
            ExitStatus::from(Outcome::DataRejects),
            ExitStatus::DataRejects
        );
    }

    #[test]
    fn exit_codes_match_the_spec() {
        let codes = [
            (ExitStatus::Ok, 0),
            (ExitStatus::BusinessRejects, 3),
            (ExitStatus::DataRejects, 4),
            (ExitStatus::Usage, 64),
            (ExitStatus::NoInput, 66),
            (ExitStatus::CantCreate, 73),
            (ExitStatus::IoError, 74),
        ];
        for (status, code) in codes {
            assert_eq!(status as i32, code, "{status:?}");
        }
    }

    #[test]
    fn directory_is_not_an_input_file() {
        let error = open_input(&std::env::temp_dir()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::IsADirectory);
    }

    #[test]
    fn missing_input_cannot_be_opened() {
        let error = open_input(Path::new("no/such/file.csv")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
