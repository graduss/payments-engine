use std::{
    fmt,
    fs::{File, OpenOptions},
    io,
    path::Path,
    sync::Arc,
};

use tracing::{Level, Subscriber};
use tracing_subscriber::{EnvFilter, fmt::MakeWriter};

use crate::{
    application::{Accepted, Category, Rejection, Summary},
    domain::EventKind,
    infrastructure::config::AppConfig,
};

const DEFAULT_FILTER: &str = "error";

/// Where the log goes (spec 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTarget {
    Stderr,
    File,
}

/// Installs the global log: stderr, or `log_file` opened for appending.
///
/// Fails only if the file cannot be opened (exit code 73). Invalid parts of
/// the filter are skipped by `EnvFilter` with a note on stderr.
pub fn init(config: &AppConfig) -> io::Result<LogTarget> {
    let filter = EnvFilter::new(config.log_filter.as_deref().unwrap_or(DEFAULT_FILTER));
    let target = match config.log_file.as_deref() {
        None => {
            install(subscriber(io::stderr, filter))?;
            LogTarget::Stderr
        }
        Some(path) => {
            install(subscriber(Arc::new(open(path)?), filter))?;
            LogTarget::File
        }
    };
    Ok(target)
}

fn install(subscriber: impl Subscriber + Send + Sync) -> io::Result<()> {
    // Fails only if a global subscriber is already set, that is if `init`
    // is called twice.
    tracing::subscriber::set_global_default(subscriber).map_err(io::Error::other)
}

/// Opens the log file for appending. `File` is unbuffered, so every event
/// reaches the file before `process::exit` (spec 3.1).
fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().append(true).create(true).open(path)
}

/// One text line per event: time, level, then `key=value` fields.
/// No module path and no ANSI colours, on stderr as well as in a file.
fn subscriber<W>(writer: W, filter: EnvFilter) -> impl Subscriber + Send + Sync
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(false)
        .finish()
}

/// Logs a rejected row. `client` and `tx` appear only if they were parsed.
///
/// `%` prints a value as is and `?` quotes it, so the reason, which may
/// contain spaces, is the only quoted field.
pub fn rejected(rejection: &Rejection<'_>) {
    tracing::error!(
        kind = %"rejected",
        line = rejection.line,
        client = rejection.client.map(tracing::field::display),
        tx = rejection.tx.map(tracing::field::display),
        category = %category_name(rejection.category),
        reason = ?rejection.reason.to_string(),
    );
}

/// Logs an applied row at `debug` (spec 3.1).
pub fn accepted(accepted: &Accepted) {
    tracing::debug!(
        kind = %"accepted",
        line = accepted.line,
        client = %accepted.client,
        tx = %accepted.tx,
        r#type = %type_name(accepted.kind),
    );
}

/// Logs the summary line at `info` (spec 2, ОВ-24).
pub fn summary(summary: &Summary) {
    tracing::info!(
        kind = %"summary",
        read = summary.read,
        accepted = summary.accepted,
        rejected_data = summary.rejected_data,
        rejected_business = summary.rejected_business,
    );
}

/// Reports a fatal error. It always reaches stderr, exactly once (spec 3.1).
///
/// Also safe before `init`: with no subscriber nothing is logged, so the
/// message goes straight to stderr.
pub fn report_fatal(target: LogTarget, error: &dyn fmt::Display) {
    let message = error.to_string();
    let logged = tracing::enabled!(Level::ERROR);
    if logged {
        tracing::error!(kind = %"fatal", error = ?message);
    }
    if needs_stderr_copy(target, logged) {
        eprintln!("error: {message}");
    }
}

/// The log already shows on stderr only if it goes there and the filter
/// lets errors through; `RUST_LOG=off` must not hide a fatal error.
fn needs_stderr_copy(target: LogTarget, logged: bool) -> bool {
    target == LogTarget::File || !logged
}

fn type_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Deposit => "deposit",
        EventKind::Withdrawal => "withdrawal",
        EventKind::Dispute => "dispute",
        EventKind::Resolve => "resolve",
        EventKind::Chargeback => "chargeback",
    }
}

fn category_name(category: Category) -> &'static str {
    match category {
        Category::Data => "data",
        Category::Business => "business",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ClientId, TxId};
    use std::{
        env, fs,
        io::Write,
        sync::{Arc, Mutex},
    };

    /// A shared in-memory log.
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Runs `f` with a test subscriber and returns the logged lines without
    /// their timestamps.
    fn capture(filter: &str, f: impl FnOnce()) -> Vec<String> {
        let buffer = Buffer::default();
        let writer = buffer.clone();
        let subscriber = subscriber(move || writer.clone(), EnvFilter::new(filter));
        tracing::subscriber::with_default(subscriber, f);

        let log = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        log.lines()
            .map(|line| line.split_once(' ').unwrap().1.trim_start().to_string())
            .collect()
    }

    fn rejection(
        client: Option<u16>,
        tx: Option<u32>,
        category: Category,
        reason: &str,
    ) -> Vec<String> {
        capture("error", || {
            rejected(&Rejection {
                line: 12,
                client: client.map(ClientId::from),
                tx: tx.map(TxId::from),
                category,
                reason: &reason,
            })
        })
    }

    #[test]
    fn rejected_row_has_spec_format() {
        assert_eq!(
            rejection(Some(3), Some(41), Category::Business, "insufficient funds"),
            vec![
                r#"ERROR kind=rejected line=12 client=3 tx=41 category=business reason="insufficient funds""#
            ]
        );
    }

    #[test]
    fn rejected_row_omits_ids_that_were_not_parsed() {
        assert_eq!(
            rejection(None, Some(41), Category::Data, "invalid client"),
            vec![r#"ERROR kind=rejected line=12 tx=41 category=data reason="invalid client""#]
        );
        assert_eq!(
            rejection(None, None, Category::Data, "expected 3 or 4 fields, got 2"),
            vec![
                r#"ERROR kind=rejected line=12 category=data reason="expected 3 or 4 fields, got 2""#
            ]
        );
    }

    #[test]
    fn quotes_in_reason_are_escaped() {
        assert_eq!(
            rejection(Some(1), Some(2), Category::Data, r#"bad "value""#),
            vec![
                r#"ERROR kind=rejected line=12 client=1 tx=2 category=data reason="bad \"value\"""#
            ]
        );
    }

    #[test]
    fn accepted_row_is_logged_at_debug_only() {
        let row = Accepted {
            line: 7,
            client: ClientId::from(2),
            tx: TxId::from(9),
            kind: EventKind::Withdrawal,
        };
        assert_eq!(
            capture("debug", || accepted(&row)),
            vec!["DEBUG kind=accepted line=7 client=2 tx=9 type=withdrawal"]
        );
        assert!(capture("info", || accepted(&row)).is_empty());
    }

    #[test]
    fn summary_is_logged_at_info_only() {
        let totals = Summary {
            read: 10,
            accepted: 7,
            rejected_data: 2,
            rejected_business: 1,
        };
        assert_eq!(
            capture("info", || summary(&totals)),
            vec!["INFO kind=summary read=10 accepted=7 rejected_data=2 rejected_business=1"]
        );
        assert!(capture("error", || summary(&totals)).is_empty());
    }

    #[test]
    fn fatal_error_is_logged() {
        let error = io::Error::other("disk failure");
        assert_eq!(
            capture("error", || report_fatal(LogTarget::File, &error)),
            vec![r#"ERROR kind=fatal error="disk failure""#]
        );
    }

    #[test]
    fn filter_off_logs_nothing() {
        let error = io::Error::other("disk failure");
        assert!(capture("off", || report_fatal(LogTarget::Stderr, &error)).is_empty());
    }

    #[test]
    fn fatal_error_reaches_stderr_exactly_once() {
        // Log on stderr and errors enabled: the log line is the stderr copy.
        assert!(!needs_stderr_copy(LogTarget::Stderr, true));
        // Log on stderr but filtered out (RUST_LOG=off): print it directly.
        assert!(needs_stderr_copy(LogTarget::Stderr, false));
        // Log in a file: stderr needs its own copy either way.
        assert!(needs_stderr_copy(LogTarget::File, true));
        assert!(needs_stderr_copy(LogTarget::File, false));
    }

    #[test]
    fn log_file_is_created_and_appended_to() {
        let dir = env::temp_dir().join(format!("payments-engine-{}-log-file", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("engine.log");

        writeln!(open(&path).unwrap(), "first").unwrap();
        writeln!(open(&path).unwrap(), "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");
    }

    #[test]
    fn init_fails_when_log_file_cannot_be_opened() {
        let path = env::temp_dir()
            .join(format!(
                "payments-engine-{}-no-such-dir",
                std::process::id()
            ))
            .join("engine.log");
        let config = AppConfig {
            log_file: Some(path),
            log_filter: None,
        };
        let error = init(&config).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
