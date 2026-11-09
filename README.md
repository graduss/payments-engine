# payments-engine

A payments engine in Rust. It reads a CSV of client transactions, applies them in order and prints the final state of every account as CSV.

```
cargo run -- transactions.csv > accounts.csv
```

## Approach

The task leaves a lot of behaviour open and asks for those gaps to be filled the way a real bank would fill them. When I started putting the specification together, more than thirty such questions came up: from which transactions can be disputed to what to do with an amount that has too many decimal places or with a repeated `tx`. I worked through each of them against how real payment systems handle it and recorded the decision.

So I decided to build the project the way I would build a production system, within the time available: specification and open questions first, then the architecture, then the code layer by layer with tests. The code follows clean architecture and DDD, to show how I design systems that are expected to grow: the domain rules live in domain types and do not depend on CSV, the log or the CLI.

Details are in `docs/`: the [specification](docs/specification.md), the [architecture](docs/architecture.md) and the [open questions](docs/open-questions.md) with the reasoning behind every decision. Below, OQ-N refers to an item in the open questions and AD-N to a decision in the architecture.

## Running

```
cargo build --release
./target/release/payments-engine transactions.csv > accounts.csv
```

The only argument is the path to the input file. stdout gets only the accounts CSV; the log goes to stderr or to a file.

Input:

```
type, client, tx, amount
deposit, 1, 1, 1.0
withdrawal, 1, 2, 0.5
dispute, 1, 1,
```

Output (row order is not defined):

```
client,available,held,total,locked
1,-0.5000,1.0000,0.5000,false
```

### Settings

Two environment variables, both optional:

| Variable | Default | Meaning |
|---|---|---|
| `LOG_FILE` | not set, log goes to stderr | Log file, appended to |
| `RUST_LOG` | `error` | Log filter in `tracing_subscriber::EnvFilter` syntax |

Values are not validated up front. If `LOG_FILE` cannot be opened, the program exits with code 73. Invalid directives in `RUST_LOG` are skipped with a warning.

The log has one line per event with `key=value` fields:

```
2026-11-09T13:59:56.913Z ERROR kind=rejected line=6 client=2 tx=5 category=business reason="insufficient funds"
2026-11-09T13:59:56.914Z  INFO kind=summary read=5 accepted=4 rejected_data=0 rejected_business=1
```

By default the log shows rejected rows and fatal errors. `RUST_LOG=info` adds the summary line, `RUST_LOG=debug` adds every accepted operation.

### Exit codes

| Code | When | stdout |
|---|---|---|
| 0 | All rows applied | result |
| 3 | Some operations were rejected by business rules (insufficient funds, locked account, invalid dispute…) | result |
| 4 | Some rows contain invalid data. Takes precedence over 3 | result |
| 64 | Invalid arguments | empty |
| 66 | The input file cannot be opened | empty |
| 73 | `LOG_FILE` cannot be opened | empty |
| 74 | I/O error while reading or writing | incomplete, must not be used |

Partial success has its own codes so that the caller can tell "everything went through" from "some rows were dropped" without parsing the log (OQ-13).

## Assumptions

The task leaves part of the behaviour open. Below is what I chose, in short. For every item, `docs/open-questions.md` explains how real payment systems handle it.

**Disputes**

- Only a `deposit` can be disputed. A dispute on a `withdrawal` works the other way round in practice and makes no sense in the task's model (OQ-4).
- The `client` in a dispute must match the owner of the transaction, otherwise the row is rejected (OQ-5).
- A `dispute` can push `available` below zero if the money has already been withdrawn: the full deposit amount always moves to `held` (OQ-6).
- A dispute can be reopened after a `resolve`. The number of cycles is not limited; the engine only counts them (OQ-7).
- `chargeback` is a final state.

**Accounts**

- An account is created only by an accepted `deposit`. Any other operation for a client without an account is rejected (OQ-10).
- After a `chargeback` the account is fully locked: `deposit` and `withdrawal` are rejected. Disputes on earlier deposits are still processed, because an outside party initiates them (OQ-3).

**Transactions**

- `tx` is unique across all `deposit` and `withdrawal` rows. A repeat is a data error (OQ-9).
- A `tx` is used up as soon as the row passes the duplicate check, even if a business rule rejects it afterwards. A row with a data error does not use up its `tx`, so a corrected row with the same `tx` will go through (OQ-18).

**Format**

- An amount with more than 4 decimal places is rejected, not rounded: a silently changed amount breaks reconciliation (OQ-2). Trailing zeros do not count: `1.50000` is `1.5`.
- A non-empty `amount` on `dispute`, `resolve` or `chargeback` is a data error. The system does not support partial disputes (OQ-11).
- The header is optional and columns are identified by position (OQ-12). `type` is compared case-insensitively, whitespace around fields is trimmed, and one pair of outer quotes is stripped.
- The output is not sorted (OQ-14).

## Error handling

There are few fatal errors: invalid arguments, an input file that cannot be opened, an I/O error. Anything wrong with a single row rejects only that row. The program logs the reason and carries on. Input data cannot cause a panic: all amounts are computed with `checked_*`, and an overflow rejects the operation.

Rejected rows fall into two categories:

- **data**: the row cannot be parsed or breaks the format: wrong number of fields, unknown `type`, bad amount, repeated `tx`;
- **business**: the row is valid but the operation is not possible: insufficient funds, no account, locked account, invalid dispute transition.

The checks run in a fixed order (spec, 10.3), and a row is rejected by the first check it fails. The log records the physical line number, plus `client` and `tx` when they could be parsed.

## How correctness was checked

```
cargo test                                 # 147 unit + 18 end-to-end
cargo test --release -- --ignored          # 1,000,000-row input
cargo clippy --all-targets -- -D warnings
cargo llvm-cov --summary-only              # coverage, about 99% of lines
```

- **Unit tests per layer.** Domain: amounts, account operations, every dispute transition, overflow. Engine: check order, `tx` usage, reopened disputes. Row parsing: amounts, quotes, header, BOM, CRLF, line numbers. Output: 4 decimal places, no `-0.0000`. Log: event format.
- **Reference data sets** in `tests/data/<scenario>/`: `input.csv`, `expected.csv` and `exit_code`. An end-to-end test runs the binary on every set and compares the output regardless of row order. There are 12 sets: the example from the task, disputes, chargeback and locking, negative balance, invalid disputes, invalid rows, format variants, an empty file and others. The expected results were worked out by hand.
- **End-to-end CLI tests**: exit codes, empty stdout on a fatal error, log only on stderr or in `LOG_FILE`, appending to the log file, a fatal error on stderr even with `RUST_LOG=off`.

To add a scenario, create a directory in `tests/data/` with the three files.

## Efficiency

- **The input is streamed**, one line at a time into a reused buffer. The whole file is never loaded into memory.
- **Only what disputes need is kept in memory**: the accounts, processed deposits (amount and dispute state) and the set of other used `tx` ids. Withdrawals are not stored, only their `tx`.
- **Money** is `rust_decimal::Decimal`, never `f64`. The engine only adds and subtracts, so precision does not grow and nothing needs rounding (OQ-8).

On my machine (release build), 5,000,000 deposits, a 143 MB CSV, are processed in 1.8 s with peak memory of about 450 MB. Almost all of that memory goes to deposits: each one has to be kept in case of a later dispute, roughly 90 bytes per deposit. This is the main limitation of the current version, see below.

**Why not the `csv` crate.** Quoting is turned off by the format (OQ-20), so all that `csv` would add is splitting on commas. At the same time its `position().line()` miscounts lines after CRLF endings and blank lines: for `a\r\nb\r\nc` it reports 1, 1, 2. The log needs the real line number in the file. So the input is read with `BufRead::read_until` and split with `split(',')` (AD-5).

## Architecture

Clean architecture in a single crate: four layers as modules, with dependencies pointing inwards.

```
interfaces (cli) ─▶ infrastructure (csv, log, config) ─▶ application (engine) ─▶ domain
```

- `domain`: the domain model in a DDD style. `Amount`, `ClientId` and `TxId` are value objects: an amount of zero or with five decimal places cannot be created. `Account` is an entity with the rules for changing the balance. `Transaction<Deposit>` owns the dispute lifecycle: `dispute`, `resolve` and `charge_back` are its methods, not engine code. The layer knows nothing about files or the log.
- `application`: `PaymentEngine` and `process`, the use case of processing a stream of rows. The engine does not log and does not read the environment: it reports rejections through closures.
- `infrastructure`: the CSV reader and writer, logging on `tracing`.
- `interfaces`: the CLI on `clap`, startup order and exit codes.

Ports are expressed as standard traits: the row source is any `Iterator`, rejections and accepted operations are `FnMut`. So the engine can be reused in a server that handles many streams, with one engine per stream or clients sharded by `client`. The engine is `Send` and has no global state.

## Limitations and next steps

- **Memory for deposits.** `tx` spans the whole `u32` range, so in the worst case billions of deposits would have to be kept. The next step is to put deposit storage behind a trait (`DepositStore`) and add an on-disk implementation. The set of used `tx` ids could be replaced by a bitmap (`roaring`).
- **Invariant tests with `proptest`** are planned but not done: `total = available + held` after every operation, conservation of money, no panic on random rows.
- **Amounts are limited to the `Decimal` range** (about 7.9·10²⁸). An operation that would take a balance beyond it is rejected.
