# Payments Engine Specification

Version: draft of 2026-11-07, brought in line with the code on 2026-11-09.

This document describes the engine's behaviour, taking into account the decisions made in discussion. Rationale and an analysis of real-world practice are in [`open-questions.md`](open-questions.md); references to its items are given in the form "see OQ-N".

---

## 1. Purpose

A console program reads a CSV file of client transactions, applies them in order to the clients' accounts and outputs the final state of all accounts as CSV.

---

## 2. Command-line interface

```
cargo run -- transactions.csv > accounts.csv
```

| Aspect | Behaviour |
|---|---|
| Arguments | Exactly one: the path to the input file |
| stdout | Only the accounts CSV (section 6). Exception: help for `--help` and `--version` |
| stderr | The log (section 3.1), unless `LOG_FILE` is set. Fatal errors are always written to stderr |
| Exit code | Reflects the outcome of processing, including partial success (table below, see OQ-13) |

| Code | When | stdout |
|---|---|---|
| `0` | All rows applied | Full result |
| `3` | Some operations rejected by business rules (category "business" in 10.2) | Full result |
| `4` | At least one row with invalid data (category "data" in 10.2). Takes precedence over `3` | Full result |
| `64` | `EX_USAGE`: invalid arguments (not exactly one path, unknown flag) | Empty |
| `66` | `EX_NOINPUT`: the input file cannot be opened | Empty |
| `73` | `EX_CANTCREAT`: the log file from `LOG_FILE` cannot be opened | Empty |
| `74` | `EX_IOERR`: I/O error while reading the input or writing the result | Empty or incomplete, must not be used |
| `101` | Rust panic. Must not happen | Empty |

At the end of processing, a summary line is written to the log at `info` level with the fields `read` (data rows read, excluding the header and blank lines), `accepted` (accepted), `rejected_data` (rejected, category "data") and `rejected_business` (rejected, category "business"). `read = accepted + rejected_data + rejected_business` always holds. See OQ-24.

The summary line is written only on exit with codes `0`, `3` and `4`. On a fatal error, including `74` in the middle of processing, it is not written: the totals of an incomplete run mean nothing, and the cause is reported by the fatal error message. See OQ-31.

### 2.1. Startup order

See OQ-17. The steps run strictly in order, and the first one that fails determines the exit code.

| Step | Action | Code on error |
|---|---|---|
| 1 | Parse arguments (`clap`): exactly one path. `--help` and `--version` print help to stdout and exit with code `0` | `64` |
| 2 | Initialise the log, open `LOG_FILE` if set | `73` |
| 3 | Open the input file | `66` |
| 4 | Process the file and output the result | `0`, `3`, `4`, `74` |

- Configuration parameters (section 3) are read by `main` before step 1 and are not validated.
- Errors in steps 1–2 are written directly to stderr, because the log is not yet initialised. Errors in steps 3–4 are output according to the rules in 3.1.

---

## 3. Configuration

| Parameter | Source | Default | Meaning |
|---|---|---|---|
| `LOG_FILE` | Process environment variable | not set, i.e. log to stderr | Path to the log file. See 3.1 and OQ-15 |
| `RUST_LOG` | Same | `error` | Log level and filter in `EnvFilter` syntax. See 3.1 and OQ-15 |

- The parameters are read by `main` from the process environment (`std::env::var_os`, `std::env::var`) and passed to the CLI in an `AppConfig` struct. A `.env` file is not supported. See OQ-23.
- Values are not validated (OQ-29, revised 2026-11-09):
  - `LOG_FILE` is used as is. If the file cannot be opened (including when the value is empty), this is code `73`.
  - `RUST_LOG` is parsed by the lenient `EnvFilter::new`: invalid directives are skipped with a warning to stderr, an empty value equals `error`. A non-UTF-8 `RUST_LOG` is treated as not set.
- The engine does not read the environment and receives no configuration: the section 3 parameters are needed only by `main` and the log.

### 3.1. Logging

See OQ-15.

- Crates `tracing` and `tracing-subscriber` (feature `env-filter`).
- The log is written to stderr or, if `LOG_FILE` is set, to a file. stdout is never used for the log.
- Event levels:

| Level | Events |
|---|---|
| `error` | Fatal errors (10.1, field `kind=fatal`) and rejected rows (10.2, field `kind=rejected`): line number, `client`, `tx`, category, reason |
| `warn` | Not used |
| `info` | Summary line (section 2): `read`, `accepted`, `rejected_data`, `rejected_business` |
| `debug` | Accepted operations |

- By default (`RUST_LOG=error`) the log contains fatal errors and rejected rows. The summary line is visible with `RUST_LOG=info`.
- The format is text, one event per line: time (RFC 3339, UTC), level, fields as `key=value`. The module name is not output. ANSI colours are not used, either in the file or in stderr.
- Events and their fields:

| Event | Example (without time) |
|---|---|
| Rejected row | `ERROR kind=rejected line=12 client=3 tx=41 category=business reason="insufficient funds"` |
| Fatal error | `ERROR kind=fatal error="cannot open input in.csv: No such file or directory (os error 2)"` |
| Summary line | `INFO kind=summary read=5 accepted=4 rejected_data=0 rejected_business=1` |
| Accepted operation | `DEBUG kind=accepted line=2 client=1 tx=1 type=deposit` |

- The message for a rejected row contains the physical line number in the file (`line`, starting at 1, counting the header and blank lines), as well as `client` and `tx` if they could be parsed. Example: `ERROR kind=rejected line=12 client=3 tx=41 category=business reason="insufficient funds"`. See OQ-21.
- Fatal errors are always written to stderr. If the log is written to a file, they are written there too. If the log already goes to stderr, the message is output once, without a duplicate. Errors that occur before the log is initialised, and the copy of a fatal error when logging to a file or with `RUST_LOG=off`, are written directly to stderr as `error: <message>`.
- The log file is opened for appending. If it cannot be opened, this is a fatal error with code `73`.
- Writing is synchronous, because the program exits via `std::process::exit`, which does not run destructors.
- The engine does not write to the log. It returns the result of the operation (accepted, or rejected with a category and reason), and the CLI layer maintains the log.

---

## 4. Input format

CSV with a fixed column order: `type, client, tx, amount`. The header is optional. See OQ-12.

- Columns are identified by position, not by name.
- The first non-blank line is treated as the header and skipped if its fields, after trimming whitespace and removing quotes, case-insensitively equal `type`, `client`, `tx` and, if a 4th field is present, `amount`. Otherwise it is processed as a data row.
- A row with 3 fields is allowed for `dispute`, `resolve` and `chargeback`. A row with fewer than 3 or more than 4 fields is rejected.

| Field | Type | Rules |
|---|---|---|
| `type` | string | One of `deposit`, `withdrawal`, `dispute`, `resolve`, `chargeback`. Lowercased before comparison: `Deposit` and `DEPOSIT` are allowed. See OQ-27 |
| `client` | `u16` | Client identifier. Parsed with `u16::from_str` |
| `tx` | `u32` | Transaction identifier, globally unique. Parsed with `u32::from_str` |
| `amount` | decimal number | Required for `deposit` and `withdrawal`. For `dispute`, `resolve` and `chargeback` it is empty or missing |

General rules:

- Whitespace around values is trimmed.
- Quote handling in the CSV parser is disabled: a comma always separates fields, a line break always separates records. If a value, after trimming whitespace, is wrapped in a pair of `"`, the outer pair is removed and the value is trimmed again. An unpaired quote is not removed, and the field fails parsing. This also applies to header recognition. See OQ-20.
- Invalid UTF-8 in a row causes the row to be rejected (category "data").
- A UTF-8 BOM at the start of the file is ignored.
- A blank line is a line of zero length or consisting only of whitespace characters. Such lines are skipped and counted neither as processed nor as rejected. A line consisting only of separators (for example, `,,,`) is not considered blank and is rejected on field count or parsing. See OQ-30.
- Line breaks are LF or CRLF.
- Rows are in chronological order and are processed strictly in the order they appear.
- The file is read as a stream, row by row, rather than loaded into memory in full.

### 4.1. Rules for `amount`

- The number syntax is defined by the library: `rust_decimal::Decimal::from_str_exact`. No custom grammar is introduced. Therefore, for example, `+5` and `1_000` are allowed. The exact set of accepted forms is pinned down by the parsing unit tests (12.2). For `client` and `tx` the rules of `u16`/`u32::from_str` apply in the same way: `+5` and leading zeros are allowed. See OQ-26.
- `from_str_exact` is used specifically, not `from_str`: `from_str` silently rounds values with more than ~28 significant digits, which would violate the "no rounding" rule (OQ-2). `from_str_exact` returns an error in that case, and the row is rejected.

- The value is strictly greater than zero. Zero and negative amounts cause the row to be rejected.
- No more than 4 significant digits after the decimal point. Trailing zeros are allowed: `1.50000` equals `1.5`. The value `1.00005` causes the row to be rejected. Neither truncation nor rounding is applied. See OQ-2.
- The rules above apply only to `deposit` and `withdrawal`. For `dispute`, `resolve` and `chargeback` the `amount` field must be missing or empty after trimming whitespace. Any non-empty value means a data inconsistency (the system does not support partial disputes): the row is rejected with category "data". A dispute always concerns the full amount of the original deposit. This check is performed by the parser (infrastructure layer, CSV reading): the domain methods `dispute`, `resolve` and `charge_back` do not take an amount. See OQ-11.

---

## 5. Data model

### 5.1. Client account

| Field | Meaning |
|---|---|
| `available` | Funds available for withdrawal. May be negative (section 7.3) |
| `held` | Funds held under open disputes |
| `total` | `available + held`. Computed and always matches this formula |
| `locked` | The account is locked after a chargeback. There is no unlocking |

An account is created only by an accepted `deposit`. Any other operation for a client who does not yet have an account is rejected with the reason "account does not exist". A rejected `deposit` does not create an account. See OQ-10.

### 5.2. Stored transaction

Only deposits are stored, because only they can be disputed (see OQ-4).

| Field | Meaning |
|---|---|
| `client` | Owner of the transaction |
| `amount` | Deposit amount |
| `state` | Dispute state (section 8) |
| `dispute_count` | How many disputes have already been opened on this transaction. Increments with saturation: on reaching the `u32` maximum it stays there, without a panic. It only counts and does not affect processing |

### 5.3. Used transaction identifiers

See OQ-9.

- `tx` is unique across all `deposit` and `withdrawal` rows. It is checked against two structures: the store of processed deposits (5.2) and the set of other used `tx` values.
- The `tx` of a `deposit` or `withdrawal` row counts as used from the moment it passes check 6 of 10.3, regardless of the subsequent outcome. An accepted `deposit` is kept in the deposit store (5.2). All others (`withdrawal`, and a `deposit` rejected by a business rule) go into the set.
- Rows of category "data" (checks 1–6) do not register their `tx`, so a corrected row with the same `tx` may arrive later and will be accepted. See OQ-18.
- The set is hidden behind an internal engine interface, so that the implementation (`HashSet<u32>`) can be replaced, for example with a compressed bitmap.

---

## 6. Output format

CSV with a header. Columns: `client, available, held, total, locked`.

- One row for each existing account.
- Amounts are output with exactly 4 digits after the decimal point (zero padding, no rounding).
- Before output, each amount is normalised, so `-0.0000` is never output, only `0.0000`. See OQ-28.
- `locked` is output as `true` or `false`.
- Row order is undefined: no sorting is done, and the order may change from run to run. See OQ-14.

---

## 7. Transaction processing rules

### 7.1. Summary table

Notation: "reject" means skip the row without changes and write a message to the log at `error` level (3.1). The table lists the conditions but not their order: the order of checks and the categories are defined in 10.3. The row must also pass parsing (checks 1–5 of 10.3), and the computations must not overflow (section 9).

| Type | Preconditions (otherwise reject) | Effect |
|---|---|---|
| `deposit` | The account is not locked. `tx` has not been seen before. Neither `available + amount` nor `total + amount` overflows | If there is no account, it is created. `available += amount`. The transaction is stored in state `Processed` |
| `withdrawal` | The account exists. The account is not locked. `tx` has not been seen before. `available ≥ amount` | `available -= amount` |
| `dispute` | The account exists. `tx` is a processed deposit. `client` matches the owner. State is `Processed`. Neither `available - amount` nor `held + amount` overflows | `available -= amount`, `held += amount`. State `Disputed`, `dispute_count += 1` |
| `resolve` | The account exists. `tx` is a processed deposit. `client` matches the owner. State is `Disputed`. Neither `available + amount` nor `held - amount` overflows | `held -= amount`, `available += amount`. State `Processed` |
| `chargeback` | The account exists. `tx` is a processed deposit. `client` matches the owner. State is `Disputed`. `held - amount` does not overflow | `held -= amount`. State `ChargedBack`. The account is locked |

`total` is recalculated as `available + held` in every case.

### 7.2. Locked account

See OQ-3.

- `deposit` and `withdrawal` are rejected. In the model of this task, a rejected deposit means the funds are returned to the sender.
- `dispute`, `resolve` and `chargeback` on previously processed deposits continue to be processed, because they are initiated by an external party.

### 7.3. Negative `available`

See OQ-6.

- `withdrawal` never takes `available` below zero.
- `dispute` may take `available` below zero if the client has already withdrawn part of the disputed deposit. The full deposit amount is always moved to `held`.
- While `available` is negative, any `withdrawal` is rejected by the condition `available ≥ amount`.

### 7.4. Client match

See OQ-5. If the `client` in a `dispute`, `resolve` or `chargeback` does not match the owner of the `tx`, the row is rejected and no account is changed.

---

## 8. Deposit lifecycle

See OQ-7.

```
                 dispute
  Processed  ─────────────────────────────▶  Disputed
      ▲                                          │
      │               resolve                    │
      └──────────────────────────────────────────┤
                                                 │ chargeback
                                                 ▼
                                            ChargedBack (terminal)
```

| Current state | `dispute` | `resolve` | `chargeback` |
|---|---|---|---|
| `Processed` | → `Disputed` | reject | reject |
| `Disputed` | reject (duplicate) | → `Processed` | → `ChargedBack` |
| `ChargedBack` | reject | reject | reject |

- After a resolve, a dispute can be opened again: the amount is moved to `held` again.
- The number of cycles is unlimited. `dispute_count` only counts opened disputes and does not affect processing.

---

## 9. Monetary arithmetic

See OQ-8.

- All amounts have type `rust_decimal::Decimal`. Using `f32` and `f64` is forbidden.
- Only addition and subtraction are used, so the precision of the result never exceeds 4 digits and no rounding is needed.
- Operations are performed via `checked_add` and `checked_sub`. Overflow does not cause a panic: the transaction is rejected and the account state does not change.
- Invariant: `total = available + held` is always representable in `Decimal`. Only `deposit` can increase `total`, which is why it checks the new `total` for overflow. `dispute` and `resolve` move an amount between `available` and `held`, and `chargeback` decreases `total`. See OQ-25.
- The invariant on `total` does not bound `available` and `held` individually. `withdrawal` decreases `total`, after which a `deposit` can again credit almost `Decimal::MAX`. The sum of disputed deposits is therefore unbounded, and `dispute` can take `available` below `-Decimal::MAX` and `held` above `Decimal::MAX`, even though `total` stays representable. Therefore `dispute`, `resolve` and `chargeback` also compute the new `available` and `held` via `checked_*` and are rejected on overflow: the account does not change, nor do the dispute state and `dispute_count`. See OQ-25, clarification of 2026-11-08.

---

## 10. Error handling

See OQ-1.

### 10.1. Fatal errors

A message to stderr (and to the log, if it is already initialised and writes to a file), non-zero exit code (section 2). The result is output only after the whole file has been processed, so on a fatal error stdout is empty. Exception: an error writing the result itself (code `74`): the output may be incomplete and must not be used.

- Wrong number of arguments.
- The input file cannot be opened (including when the path points to a directory).
- I/O error while reading or writing the result.
- The log file cannot be opened.

### 10.2. Row rejection

The row is skipped, state does not change, a message with the line number, category and reason is written to the log at `error` level, and processing continues.

| Reason | Category | Source |
|---|---|---|
| The row cannot be parsed (invalid UTF-8, wrong number of fields, invalid `client` or `tx`) | data | OQ-1, OQ-12, OQ-20 |
| Unknown `type` | data | OQ-1 |
| `deposit` or `withdrawal` has no `amount` | data | OQ-1 |
| `amount` ≤ 0 or not parsed by `from_str_exact` (including excess precision) | data | OQ-1, OQ-26 |
| More than 4 significant digits after the decimal point | data | OQ-2 |
| Non-empty `amount` on `dispute`, `resolve` or `chargeback` | data | OQ-11 |
| Duplicate `tx` on `deposit` or `withdrawal` | data | OQ-9, OQ-13 |
| Client account does not exist (operation other than `deposit`) | business | OQ-10 |
| Insufficient funds for `withdrawal` | business | spec |
| `deposit` or `withdrawal` operation on a locked account | business | OQ-3 |
| `tx` is not a processed deposit (unknown, a withdrawal or a rejected deposit) | business | spec, OQ-4 |
| `client` does not match the owner of `tx` | business | OQ-5 |
| Transition not allowed in the current state (section 8) | business | spec, OQ-7 |
| Overflow during computation | business | OQ-8 |

The category determines the exit code (section 2): "data" gives `4`, "business" gives `3`.
The program must not panic on any input.

### 10.3. Order of row checks

See OQ-16. Checks run strictly in order. A row is rejected at the first check it fails, and only that check's reason is written to the log. Therefore category "data" always takes precedence over "business".

| No. | Check | For which types | Category |
|---|---|---|---|
| 1 | The row is valid UTF-8, field count is 3 or 4 | all | data |
| 2 | `type` is valid | all | data |
| 3 | `client` is a valid `u16` | all | data |
| 4 | `tx` is a valid `u32` | all | data |
| 5 | `amount` per the rules in 4.1 (performed by the parser) | all | data |
| 6 | `tx` has not been used yet | `deposit`, `withdrawal` | data |
| 7 | The account exists | all except `deposit` | business |
| 8 | The account is not locked (if it exists) | `deposit`, `withdrawal` | business |
| 9+ | Per-type rules (below) | | business |

Per-type rules, in order:

- `deposit`: no overflow of either `available + amount` or `available + held + amount` (the new `total`). See OQ-25.
- `withdrawal`: `available ≥ amount`.
- `dispute`: `tx` is a processed deposit; `client` matches the owner; state is `Processed`; no overflow.
- `resolve`, `chargeback`: `tx` is a processed deposit; `client` matches the owner; state is `Disputed`; no overflow.

An unknown `tx`, the `tx` of a withdrawal and the `tx` of a rejected deposit give the same reason: "`tx` is not a processed deposit".

All disallowed dispute transitions, including repeats (`dispute` on `Disputed`, `chargeback` on `ChargedBack`), belong to category "business". Repeats are distinguished in the log by a separate reason, for example `dispute already open` or `already charged back`. See OQ-19.

---

## 11. Architecture

The detailed structure of the code is described in [`architecture.md`](architecture.md): four layers (`domain`, `application`, `infrastructure`, `interfaces`) as modules of a single crate.

- The engine is separated from CSV I/O. It accepts typed transactions and returns account state, knowing nothing about files, stdout or the environment.
- The engine can be unit-tested and reused, for example in a server that processes several concurrent streams.
- Only accounts, the processed-deposit data needed for disputes, and the set of other used `tx` values (5.3) are kept in memory.
- The code remains a single crate without a workspace.
- Clean, maintainable code takes priority over micro-optimisation.

---

## 12. Correctness checks

See OQ-22.

1. **Engine unit tests** (no I/O): all transitions in section 8, checking the rejection reason; rules 7.2–7.4; missing account (5.1); reopening a dispute and the increment of `dispute_count`; `tx` registration (5.3); order of checks when there are several reasons (10.3); overflow.
2. **Row parsing unit tests:** `amount` rules (4.1), including rejection on excess precision beyond 28 digits; `type` case; blank lines made of whitespace; whitespace trimming and quote removal; header recognition in all variants; field count, unknown `type`, invalid `client`, `tx` and UTF-8; no `tx` registration for rows of category "data".
3. **Configuration:** values are not validated, so there are no separate unit tests. Behaviour with an empty `LOG_FILE` and an invalid `RUST_LOG` is checked by end-to-end tests.
4. **End-to-end CLI tests** (`tests/`, running the program via `std::process::Command` with an explicit environment):
   - reference sets `tests/data/<scenario>/` with files `input.csv`, `expected.csv` and `exit_code`; output is compared ignoring row order, and the exit code is checked;
   - all fatal codes (`64`, `66`, `73`) with empty stdout, `--help` with code `0`;
   - log: only CSV in stdout; `LOG_FILE` is written in append mode; a fatal error appears in stderr exactly once, including when logging to a file and with `RUST_LOG=off`; an invalid or empty `RUST_LOG` gives level `error`.
5. **Invariant tests** (`proptest`, dev dependency; planned, not yet implemented): after every operation `total = available + held`, `total` is representable without overflow and `held ≥ 0`; the sum of `total` across accounts equals accepted deposits minus accepted withdrawals and chargebacks; parsing an arbitrary row does not panic.
6. **Streaming:** a test on a generated input of 1,000,000 rows marked `#[ignore]`, run via `cargo test --release -- --ignored`.
7. **Code quality:** `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` with no findings.
8. **Code coverage:** measured with [`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) (`cargo install cargo-llvm-cov`, requires the `llvm-tools-preview` component). Summary: `cargo llvm-cov --summary-only`; uncovered lines: `cargo llvm-cov --text` or the HTML report `cargo llvm-cov --html` (in `target/llvm-cov/html`). There is no hard threshold. Every uncovered branch of production code must have an explanation: a defensive branch unreachable through the public API, or an error the standard library does not produce in this configuration (for example, `?` after writing the header to a `BufWriter`). Branches of test helper code (`panic!` in helpers, unused methods of fakes) are not counted.

---

## 13. Open questions

All questions identified while assembling the specification have been resolved (2026-11-07). The table is kept as history.

| No. | Question | Decision |
|---|---|---|
| Q-1 | ~~Should `tx` uniqueness be checked for withdrawals?~~ | **Resolved:** see 5.3 and OQ-9 |
| Q-2 | ~~Is an account created if the client's row is rejected?~~ | **Resolved:** an account is created only by an accepted `deposit`, see 5.1 and OQ-10 |
| Q-3 | ~~What to do if `dispute`, `resolve` or `chargeback` has an `amount`?~~ | **Resolved:** the row is rejected, category "data", code `4`, see 4.1 and OQ-11 |
| Q-4 | ~~Is a header in the input file mandatory?~~ | **Resolved:** the header is optional, column order is fixed, see 4 and OQ-12 |
| Q-5 | ~~Does the number of rejected rows affect the exit code?~~ | **Resolved:** yes, partial success has its own code, see OQ-13 |
| Q-6 | ~~Should output rows be sorted?~~ | **Resolved:** no, order is undefined, see 6 and OQ-14 |
