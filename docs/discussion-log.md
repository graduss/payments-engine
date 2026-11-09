# Discussion log

A chronological log of the work on the specification, architecture, tests and documentation (2026-11-07 to 2026-11-09). Detailed analyses and final decisions are in [`open-questions.md`](open-questions.md) (OQ-N), the consolidated specification is in [`specification.md`](specification.md), and the code structure is in [`architecture.md`](architecture.md) (AD-N).

Requests to generate the main code and housekeeping requests (commits, translations, stops) are not included in the log.

---

## Part 1. Questions the problem statement does not answer

### 1. Initial questions

**User:**

> here are my questions that were not answered
>
> 1. The task defines the input CSV format, but the file still comes from an external source, so it may contain invalid rows, for example text where a number is expected, or a deposit with no amount. How should I handle such errors: stop processing and print an error to stderr, or skip the bad row and continue?
> 2. Amounts are said to have a precision of up to four decimal places. If a value comes with more decimal places, should the extra digits be truncated, or should the value be rounded?
> 3. After a chargeback the account is frozen. If the file keeps sending transactions for that client, should they be ignored, or still applied, with the chargeback only setting the locked flag?
>
> write them to an md file and let's discuss how these cases are handled in real fintech systems

**Assistant:** created `open-questions.md` with an analysis of real-world practice for each question:

1. Errors are split by level (file, batch, record). Only the bad record is rejected, with a reason code (ACH return codes, ISO 20022 `pain.002`). Proposal: I/O errors are fatal; a bad row is skipped with a message.
2. An input amount with excess precision is rejected, not rounded: silently changing an amount breaks reconciliation. Rounding is applied only to the results of calculations.
3. Freeze modes: debit freeze, full freeze, closed. After a chargeback a full freeze is usual. Disputes and chargebacks are initiated by an external party and cannot be refused. Proposal: block deposit and withdrawal, keep processing disputes.

### 2. Decision on locking

**User:**

> Agreed with the proposed option: after a chargeback the account is fully frozen.

**Assistant:** marked OQ-3 as agreed.

### 3. Decisions on row errors and precision

**User:**

> Agreed with the proposals for items 1 and 2: an invalid row is skipped with a message, and an amount with excess precision is rejected, with no rounding or truncation.

**Assistant:** marked OQ-1 and OQ-2. Proposed to go through the remaining open points: which transactions can be disputed, client match, negative `available`, repeated dispute.

### 4. Additional open points

**User:**

> Yes, analyse these cases and add them to the document in the same form: real-world practice and a proposal.

**Assistant:** added OQ-4…OQ-7 with real-world practice:

- OQ-4: only a deposit can be disputed (in reality, a dispute over an outgoing operation works in the opposite direction, as a provisional credit).
- OQ-5: if the client does not match, the row is ignored (the account is determined by the original transaction).
- OQ-6: a dispute may take `available` negative (a chargeback does not depend on the balance; acquirer reserves).
- OQ-7: the dispute lifecycle; proposed to allow a repeated dispute after resolve, as an analogue of pre-arbitration.

### 5. Reopening a dispute

**User:**

> Agreed on OQ-4…OQ-6, mark them as agreed. A clarification on OQ-7: a dispute after a resolve moves the transaction amount back to held, which means the dispute is reopened.

**Assistant:** marked OQ-4…OQ-7 and clarified OQ-7: a repeated dispute moves the amount to held again. Added on its own initiative "the number of cycles is unlimited" and asked whether a limit is needed.

### 6. Dispute limit

**User:**

> For OQ-7 I want the implementation to support a limit on the number of disputes per transaction. The limit is an optional setting in `.env`; if it is not set, the number of disputes is unlimited.

**Assistant:** proposed the details: a `MAX_DISPUTES_PER_TX` variable, every dispute counts including the first, exceeding the limit means rejection, an invalid value and `0` stop the program with an error, the engine receives `Option<NonZeroU32>`, `.env` in `.gitignore`, `.env.example` in the repository. The user agreed to stopping on an invalid value. The limit was later dropped (see "Dropping the dispute limit").

### 7. Money arithmetic

**User:**

> We use `rust_decimal` for money. Record this in the decision log and explain why binary floating point is unsuitable for money: because of how IEEE 754 works, decimal fractions are not represented exactly and the error accumulates.

**Assistant:** added OQ-8: why `f64` is not suitable (IEEE 754, binary base, error accumulation, unreliable comparison, limited precision), why `rust_decimal` (a 96-bit integer and a decimal scale), `checked_*` instead of operators, and the alternative, a custom fixed-point type on `i64`.

### 8. Precision in the engine

**User:**

> We have already decided on excess precision: such a transaction is rejected and logged. The engine itself only adds and subtracts, so precision cannot grow inside it, and the engine needs no rounding.

**Assistant:** rewrote the end of OQ-8: precision is controlled only on input, the engine only adds and subtracts, so rounding is not needed, and output is padding with zeros. Removed the mention of rounding modes.

---

## Part 2. Assembling the specification and open questions Q-1…Q-6

### 9. Assembling the specification

**User:**

> Before implementation, let's refine the specification. First, assemble a single document from the decisions agreed so far.

**Assistant:** created `specification.md` (interface, configuration, formats, data model, rules, lifecycle, arithmetic, errors, architecture, tests) and identified six undiscussed questions Q-1…Q-6.

### 10. Q-1: `tx` uniqueness for withdrawal

**User:**

> Let's go through the open questions in order, starting with Q-1.

**Assistant:** OQ-9: idempotency keys, counterparty guarantees are not trusted, a duplicate withdrawal is more dangerous than a duplicate deposit, the key is consumed together with the result. Options: no check, `HashSet`, a bitmap, `roaring`. Proposed `HashSet` plus a `HashMap` of deposits.

**User:**

> Agreed, mark Q-1 as agreed. Moving on to Q-2.

### 11. Q-2: creating an account on a rejected row

**Assistant:** OQ-10: in reality an account is not created by a transaction (KYC, ACH R03). Proposed option A (create on any parsed row) as a literal reading of the spec wording.

**User:**

> Here I choose option B. The task says nothing about invalid rows, so within its scope a parsed row counts as valid. Also add a rule: an account is created only by the first deposit; if a client's first operation is of any other type, it is ignored with a message on stderr.

**Assistant:** recorded option B: an account is created only by an accepted deposit, other operations without an account are rejected. Checked consistency with OQ-5. Analysed Q-3 (OQ-11): card schemes have partial chargebacks; proposed to ignore `amount` on dispute rows with a warning.

### 12. Q-3 and Q-4

**User:**

> Agreed, mark Q-3 as agreed and add the decision to the list of questions. Moving on to Q-4.

**Assistant:** marked OQ-11. Analysed Q-4 (OQ-12): in NACHA and ISO 20022 the header is mandatory, and a file without one is rejected. Proposed a mandatory header with columns matched by name.

**User:**

> I choose this option: the header is optional and the column order is fixed. Add the decision to the list of open questions and note that standards exist for such files and that real systems handle this differently. The choice is deliberate: I do not know whether the files used in automated checking will follow a standard.

**Assistant:** recorded the decision with its rationale. Added details: exact header recognition, handling of an unrecognised first row, rules for the number of fields.

**User:**

> Also add that, under these standards, real systems also verify the file's control totals.

**Assistant:** added a paragraph to OQ-12 about control totals (NACHA Batch/File Control, ISO 20022 `NbOfTxs` and `CtrlSum`) and noted that the task's input format has none.

### 13. Q-5: exit code

**User:**

> Agreed, mark Q-4 as agreed. Moving on to Q-5.

**Assistant:** OQ-13: batch processing outcomes (`ACCP`/`PART`/`RJCT`), the JCL return code convention. Proposed option A (`0` on partial success) because of the risk posed by the automated check.

**User:**

> I choose option B: partial success must show in the exit code. Let's also fix which code corresponds to which case.

**Assistant:** proposed a table: `0` success, `3` business rejections, `4` invalid data, `sysexits.h` codes for fatal errors (`64`, `66`, `74`, `78`), `101` panic. Asked where a repeated `tx` belongs.

**User:**

> Agreed with the code table. A repeated `tx` counts as invalid data, code 4. Moving on to Q-6.

**Assistant:** marked OQ-13, introduced the "data" and "business" categories in the table of reasons. Analysed Q-6 (OQ-14): deterministic output for reconciliation and audit, the random order of `HashMap`. Proposed sorting by `client`.

### 14. Revisiting Q-3

**User:**

> Back to Q-3: an `amount` field on dispute, resolve or chargeback must give code 4. Our system does not support partial refunds or partially won disputes, so an amount on these operations means the data is incompatible with the system.

**Assistant:** recorded the "data" category and code `4` for Q-3 (OQ-11). Asked whether to apply the row for the full amount or reject it.

**User:**

> Reject the whole row, code 4. Moving on to Q-6.

**Assistant:** recorded the rejection. Repeated the proposal for Q-6.

### 15. Q-6: output order

**User:**

> Output without sorting: the task states that row order in the output does not matter.

**Assistant:** recorded OQ-14: no sorting, tests compare regardless of order.

---

## Part 3. Logging

### 16. The `tracing` crate

**User:**

> Let's continue with the specification. For logging I propose the `tracing` crate: stderr by default, with the option to send the log to a file through a setting in `.env`.

**Assistant:** proposed OQ-15: `LOG_FILE`, `RUST_LOG` (default `info`), event levels, a text format with fields, fatal errors duplicated to stderr, appending to the file, code `73`, synchronous writes, the engine does not write to the log. Noted that `cargo init` must be run with an explicit `--name`.

**User:**

> Agreed, mark it as agreed, with one change: `RUST_LOG` defaults to `error`.

**Assistant:** marked OQ-15 with the `error` level. Proposed to treat an invalid `RUST_LOG` as configuration error `78`.

**User:**

> Rejected rows are logged at the `error` level.

**Assistant:** moved rejected rows to the `error` level, added the field `kind=fatal` / `kind=rejected`, marked `warn` as unused.

**User:**

> An invalid `RUST_LOG` silently falls back to the `error` level.

**Assistant:** recorded a silent fallback of an invalid `RUST_LOG` to `error`.

**User:**

> No, after all any invalid configuration value must lead to code 78.

**Assistant:** restored the rule: any invalid configuration value means code `78`; the exception is an inaccessible `LOG_FILE` (`73`).

---

## Part 4. First spec review (A–Zh)

### 17. Review

**User:**

> Reread the whole specification and look for contradictions.

**Assistant:** fixed inconsistencies (preconditions in 7.1, stdout on a fatal error, duplicate messages in stderr, what data is kept in memory). Identified questions A–Zh.

### 18. A: order of row checks

**User:**

> Let's go in order, starting with A.

**Assistant:** OQ-16: validation stages (syntax, idempotency, account, business rules), fail-fast, a single rejection reason. Proposed an order in which "data" always wins over "business".

**User:**

> Agreed, mark A as agreed. Moving on to B.

### 19. B: startup order

**Assistant:** OQ-17: fail fast at startup, all configuration errors at once. Order: arguments (`64`), `.env` (`78`), configuration (`78`), log (`73`), input file (`66`).

**User:**

> Agreed, mark B as agreed. Moving on to V.

### 20. V: registering `tx` for rows with a data error

**Assistant:** OQ-18: an idempotency key is stored only after validation passes. Proposed: rows in the "data" category do not register their `tx`.

**User:**

> Agreed, mark V as agreed. Moving on to G.

### 21. G: category of repeated dispute transitions

**Assistant:** OQ-19: card schemes flag duplicates separately. Proposed to keep all invalid transitions in the "business" category to preserve the check-order property, and to distinguish repeats by the reason in the log.

**User:**

> Agreed, mark G as agreed. Moving on to D.

### 22. D: CSV errors within a row

**Assistant:** OQ-20: an unclosed quote in `csv` can swallow the rest of the file. Proposed to disable quote parsing, reject rows with invalid UTF-8, strip the BOM, skip empty rows. Noted the risk for quoted fields.

**User:**

> We turn off quote handling in the CSV parser and strip outer quotes around values ourselves when parsing fields.

**Assistant:** recorded that our code strips the outer pair of quotes when parsing a field, with examples and properties. Analysed E (OQ-21): the physical line number in the log plus `client` and `tx`.

### 23. E and Zh

**User:**

> Agreed, mark E as agreed. Moving on to Zh.

**Assistant:** OQ-22: a test plan of seven groups (engine, parsing, configuration, end-to-end CLI tests with reference data sets, `proptest` invariants, large input, code quality).

**User:**

> Agreed with the test plan; we keep `proptest`. Mark it as agreed.

**Assistant:** marked OQ-22, rewrote section 12 of the specification.

---

## Part 5. Second spec review (Z, I)

### 24. Second spec review

**User:**

> Reread the whole specification once more and look for contradictions.

**Assistant:** fixed: UTF-8 in the check order and the table of reasons, aligning 7.1 with 10.3, the wording of 5.3, quote stripping during header recognition. Proposed Z (look for `.env` only in the current directory, not in parent directories as `dotenvy::dotenv()` does) and I (summary line fields `read`, `accepted`, `rejected_data`, `rejected_business`, with their equality as a self-check).

**User:**

> Agreed with both proposals.

**Assistant:** recorded OQ-23 and OQ-24.

---

## Part 6. Third spec review (K–R)

### 25. Review

**User:**

> Let's continue working on the specification.

**Assistant:** reread the specification and found seven gaps: K (overflow of `total` and a panic on output), L (grammar of `amount`, `client` and `tx`), M (case of `type`), N (`-0.0000` in the output), O (empty value of a configuration parameter), P (whitespace-only rows and header detection), R (the log summary line on a fatal error).

### 26. Decisions

**User:**

> K: agreed. L: we keep the library's number grammar and document it. M: lowercase `type` before comparing. N: agreed, normalise amounts before output. O: agreed. P: agreed. R: do not output the summary line at all, because stdout is taken by the resulting CSV.

**Assistant:** recorded OQ-25…30. On L, clarified: `Decimal::from_str` rounds values longer than ~28 significant digits, so parsing goes through `from_str_exact` with the same grammar but without rounding (otherwise OQ-2 is violated). On R, recorded nothing yet: the summary line is written to the log (stderr or `LOG_FILE`), not to stdout, so asked for the decision to be clarified.

**User:**

> I choose option 1: with code 74 the summary line is not written. Explain in which situations code 74 can occur at all.

**Assistant:** recorded OQ-31. Explained when `74` occurs: a read error in the middle of the file (disk failure, network file system, pipe) or an error writing the result (no disk space, a closed pipe with `| head`). Errors in the file's content do not lead to `74`; they are rejected row by row.

---

## Part 7. Architecture

### 27. Proposal

**User:**

> Now let's work on the architecture. I propose a clean architecture with four layers: domain, application, infrastructure and interfaces. The logger goes into infrastructure, the CLI into interfaces.

**Assistant:** supported it, with a caveat: the layers are modules of a single crate, and port traits are introduced only where they are needed. Proposed a module layout and three questions: A1 (CSV reader and CSV writer in `infrastructure`), A2 (rejections passed via a closure or a trait), A3 (account and dispute transition rules are methods of domain types; the engine in `application` only orchestrates).

**User:**

> A1: agreed. A2: rejections are passed through a closure. A3: agreed.

**Assistant:** created `architecture.md` with decisions AD-1…AD-3 and referenced it from section 11 of the specification.

### 28. Public types and signatures

**User:**

> Let's describe the public types and signatures, layer by layer.

**Assistant:** proposed signatures by layer and four decisions: T1 (newtypes for `ClientId` and `TxId`), T2 (transitions return a new value; the engine records changes only after all checks), T3 (`thiserror` for `Display` on errors), T4 (amount normalisation for output in `csv_writer`). Separately, `saturating_add` for `dispute_count`.

**User:**

> Agreed with the T decisions; I also accept the saturating dispute counter.

**Assistant:** added section 7 "Public types and signatures" and decisions T1–T5 to `architecture.md`, noted the saturation of `dispute_count` in spec 5.2.

### 29. Dropping the dispute limit, checking amount on disputes in the parser

**User:**

> I have decided not to implement the dispute limit: we keep only the count of disputes per transaction.

> We move the check from item 12 to the parser level.

**Assistant:** removed the dispute limit from the documents (2026-11-08): the `MAX_DISPUTES_PER_TX` parameter, `EngineConfig`, the `DisputeLimitReached` rejection reason and the "limit not reached" condition in transitions. A repeated dispute after resolve is still allowed and the number of cycles is unlimited; `dispute_count` grows via `saturating_add` and only counts disputes. The check for a non-empty `amount` on `dispute`, `resolve` and `chargeback` (OQ-11, spec 4.1) was assigned to the parser in the infrastructure layer: domain methods do not take an amount. Updated OQ-7 and OQ-11 in `open-questions.md`, the specification and `architecture.md`.

### 30. Overflow of `available` and `held` on disputes

**Assistant:** showed with a test that the `total` check in `deposit` does not protect `hold`: after `deposit` + `withdrawal` pairs of nearly `Decimal::MAX`, two `dispute` rows push `available` and `held` beyond the range of `Decimal`, and the engine panics. Proposed to bound `Amount` from above or to check for overflow in `hold`, `release` and `charge_back`.

**User:**

> I have fixed the overflow, please check.

**Assistant:** confirmed: `hold`, `release` and `charge_back` compute via `checked_*` and return `Overflow`, and the dispute transition propagates the error before changing state. Added the test `dispute_overflow_is_rejected_and_changes_nothing`. In the documents, added the overflow conditions for `dispute`, `resolve` and `chargeback` (spec 7.1 and 9), a clarification to OQ-25 and the `account` row in `architecture.md`.

---

## Part 8. The `infrastructure` layer: CSV

### 31. Layer plan

**User:**

> The application layer is done. Let's move on to the next layer.

**Assistant:** proposed building the layer in two steps (CSV first, then configuration and the log) and decisions I1–I7: a reader on the `csv` crate without `serde`, a fatal reader error is `io::Error`, `client` and `tx` in the rejection where possible, a missing `amount` on `deposit` and `withdrawal` is rejected by the parser, a writer based on `writeln!`. The user agreed. Checking the `csv` crate revealed that with CRLF and empty lines `position().line()` gives a wrong line number (for `a\r\nb\r\nc` it gives 1, 1, 2), which violates OQ-21. Proposed reading line by line via `read_until`.

### 32. Ports

**User:**

> What do you think about declaring ports in the application layer and implementing adapters for them in infrastructure?

**Assistant:** the ports already exist, just expressed through `std` traits: `Iterator` for the row source, `FnMut` for rejections, `Iterator<Item = &Account>` for output. Custom traits would duplicate them to no benefit. The real candidate for a port is the deposit store, but there is no second implementation yet.

**User:**

> Agreed: ports are expressed as standard library traits, and the reader is built on `read_until`.

**Assistant:** recorded AD-4…AD-8 and a "Ports" section in `architecture.md`.

### 33. Why not the `csv` crate

**User:**

> Explain once more why we do not use the `csv` crate to read the file.

**Assistant:** the main reason is line numbers: `csv` counts them incorrectly after CRLF and empty lines, while the log must point to the real line of the file. Besides, almost nothing of the crate would remain in use: quotes are disabled (OQ-20), the header is detected by our own rules, `serde` does not give an error for a specific field, and whitespace-only rows and UTF-8 are checked manually anyway. Manual reading via `read_until` is simpler and accurate. Risk: the reviewer may expect `csv`, so the decision needs to be explained in the README.

---

## Part 9. Tests and coverage

### 34. Checking the transferred code and tests for the CSV part

**User:**

> I have moved the code over. Check the transfer and add tests.

**Assistant:** ran the reader and writer unit tests on the transferred code. They found an inverted filter in `parse_amount` (`filter(|t| t.is_empty())`: 25 of 41 reader tests fail), `Iterator` instead of `IntoIterator` in `write_accounts`, and a clippy `explicit_auto_deref` warning. Added tests for streaming reads (an infinite source) and for writing accounts directly from the engine.

### 35. Coverage

**User:**

> Record in the documentation that coverage is measured with `cargo llvm-cov`. Check the coverage yourself and add the missing tests.

**Assistant:** recorded `cargo llvm-cov` in spec 12.8, `architecture.md` and `CLAUDE.md`, with the rule: there is no hard threshold, but every uncovered branch of production code must have an explanation. Coverage: 99.4% of lines. Added tests for `Display` of identifiers and for a write error in the middle of output. The remaining uncovered branches are defensive: `?` after writing the header to `BufWriter`, and overflow in `hold`, `release` and `charge_back`, which is unreachable given the account invariants.

### 36. Tests in files

**User:**

> I have applied the fixes, please check. Add the tests to the files yourself.

**Assistant:** added `tests` modules to `csv_reader.rs`, `csv_writer.rs` and `ids.rs` (45 tests), without touching production code. In total 129 tests, coverage 99.37% of lines.

---

## Part 10. Configuration and log

### 37. Decisions on configuration and log

**Assistant:** tested `EnvFilter`, the `tracing_subscriber::fmt` format and `dotenvy::from_path_iter` in practice and proposed decisions C1–C8. Findings: `EnvFilter::try_new` accepts the typo `eror` as a target filter; the default `fmt` format puts strings in quotes but not `Display` values, so fields must be written with `%` and `?` to get the format from 3.1; with `RUST_LOG=off` a fatal error would be lost unless it is printed to stderr separately.

**User:**

> Agreed with all the proposed decisions.

**Assistant:** recorded C1–C7 and the signatures in `architecture.md`.

### 38. Dropping `.env`

**User:**

> Let's drop `.env` and `dotenvy`: configuration comes only from the process environment, through `std::env::var` and `std::env::var_os`.

**Assistant:** removed `.env` and `dotenvy`: parameters are read only via `env::var_os`. The startup order was reduced to five steps. Updated spec 2.1 and 3, OQ-15, OQ-17 and OQ-23, and `architecture.md` (C1).

### 39. Configuration and log tests

**Assistant:** added tests: configuration parsing receives values as an argument rather than from the process environment, so tests do not interfere with each other; the log is checked via a subscriber that writes to a buffer (`with_default`). Checked: the format of `rejected`, `summary` and `fatal` events, escaping of quotes in the reason, the level filter, appending to the log file, and the rule "a fatal error appears in stderr exactly once".

---

## Part 11. End-to-end tests and code documentation

### 40. Reference data sets and end-to-end tests

**User:**

> Prepare reference test files for end-to-end checking of the program.

**Assistant:** created 12 reference data sets `tests/data/<scenario>/` (`input.csv`, `expected.csv`, `exit_code`): the example from the task, a clean run, a full dispute cycle, chargeback and lock, a negative balance, erroneous disputes, invalid rows, a mix of categories, format variants (BOM, CRLF, case, quotes, `+2`, `.5`, `1_000`), a file without a header, header only, an empty file. Expected results were computed by hand, and the binary matched on all of them. In `tests/cli.rs`: a run over all sets, fatal codes with empty stdout, the log only in stderr or `LOG_FILE`, appending, a fatal error with `RUST_LOG=off`, a large input of 1,000,000 rows (`#[ignore]`, 0.4 s in release). Along the way AD-9 (accepted operations at `debug` via `on_accepted`) and AD-10 (a directory instead of the input file gives `66`) were adopted.

### 41. Documentation in the code

**User:**

> Add documentation to the code: no boilerplate phrasing that reads as generated text, and no excess volume.

**Assistant:** added about 90 lines of comments across 17 files: one or two `//!` lines per module and comments on types and non-obvious functions. The comments explain why something is done the way it is rather than restating signatures; trivial getters have no comments. For example: why `hold` works on a locked account and can take `available` negative, why the reader does not use the `csv` crate, why `Fixed4` does not output `-0.0000`.

### 42. Simplifying the configuration

**User:**

> `config.rs` has become too complex. Make `AppConfig` a plain data structure: `main.rs` reads the values from the environment and builds the configuration itself. Remove the validity checks.

**Assistant:** reduced `config.rs` to the struct `AppConfig { log_file, log_filter }`, which `main` fills via `env::var_os` and `env::var`. The checks and code `78` were removed: an empty `LOG_FILE` gives `73` on opening, and `RUST_LOG` is parsed by the lenient `EnvFilter::new`. Replaced the code `78` tests with end-to-end tests of the new behaviour. Updated spec 2, 2.1, 3, 10.1, 12, OQ-29, and `architecture.md` (AD-12).

### 43. CLI on `clap`

**User:**

> Let's simplify `cli.rs` as well: use the `clap` crate to parse command-line arguments.

**Assistant:** moved argument parsing to `clap` (derive). A parse error gives `64` instead of `clap`'s standard code `2`; `--help` and `--version` give `0`. Added a test of the `Args` definition (`debug_assert`) and an end-to-end test of `--help`. Recorded AD-13.

---

## Part 12. Project documentation

### 44. Bringing the documents in line with the code

**User:**

> Now go through the documents in `docs/` and bring them in line with the code. Leave `discussion-log.md` alone for now.

**Assistant:** rewrote `architecture.md` according to the code: the domain part described an early version (`StoredDeposit`, `RejectReason`, methods returning a new value). Rephrased decision T2: methods modify `&mut self`, but write values only after all checks. In the specification, clarified the log format, stdout with `--help`, and the status of `proptest`. In `open-questions.md` the history was not rewritten; instead, "revised 2026-11-09" notes were added to OQ-13, 15, 17, 20, 21, 22, 23, 29.

### 45. README

**User:**

> Now let's write the README, starting with the Russian version, `README-ru.md`.

**Assistant:** before writing, measured performance: 5,000,000 deposits (143 MB of CSV) are processed in 1.8 s, with peak memory around 450 MB, roughly 90 bytes per processed deposit that has to be kept for disputes. Wrote the README: running and settings, exit codes, assumptions with references to OQ-N, error handling, how correctness was checked, efficiency, why not the `csv` crate, architecture, limitations (memory for deposits, `proptest` not done, the range of `Decimal`).

---

## Summary of decisions

| OQ | Topic | Decision |
|---|---|---|
| 1 | Invalid rows | Skip the row with a message; only I/O errors are fatal |
| 2 | Excess amount precision | The row is rejected, without rounding or truncation |
| 3 | Locked account | Full lock: deposit and withdrawal are rejected, disputes are processed |
| 4 | Which transactions can be disputed | Deposit only |
| 5 | Client mismatch | The row is rejected |
| 6 | Negative `available` | Allowed after a dispute |
| 7 | Repeated dispute | Allowed after resolve, without a limit; `dispute_count` only counts |
| 8 | Money type | `rust_decimal`, `checked_*`, no rounding |
| 9 | `tx` uniqueness | Checked for deposit and withdrawal |
| 10 | Account creation | Only by an accepted deposit |
| 11 | `amount` on disputes | The row is rejected at the parser level, category "data" |
| 12 | Header | Optional, column order is fixed |
| 13 | Exit codes | `0`, `3`, `4`, `64`, `66`, `73`, `74`, `101` (`78` removed 2026-11-09) |
| 14 | Output order | No sorting |
| 15 | Log | `tracing`, stderr or `LOG_FILE`, `RUST_LOG=error`, rejected rows at the `error` level; `RUST_LOG` is not validated (2026-11-09) |
| 16 | Order of row checks | Parsing → idempotency → account → type rules |
| 17 | Startup order | Arguments → configuration → log → input (`.env` removed 2026-11-09) |
| 18 | Registering `tx` | Only after the data checks pass |
| 19 | Dispute repeats | Category "business", a separate reason in the log |
| 20 | Quotes and encoding | Quote parsing disabled, outer quotes are stripped when parsing a field; a custom reader instead of `csv` |
| 21 | Line number | Physical line number, plus `client` and `tx` |
| 22 | Tests | Seven groups plus `cargo llvm-cov` coverage; `proptest` not yet implemented |
| 23 | Locating `.env` | Current directory only; revised 2026-11-09: `.env` is not supported |
| 24 | Summary line | `read = accepted + rejected_data + rejected_business` |
| 25 | Overflow of `total` | `deposit` checks both `available + amount` and the new `total`; `dispute`, `resolve`, `chargeback` check the new `available` and `held` |
| 26 | Number grammar | The library's; `amount` via `Decimal::from_str_exact` |
| 27 | Case of `type` | Converted to lower case |
| 28 | `-0` in the output | Amounts are normalised before output |
| 29 | Empty configuration parameter | Invalid, code `78`; revised 2026-11-09: no checks, `78` is not used |
| 30 | Empty rows and header | Empty means zero length or whitespace only; the header is in the first non-empty row; LF or CRLF |
| 31 | Summary line on a fatal error | Not written; only with codes `0`, `3`, `4` |
