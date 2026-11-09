//! End-to-end tests of the binary (spec 12.4).
//!
//! Every run gets an explicit environment: `LOG_FILE` and `RUST_LOG` from
//! the developer's shell must not leak into the tests.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const BIN: &str = env!("CARGO_BIN_EXE_payments-engine");
const HEADER: &str = "client,available,held,total,locked";

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

/// A fresh scratch directory for one test.
fn scratch(test: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("payments-engine-cli-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn command(args: &[&Path], env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .env_remove("LOG_FILE")
        .env_remove("RUST_LOG");
    for (name, value) in env {
        command.env(name, value);
    }
    command
}

fn run(args: &[&Path], env: &[(&str, &str)]) -> Output {
    command(args, env).output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// CSV lines in a stable order: row order is not defined (spec 6).
fn sorted_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<_> = text.lines().collect();
    lines.sort_unstable();
    lines
}

fn write_input(dir: &Path, content: &str) -> PathBuf {
    let path = dir.join("input.csv");
    fs::write(&path, content).unwrap();
    path
}

// --- Reference data sets ---

/// Runs every `tests/data/<case>/`: `input.csv` must give `expected.csv`
/// (in any row order) and the exit code in `exit_code`.
#[test]
fn reference_data_sets() {
    let mut cases: Vec<_> = fs::read_dir(data_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();
    assert!(
        !cases.is_empty(),
        "no data sets in {}",
        data_dir().display()
    );

    for case in cases {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let output = run(&[&case.join("input.csv")], &[]);

        let expected = fs::read_to_string(case.join("expected.csv")).unwrap();
        let actual = stdout(&output);
        assert_eq!(
            sorted_lines(&actual),
            sorted_lines(&expected),
            "{name}: stdout differs"
        );
        assert_eq!(actual.lines().next(), Some(HEADER), "{name}: header first");

        let code: i32 = fs::read_to_string(case.join("exit_code"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(output.status.code(), Some(code), "{name}: exit code");
    }
}

// --- Fatal errors: exit code and empty stdout (spec 2, 10.1) ---

#[test]
fn no_arguments_is_a_usage_error() {
    let output = run(&[], &[]);
    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("Usage: payments-engine <INPUT>"));
}

#[test]
fn help_goes_to_stdout_with_exit_code_zero() {
    let output = run(&[Path::new("--help")], &[]);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).contains("LOG_FILE"));
}

#[test]
fn two_arguments_is_a_usage_error() {
    let input = data_dir().join("basic/input.csv");
    let output = run(&[&input, &input], &[]);
    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
}

#[test]
fn missing_input_file() {
    let output = run(&[Path::new("no/such/input.csv")], &[]);
    assert_eq!(output.status.code(), Some(66));
    assert!(output.stdout.is_empty());
    assert!(
        stderr(&output).contains("kind=fatal"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn directory_as_input() {
    let dir = scratch("directory-input");
    let output = run(&[&dir], &[]);
    assert_eq!(output.status.code(), Some(66));
    assert!(output.stdout.is_empty());
}

#[test]
fn log_file_that_cannot_be_opened() {
    let dir = scratch("bad-log-file");
    let log = dir.join("missing-dir/engine.log");
    let input = data_dir().join("basic/input.csv");
    let output = run(&[&input], &[("LOG_FILE", log.to_str().unwrap())]);
    assert_eq!(output.status.code(), Some(73));
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("cannot open log file"));
}

#[test]
fn empty_log_file_cannot_be_opened() {
    let input = data_dir().join("basic/input.csv");
    let output = run(&[&input], &[("LOG_FILE", "")]);
    assert_eq!(output.status.code(), Some(73));
    assert!(output.stdout.is_empty());
}

#[test]
fn log_is_set_up_before_the_input_file_is_opened() {
    let output = run(&[Path::new("no/such/input.csv")], &[("LOG_FILE", "")]);
    assert_eq!(output.status.code(), Some(73));
}

// --- Configuration is not validated (spec 3) ---

#[test]
fn invalid_filter_falls_back_to_errors_only() {
    let input = data_dir().join("basic/input.csv");
    let output = run(&[&input], &[("RUST_LOG", "info=bar")]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(stdout(&output).lines().next(), Some(HEADER));

    let stderr = stderr(&output);
    assert!(stderr.contains("ignoring `info=bar`"), "{stderr}");
    assert!(stderr.contains("kind=rejected"), "{stderr}");
    assert!(!stderr.contains("kind=summary"), "{stderr}");
}

#[test]
fn empty_filter_means_errors_only() {
    let input = data_dir().join("basic/input.csv");
    let stderr = stderr(&run(&[&input], &[("RUST_LOG", "")]));
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.contains("kind=rejected"), "{stderr}");
}

// --- Log (spec 3.1) ---

#[test]
fn stdout_holds_only_csv_even_with_full_logging() {
    let input = data_dir().join("basic/input.csv");
    let output = run(&[&input], &[("RUST_LOG", "trace")]);
    assert_eq!(output.status.code(), Some(3));

    let stdout = stdout(&output);
    assert_eq!(stdout.lines().next(), Some(HEADER));
    assert!(
        stdout.lines().all(|line| !line.contains("kind=")),
        "{stdout}"
    );

    let stderr = stderr(&output);
    assert!(
        stderr.contains("DEBUG kind=accepted line=2 client=1 tx=1 type=deposit"),
        "{stderr}"
    );
    assert!(stderr.contains(r#"ERROR kind=rejected line=6 client=2 tx=5 category=business reason="insufficient funds""#), "{stderr}");
    assert!(
        stderr.contains("INFO kind=summary read=5 accepted=4 rejected_data=0 rejected_business=1"),
        "{stderr}"
    );
}

#[test]
fn default_log_shows_only_rejections() {
    let input = data_dir().join("basic/input.csv");
    let stderr = stderr(&run(&[&input], &[]));
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.contains("kind=rejected"), "{stderr}");
}

#[test]
fn rejection_without_parsed_ids_omits_them() {
    let dir = scratch("unparsed-ids");
    let input = write_input(&dir, "deposit,1\n");
    let stderr = stderr(&run(&[&input], &[]));
    assert!(
        stderr.contains(
            r#"ERROR kind=rejected line=1 category=data reason="expected 3 or 4 fields, got 2""#
        ),
        "{stderr}"
    );
}

#[test]
fn log_file_gets_the_log_and_is_appended_to() {
    let dir = scratch("log-file");
    let log = dir.join("engine.log");
    let input = data_dir().join("basic/input.csv");
    let env = [("LOG_FILE", log.to_str().unwrap())];

    let first = run(&[&input], &env);
    assert_eq!(first.status.code(), Some(3));
    assert!(first.stderr.is_empty(), "log goes to the file, not stderr");
    run(&[&input], &env);

    let log = fs::read_to_string(&log).unwrap();
    assert_eq!(log.matches("kind=rejected").count(), 2, "{log}");
}

#[test]
fn fatal_error_reaches_stderr_and_log_file() {
    let dir = scratch("fatal-with-log-file");
    let log = dir.join("engine.log");
    let output = run(
        &[Path::new("no/such/input.csv")],
        &[("LOG_FILE", log.to_str().unwrap())],
    );
    assert_eq!(output.status.code(), Some(66));
    assert!(stderr(&output).contains("cannot open input"));
    assert!(fs::read_to_string(&log).unwrap().contains("kind=fatal"));
}

#[test]
fn fatal_error_reaches_stderr_once_when_log_is_on_stderr() {
    let output = run(&[Path::new("no/such/input.csv")], &[]);
    assert_eq!(stderr(&output).matches("cannot open input").count(), 1);
}

#[test]
fn fatal_error_reaches_stderr_even_with_log_off() {
    let output = run(&[Path::new("no/such/input.csv")], &[("RUST_LOG", "off")]);
    assert_eq!(output.status.code(), Some(66));
    assert!(stderr(&output).contains("cannot open input"));
}

// --- Streaming (spec 4, 12.6) ---

/// A large generated input: 1,000,000 deposits over all `u16` clients.
/// Run with `cargo test --release -- --ignored`.
#[test]
#[ignore = "large input; run with --ignored"]
fn large_input() {
    use std::{fmt::Write as _, io::Write as _};

    let dir = scratch("large-input");
    let path = dir.join("input.csv");
    let mut file = std::io::BufWriter::new(fs::File::create(&path).unwrap());
    writeln!(file, "type,client,tx,amount").unwrap();
    let rows: u32 = 1_000_000;
    let mut line = String::new();
    for tx in 1..=rows {
        line.clear();
        writeln!(line, "deposit,{},{tx},1.0001", tx % 65_536).unwrap();
        file.write_all(line.as_bytes()).unwrap();
    }
    drop(file);

    let output = run(&[&path], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output).lines().count(), 1 + 65_536);
}
