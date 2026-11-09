# Payments engine architecture

Version: draft of 2026-11-07, brought in line with the code on 2026-11-09.

This document describes how the code is structured. Program behaviour is defined by [`specification.md`](specification.md); references to its sections are given as "spec, N".

---

## 1. Principles

- Clean architecture with four layers: `domain`, `application`, `infrastructure`, `interfaces`.
- The layers are modules inside a single crate, not separate crates. No workspace is used (spec, 11).
- Dependencies point inward only: `interfaces → infrastructure → application → domain`. `interfaces` may access any inner layer.
- `domain` knows nothing about CSV, the log, the environment or files. Its external dependencies are `rust_decimal` and `thiserror`.
- Port traits are introduced only where they have a second implementation or tests need them. No ceremony for its own sake: the solution is scoped for 2–3 hours, and what is assessed is the cleanliness and maintainability of the code.

```
interfaces ──▶ infrastructure ──▶ application ──▶ domain
     └───────────────────────────────▲──────────────▲
```

Crate dependencies: `rust_decimal`, `thiserror`, `tracing`, `tracing-subscriber` (feature `env-filter`), `clap` (feature `derive`). Dev dependency: `rust_decimal_macros`.

---

## 2. Module layout

```
src/
  main.rs                 reads LOG_FILE and RUST_LOG into AppConfig, calls cli::run and std::process::exit
  lib.rs                  declares the layers; domain, application, interfaces are public (AD-11)
  domain.rs               re-exports domain types
  domain/
    ids.rs                ClientId(u16), TxId(u32)
    money.rs              Amount: newtype over Decimal (> 0, ≤ 4 decimal places)
    account.rs            Account { client_id, available, held, locked } and the account change rules
    transaction.rs        Event, EventKind, Transaction<K>, Deposit, Withdrawal, DisputeState and dispute transitions
    error.rs              DomainError
  application.rs          re-exports: PaymentEngine, process and the process types
  application/
    engine.rs             PaymentEngine: apply(event), accounts()
    used_tx.rs            UsedTxIds: hides HashSet<TxId>
    process.rs            "process a stream" use case: Row, Rejection, Accepted, Summary, Outcome, categories
    error.rs              ApplicationError
  infrastructure.rs       pub(crate)
  infrastructure/
    config.rs             AppConfig: log parameters, filled in by main
    logging.rs            tracing: stderr or LOG_FILE, rejected/accepted/summary/fatal events
    csv_reader.rs         file lines -> Row<RowError>
    csv_writer.rs         accounts -> CSV
  interfaces.rs
  interfaces/
    cli.rs                arguments (clap), startup order, Outcome -> exit code
tests/
  cli.rs                  end-to-end tests of the binary
  data/<scenario>/        input.csv, expected.csv, exit_code
```

---

## 3. Layers

### 3.1. `domain`

Domain types and rules. Does not depend on the other layers.

| Module | Contents | Spec |
|---|---|---|
| `ids` | `ClientId` and `TxId` are newtypes so the identifiers cannot be mixed up (T1). Created via `From<u16>` / `From<u32>`, printed via `Display` | 4 |
| `money` | `Amount`: `TryFrom<Decimal>` normalises the value (trailing zeros) and checks `> 0` and at most 4 decimal places, without rounding | 4.1, 9 |
| `account` | `Account` and the methods `deposit`, `withdraw`, `hold`, `release`, `charge_back`. All arithmetic goes through `checked_*`, including the check of the new `total` in `deposit` and of the new `available` and `held` in `hold`, `release` and `charge_back` (OQ-25). `total()` is computed as `available + held` | 5.1, 7, 9 |
| `transaction` | `Event` is a parsed row (`client`, `tx`, `kind`, `amount: Option<Amount>`). `Transaction<K>` is an accepted transaction of kind `Deposit` or `Withdrawal`, built from an `Event` via `TryFrom` (kind mismatch or missing amount gives `UnsupportedEvent`). `Transaction<Deposit>` stores the amount, `DisputeState` and `dispute_count` and implements the dispute transitions `dispute`, `resolve`, `charge_back`. The transitions take no amount: a dispute always covers the full deposit amount. `dispute_count` grows via `saturating_add` and only counts disputes | 5.2, 8 |
| `error` | `DomainError`: one reason per variant, reason text via `thiserror` | 10.2 |

The rules for dispute transitions and account changes live here, not in the engine. This lets them be tested separately from orchestration.

### 3.2. `application`

Use cases. Depends only on `domain`.

- `PaymentEngine` holds the accounts (`HashMap<ClientId, Account>`), the processed deposits (`HashMap<TxId, Transaction<Deposit>>`) and `UsedTxIds`. The `apply` method performs checks 6–9+ strictly in the order given in spec 10.3: it looks up the required domain objects and calls their methods.
- `ApplicationError` covers the errors the engine itself detects (`DuplicateTx`, `AccountNotFound`, `NotADeposit`), plus a wrapper around `DomainError`.
- `PaymentEngine` receives no configuration and does not read the environment.
- `UsedTxIds` hides the implementation of the set of used `tx` values (spec, 5.3).
- `process` is the "process a stream" use case. It takes the engine, an iterator of rows from the reader, and the closures `on_rejected` and `on_accepted` (AD-2, AD-9). It keeps the counters `read`, `accepted`, `rejected_data` and `rejected_business`. It returns a `Summary` or a fatal source error.
- The rejection category (`Category { Data, Business }`) is determined in `process` by the function `category_of`. The domain knows nothing about categories. Parse errors are always "data"; among engine errors, `DuplicateTx`, `AmountNotPositive`, `AmountTooPrecise`, `UnsupportedEvent` are "data", the rest are "business".
- `Outcome { Clean, BusinessRejects, DataRejects }` (`Summary::outcome`) describes the result in domain terms. `application` knows nothing about exit code numbers.

### 3.3. `infrastructure`

Adapters to the outside world. The module is `pub(crate)`: outside the crate only `AppConfig` is visible, re-exported from `cli`.

| Module | Responsibility | Spec |
|---|---|---|
| `config` | Plain struct `AppConfig` (`log_file`, `log_filter`). `main` fills it from the environment; values are not validated (AD-12) | 3 |
| `logging` | Initialises `tracing` with the non-strict `EnvFilter::new`, writes to stderr or `LOG_FILE` (append, no ANSI, synchronous). Produces the `rejected`, `accepted`, `summary`, `fatal` events. `report_fatal` guarantees that a fatal error reaches stderr exactly once | 3.1 |
| `csv_reader` | Reads input line by line via `BufRead::read_until` and splits each line on commas; the `csv` crate is not used (AD-5). Skips empty lines and the header, strips the BOM, tracks the physical line number. Performs checks 1–5 from 10.3 and yields an `Event` or a row error of category "data", including a non-empty `amount` on disputes (`UnexpectedAmount`, OQ-11) and a missing `amount` on `deposit` and `withdrawal` (`MissingAmount`, AD-6) | 4, 4.1, 10.3 |
| `csv_writer` | Writes the header and account rows with 4 decimal places via `BufWriter`. Normalises amounts before output: `-0.0000` is never printed (T4) | 6 |

### 3.4. `interfaces`

Entry point. Knows about all layers and wires them together.

- `main` reads `LOG_FILE` and `RUST_LOG` into `AppConfig` and calls `cli::run(args, config)` (AD-12).
- `cli::run` parses arguments via `clap` (AD-13) and carries out the startup order (spec, 2.1): log, input file, processing, output.
- Passes the CSV reader and the functions `logging::rejected` and `logging::accepted` as closures to `process`.
- Writes the result via `csv_writer` and writes the summary line (`logging::summary`).
- Maps `Outcome` and fatal errors to exit codes (spec, 2).

---

## 4. Data and error flows

```
file ─▶ csv_reader ─▶ io::Result<Row<RowError>> ─▶ process ─▶ PaymentEngine::apply
                          │                            │
                          │ Row { content: Err }       ├─ Ok(())     → accepted += 1, on_accepted
                          │   → on_rejected (data)     └─ Err(error) → on_rejected (category via category_of)
                          │ Err(io::Error) (fatal)
                          ▼
                     abort, code 74
```

- **Fatal and per-row errors are separated by type.** `Err(io::Error)` from the reader aborts processing (code `74`). A `Row` with `content: Err(InvalidRow)` rejects one row, and processing continues.
- **Checks 1–5** (row parsing) are knowledge of the CSV format, so they live in `csv_reader`. This also covers the ban on `amount` for disputes (OQ-11) and the required `amount` for `deposit` and `withdrawal`. **Check 6** (repeated `tx`) is business knowledge and is done by the engine, even though its category is "data".
- **Rejections and accepted rows are passed via closures** `on_rejected(&Rejection)` and `on_accepted(&Accepted)`. A `Rejection` contains the line number, `client` and `tx` (if parsed), the category and the reason. Neither the engine nor `process` writes to the log. In tests the closures collect events into a `Vec`.
- **Totals** are computed by `process` and written to the log by `cli` (only for codes `0`, `3`, `4`).

---

## 4.1. Ports

Ports are declared by `application`; adapters in `infrastructure` implement them. Ports are expressed with standard library traits; there are no custom traits (AD-4).

| Port | Declared in | Expressed as | Adapter |
|---|---|---|---|
| Row source | `process` | `IntoIterator<Item = Result<Row<E>, F>>` | `CsvTransactionReader` implements `Iterator` |
| Rejections | `process` | `FnMut(&Rejection)` (AD-2) | `logging::rejected` |
| Accepted rows | `process` | `FnMut(&Accepted)` (AD-9) | `logging::accepted` |
| Accounts for output | `PaymentEngine::accounts()` | `Iterator<Item = &Account>` | `csv_writer::write_accounts` |

A future extension point is a deposit store (`DepositStore`) with a `HashMap` implementation and an on-disk one. It is not introduced now: there is no second implementation.

---

## 5. Reuse

- The engine has no global state. One `PaymentEngine` serves one transaction stream. Only the `tracing` subscriber is global, and `cli` installs it.
- A server that processes many concurrent streams creates one engine per stream, or shards clients across engines by `client`. The engine is `Send`, so it can be moved between threads.
- `process` accepts any iterator of rows, not only the CSV reader. The source can be a network stream or test data.

---

## 6. Testing by layer

| Layer | What is tested | Spec |
|---|---|---|
| `domain` | `Amount`, `Account` methods, `Transaction<Deposit>` transitions, overflow | 12.1 |
| `application` | Order of engine checks, `tx` registration, reopening a dispute and `dispute_count`, `process` counters and categories, `on_accepted` | 12.1 |
| `infrastructure` | Parsing of rows and the header (`csv_reader` on in-memory bytes), output format, log event format (subscriber writes to a buffer) | 12.2 |
| `interfaces` | Unit tests of `cli` (arguments, exit codes); end-to-end tests of the binary in `tests/cli.rs` on the `tests/data/` sets | 12.4 |

Coverage is measured with `cargo llvm-cov` (spec, 12.8). Invariant tests with `proptest` (12.5) are not implemented yet.

---

## 7. Public types and signatures

Signatures without bodies. `pub(crate)` marks what does not leave the crate. Errors implement `Display` via `thiserror` (T3).

### 7.1. `domain`

```rust
// domain/ids.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(u16);   // From<u16>, Display

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TxId(u32);       // From<u32>, Display
```

```rust
// domain/money.rs
/// Operation amount: strictly > 0, at most 4 decimal places (spec 4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Amount(Decimal);

impl Amount {
    pub const MAX_SCALE: u32 = 4;
}

/// Normalises the value; errors are AmountNotPositive, AmountTooPrecise(MAX_SCALE).
impl TryFrom<Decimal> for Amount { type Error = DomainError; }
impl From<Amount> for Decimal {}
```

```rust
// domain/account.rs
#[derive(Debug)]
pub struct Account { /* client_id, available: Decimal, held: Decimal, locked: bool */ }

impl Account {
    pub fn new(client_id: ClientId) -> Self;
    pub fn client_id(&self) -> ClientId;
    pub fn available(&self) -> Decimal;
    pub fn held(&self) -> Decimal;
    /// available + held; always representable, deposit guarantees this (OQ-25).
    pub fn total(&self) -> Decimal;
    pub fn is_locked(&self) -> bool;

    /// First all new values via checked_*, then the write (T2):
    /// on error the account does not change.
    pub fn deposit(&mut self, amount: Amount) -> Result<(), DomainError>;     // locked, overflow
    pub fn withdraw(&mut self, amount: Amount) -> Result<(), DomainError>;    // locked, insufficient funds
    pub fn hold(&mut self, amount: Amount) -> Result<(), DomainError>;        // dispute
    pub fn release(&mut self, amount: Amount) -> Result<(), DomainError>;     // resolve
    pub fn charge_back(&mut self, amount: Amount) -> Result<(), DomainError>; // chargeback and lock
}
```

```rust
// domain/transaction.rs
/// Parsed input row. Only deposit and withdrawal have an amount.
#[derive(Debug)]
pub struct Event {
    pub client: ClientId,
    pub tx: TxId,
    pub kind: EventKind,
    pub amount: Option<Amount>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind { Deposit, Withdrawal, Dispute, Resolve, Chargeback }

/// Accepted transaction of kind K: Deposit or Withdrawal.
#[derive(Debug)]
pub struct Transaction<K> { /* client_id, tx_id, kind: K */ }

impl<K> Transaction<K> {
    pub fn client_id(&self) -> ClientId;
    pub fn tx_id(&self) -> TxId;
}

/// Processed deposit: everything disputes need (spec 5.2).
#[derive(Debug)]
pub struct Deposit { /* amount, state: DisputeState, dispute_count: u32 */ }

#[derive(Debug)]
pub struct Withdrawal { /* amount */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeState { Processed, Disputed, ChargedBack }

impl Transaction<Deposit> {
    pub fn amount(&self) -> Amount;
    pub fn state(&self) -> DisputeState;
    pub fn dispute_count(&self) -> u32;

    /// Transitions of section 8. They change the deposit and the account only if the account
    /// accepted the change; otherwise neither the deposit nor the account changes (T2).
    pub fn dispute(&mut self, account: &mut Account) -> Result<(), DomainError>;
    pub fn resolve(&mut self, account: &mut Account) -> Result<(), DomainError>;
    pub fn charge_back(&mut self, account: &mut Account) -> Result<(), DomainError>;

    pub fn ensure_transaction_by(&self, event: &Event) -> Result<(), DomainError>; // TransactionMismatch
    pub fn ensure_owned_by(&self, event: &Event) -> Result<(), DomainError>;       // ClientMismatch
}

impl Transaction<Withdrawal> {
    pub fn amount(&self) -> Amount;
}

/// Event kind and presence of an amount must match, otherwise UnsupportedEvent.
impl TryFrom<Event> for Transaction<Deposit> { type Error = DomainError; }
impl TryFrom<Event> for Transaction<Withdrawal> { type Error = DomainError; }
```

```rust
// domain/error.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    AmountNotPositive,      // "amount must be greater than zero"
    AmountTooPrecise(u32),  // "amount has more than {0} decimal places"
    AccountLocked,          // "account locked"
    InsufficientFunds,      // "insufficient funds"
    ClientMismatch,         // "client does not own tx"
    TransactionMismatch,    // "event refers to another tx"
    DisputeAlreadyOpen,     // "dispute already open"
    NotDisputed,            // "tx is not under dispute"
    AlreadyChargedBack,     // "already charged back"
    UnsupportedEvent,       // "unsupported event"
    Overflow,               // "arithmetic overflow"
}
```

### 7.2. `application`

```rust
// application/engine.rs
#[derive(Debug, Default)]
pub struct PaymentEngine { /* accounts, deposits, used: UsedTxIds */ }

impl PaymentEngine {
    pub fn new() -> Self;
    /// Checks 6–9+ from 10.3 in order. A rejected event does not change balances,
    /// but a deposit or withdrawal that passed the duplicate check consumes the tx (5.3).
    pub fn apply(&mut self, event: Event) -> Result<(), ApplicationError>;
    /// Accounts in arbitrary order.
    pub fn accounts(&self) -> impl Iterator<Item = &Account>;
}
```

```rust
// application/error.rs
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApplicationError {
    DuplicateTx,              // "duplicate tx"
    AccountNotFound,          // "account does not exist"
    NotADeposit,              // "tx is not a processed deposit"
    Domain(DomainError),      // transparent
}
```

```rust
// application/used_tx.rs
#[derive(Debug, Default)]
pub(crate) struct UsedTxIds(HashSet<TxId>);

impl UsedTxIds {
    pub(crate) fn contains(&self, tx: &TxId) -> bool;
    pub(crate) fn insert(&mut self, tx: TxId) -> bool;
}
```

```rust
// application/process.rs
/// Input row. E is the parse error of a specific format.
pub struct Row<E> {
    pub line: u64,
    pub content: Result<Event, InvalidRow<E>>,
}

pub struct InvalidRow<E> {
    pub client: Option<ClientId>,
    pub tx: Option<TxId>,
    pub error: E,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category { Data, Business }

/// What on_rejected receives (AD-2).
pub struct Rejection<'a> {
    pub line: u64,
    pub client: Option<ClientId>,
    pub tx: Option<TxId>,
    pub category: Category,
    pub reason: &'a dyn fmt::Display,
}

/// What on_accepted receives (AD-9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
    pub line: u64,
    pub client: ClientId,
    pub tx: TxId,
    pub kind: EventKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub read: u64,
    pub accepted: u64,
    pub rejected_data: u64,
    pub rejected_business: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome { Clean, BusinessRejects, DataRejects }

impl Summary {
    /// DataRejects takes precedence over BusinessRejects.
    pub fn outcome(&self) -> Outcome;
}

/// An Err from input is fatal: processing stops and the error is returned (code 74 in cli).
pub fn process<E: fmt::Display, F>(
    engine: &mut PaymentEngine,
    input: impl IntoIterator<Item = Result<Row<E>, F>>,
    on_rejected: impl FnMut(&Rejection<'_>),
    on_accepted: impl FnMut(&Accepted),
) -> Result<Summary, F>;
```

### 7.3. `infrastructure`

```rust
// infrastructure/config.rs
/// Filled in by main from the environment; values are not validated (AD-12).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppConfig {
    pub log_file: Option<PathBuf>,   // LOG_FILE
    pub log_filter: Option<String>,  // RUST_LOG, default "error"
}
```

```rust
// infrastructure/logging.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTarget { Stderr, File }

/// Failure to open LOG_FILE → code 73. EnvFilter::new is non-strict (C2).
/// Format: time, level, key=value fields, no module name and no ANSI (C4).
pub fn init(config: &AppConfig) -> io::Result<LogTarget>;

/// ERROR kind=rejected line=.. client=.. tx=.. category=.. reason="..";
/// client and tx only if parsed.
pub fn rejected(rejection: &Rejection<'_>);

/// DEBUG kind=accepted line=.. client=.. tx=.. type=..
pub fn accepted(accepted: &Accepted);

/// INFO kind=summary read=.. accepted=.. rejected_data=.. rejected_business=..
pub fn summary(summary: &Summary);

/// ERROR kind=fatal error=".."; to stderr exactly once, including with
/// RUST_LOG=off and before init (C5).
pub fn report_fatal(target: LogTarget, error: &dyn fmt::Display);
```

```rust
// infrastructure/csv_reader.rs
pub struct CsvTransactionReader<R> { /* input, buf, line, seen_data */ }

impl<R: io::BufRead> CsvTransactionReader<R> {
    pub fn new(input: R) -> Self;
}

/// Empty lines and the header are not yielded (spec 4, OQ-30).
impl<R: io::BufRead> Iterator for CsvTransactionReader<R> {
    type Item = io::Result<Row<RowError>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RowError {
    InvalidUtf8,
    FieldCount(usize),
    UnknownType,
    InvalidClient,
    InvalidTx,
    MissingAmount,
    InvalidAmount,         // not parsed by Decimal::from_str_exact
    Amount(DomainError),   // AmountNotPositive, AmountTooPrecise
    UnexpectedAmount,
}
```

```rust
// infrastructure/csv_writer.rs
pub fn write_accounts<'a, W: io::Write>(
    out: W,
    accounts: impl IntoIterator<Item = &'a Account>,
) -> io::Result<()>;

/// Amount with exactly 4 decimal places; -0 is normalised to 0 (OQ-28, T4).
struct Fixed4(Decimal);   // impl Display
```

### 7.4. `interfaces`

```rust
// interfaces/cli.rs
pub use crate::infrastructure::config::AppConfig;

/// Arguments are parsed by clap (derive), help text comes from the doc comment.
#[derive(Debug, clap::Parser)]
struct Args { input: PathBuf }

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

impl From<Outcome> for ExitStatus {}

/// Argument parse error gives 64, --help and --version give 0 (AD-13).
pub fn run(args: impl IntoIterator<Item = OsString>, config: AppConfig) -> ExitStatus;
```

```rust
// main.rs
fn main() {
    let config = AppConfig {
        log_file: env::var_os("LOG_FILE").map(PathBuf::from),
        log_filter: env::var("RUST_LOG").ok(),
    };
    std::process::exit(cli::run(env::args_os(), config) as i32);
}
```

---

## 8. Decisions made

| # | Question | Decision |
|---|---|---|
| AD-1 | Where the CSV reader and CSV writer live | In `infrastructure`. `interfaces` is responsible only for the entry point and exit codes |
| AD-2 | How the engine reports rejections without a log | Via the `on_rejected` closure, without a port trait |
| AD-3 | Split between `domain` and `application` | Account rules and dispute transitions are methods of domain types. `PaymentEngine` in `application` only performs the checks in order and applies the changes |
| T1 | Identifiers | Newtypes `ClientId(u16)` and `TxId(u32)`, not aliases |
| T2 | State changes | `Account` methods and `Transaction<Deposit>` transitions modify `&mut self`, but first compute all new values via `checked_*` and write them only on success. On error neither the account nor the deposit changes. (The original option "methods return a new value" was replaced when the domain was implemented) |
| T3 | `Display` for errors | Via `thiserror` |
| T4 | Normalising amounts for output | In `csv_writer` (`Fixed4`), not in the domain |
| T5 | `dispute_count` overflow | `saturating_add` |
| AD-4 | Ports | `std` traits (`Iterator`, `FnMut`), no custom port traits |
| AD-5 | Reading CSV | `BufRead::read_until` and splitting on commas instead of the `csv` crate: with CRLF and empty lines `csv` reports a wrong `position().line()`, and OQ-21 requires the physical line number. Quotes are disabled anyway (OQ-20) |
| AD-6 | Missing `amount` on `deposit` and `withdrawal` | Rejected by the parser (`MissingAmount`). The `UnsupportedEvent` branch in the engine stays as a guard for other sources |
| AD-7 | `client` and `tx` in a rejected row | Parsed up front and included in the rejection, even if another check failed earlier. The reason is the first failed check |
| AD-8 | CSV output | `BufWriter` and `writeln!` without the `csv` crate: there is nothing to escape |
| C1 | Configuration source | Process environment only; `.env` and `dotenvy` are not used (OQ-23, revised 2026-11-09) |
| C2 | Error in `RUST_LOG` | Non-strict `EnvFilter::new`: invalid directives are skipped with a warning on stderr, an empty value equals `error`, `eror` is accepted as a target filter |
| C3 | Validating `LOG_FILE` | None; problems with the file, including an empty value, are detected when it is opened (code `73`) |
| C4 | Log format | Same for stderr and file: time, level, `key=value`, no module and no ANSI; `reason` and `error` in quotes |
| C5 | Fatal error | `report_fatal`: to the log if level `error` is enabled; to stderr if the log goes to a file or `error` is filtered out |
| C6 | Log testability | The subscriber is built separately from `init`; tests write to a buffer via `with_default` |
| C7 | Event format | `logging::rejected`, `logging::accepted`, `logging::summary`; `cli` only passes them on |
| AD-9 | Accepted operations in the log at `debug` (3.1) | `process` gained the closure `on_accepted(&Accepted)` (`line`, `client`, `tx`, `kind`), paired with `on_rejected`. Format: `DEBUG kind=accepted line=.. client=.. tx=.. type=..` |
| AD-10 | Directory instead of an input file | `cli` checks `metadata().is_dir()` after opening: code `66`, not `74` on the first read |
| AD-11 | Module visibility | `domain`, `application` and `interfaces` are public (the engine can be reused, `main` calls `cli::run`), `infrastructure` is `pub(crate)` |
| AD-12 | Configuration | `main` reads `LOG_FILE` and `RUST_LOG` into `AppConfig` and passes it to `cli::run`; there is no value validation and no code `78` (2026-11-09) |
| AD-13 | Argument parsing | The `clap` crate (derive) instead of hand-written parsing: `--help`/`--version` help and clear error messages. `Args::try_parse_from`; a parse error gives `64` (`clap` defaults to `2`), `--help` and `--version` give `0` |
