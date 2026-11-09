# Open questions on the specification

Questions the problem statement does not answer, and how such cases are usually handled in real payment systems. The final decisions need to be carried over to the "Assumptions" section of the README.

---

## 1. Invalid rows in the input file

> The task defines the input CSV format, but the file still comes from an external source, so it may contain invalid rows, for example text where a number is expected, or a deposit with no amount. How should I handle such errors: stop processing and print an error to stderr, or skip the bad row and continue?

### How real systems do it

Batch file processing (ACH/NACHA, ISO 20022 `pain.001`, card scheme clearing files) is almost always built on **separating errors by level**:

| Level | Example | Reaction |
|---|---|---|
| Whole file | Unreadable file, broken header, control totals (trailer) do not match | Reject the whole file: no row can be trusted |
| Batch | Error in the batch header | Reject the batch, process the rest |
| Single record | Wrong amount, unknown account, insufficient funds | Reject only this record, process the rest |

General rules:

- **Nothing is dropped silently.** Every rejected record is recorded with a reason code: in ACH these are return codes (R01 "insufficient funds", etc.), in ISO 20022 it is the `pain.002` status report with `ACCP` / `RJCT` / `PART` statuses per transaction.
- Rejected records go to an **exception queue** (exception queue, dead-letter queue or suspense account), where an operator investigates them or they are returned to the sender.
- One bad record **does not stop** processing of thousands of correct ones: otherwise one client's typo blocks everyone else's payments.

### Decision for the task

**Status: agreed (2026-11-07).**

- **Fatal errors** (the file cannot be opened, the CSV header cannot be read, an I/O error in the middle of the stream): a message to stderr, non-zero exit code.
- **Errors in individual rows** (type, client or tx do not parse; deposit/withdrawal has no amount; the amount is negative or zero; duplicate tx id): **skip the row**, print a warning to stderr with the row number and the reason, continue processing.
- stdout contains only the accounts CSV, so warnings do not break the automated check.

A side effect to keep in mind: if a deposit is skipped, a later dispute on that tx refers to an unknown transaction and is also ignored. This is consistent with the rules of the spec.

---

## 2. Amount with more than 4 decimal places

> Amounts are said to have a precision of up to four decimal places. If a value comes with more decimal places, should the extra digits be truncated, or should the value be rounded?

### How real systems do it

- Amounts are stored in **minor units** (cents, satoshis) as integers or in fixed-point. Precision is set by the currency exponent per ISO 4217 (USD has 2 places, JPY 0, BHD 3).
- **An incoming amount with excess precision is rejected, not corrected.** In card protocols (ISO 8583) the amount field holds an integer number of minor units, so a fractional part physically cannot be transmitted. In ISO 20022 the number of decimal places is limited by the schema, and an amount that does not match the currency exponent fails validation.
- The reason is that silently changing an amount sent by a client or counterparty **creates or destroys money**: the balance stops matching the external side, and reconciliation breaks. Dropping `0.00005` over millions of operations produces a real discrepancy.
- **Rounding is applied only to computed results**: currency conversions, interest, fees. The mode is usually fixed by a regulator or a contract: for conversion to euro, EU Regulation 1103/97 requires half-up, and many accounting systems use banker's rounding (half-even). The rounding remainder is explicitly posted to a separate account.

### Decision for the task

**Status: agreed (2026-11-07).**

- An amount with significant digits beyond the 4th place (`1.00005`) is treated as an **invalid row**: the row is skipped, a warning is printed to stderr (see item 1).
- Trailing zeros are allowed: `1.50000` is exactly equal to `1.5000`, no information is lost.
- Neither truncation nor rounding is applied silently. The README should mention the alternative (half-even rounding) and explain why rejection was chosen.

---

## 3. Operations on a locked account after a chargeback

> After a chargeback the account is frozen. If the file keeps sending transactions for that client, should they be ignored, or still applied, with the chargeback only setting the locked flag?

### How real systems do it

The "locked" status in banks is not uniform; several modes are usually distinguished:

| Mode | Incoming (credit) | Outgoing (debit) | Typical reason |
|---|---|---|---|
| Debit freeze | allowed | forbidden | court-ordered seizure, debt collection |
| Full freeze / block | forbidden | forbidden | suspected fraud, AML review |
| Closed | forbidden (returned to sender) | forbidden | account closure |

Key points:

- A chargeback on a deposit usually means the sender's card or account was compromised. So after a chargeback, exchanges and payment services as a rule **fully freeze the account** until a manual review (risk/compliance review). This is closest to the `locked` flag in the task.
- **Withdrawals are always blocked**: this is the main channel through which a fraudster takes money out.
- **Deposits** to a locked account are in reality not "ignored" but rejected and returned to the sender. If they keep being accepted, the amount that may later turn out to be disputed grows.
- **Dispute, resolve and chargeback are initiated by an external party** (the issuing bank or card scheme) against transactions that have already been processed. They cannot be refused because of the status of one's own account: the scheme has already taken the money. The account lock does not affect them.

### Decision for the task

**Status: agreed (2026-11-07), full freeze chosen.**

- On a locked account **deposit and withdrawal are rejected** (the row is ignored, a warning is printed to stderr). In the task's model, "rejecting a deposit" means returning the funds to the sender.
- **Dispute, resolve and chargeback continue to be processed** for this client's previously processed transactions.
- There is no unlocking: the input format has no corresponding operation, and in reality it is an operator's decision after a review.
- The README should mention the alternative (debit freeze, where deposits are accepted) and explain the choice.

---

## 4. Which transactions can be disputed

The spec describes a dispute as moving the amount from available to held, but does not say whether this also applies to a withdrawal.

### How real systems do it

- **A card chargeback concerns incoming money.** The cardholder disputes a payment with their issuing bank, and the money is taken from the recipient, that is, from the platform that accepted this payment as a deposit. The spec's mechanics (freeze the amount, then return it or debit it) describe exactly this case.
- **Disputing an outgoing operation works differently.** If a client claims they did not make a withdrawal, it is an internal fraud investigation. In the US it is governed by Reg E (error resolution for unauthorized electronic transfers). The bank does not freeze the client's money; on the contrary, it temporarily credits the disputed amount to the client (provisional credit) for the duration of the investigation. The direction of money movement is the opposite of the task's dispute.
- Applying the spec's mechanics to a withdrawal produces nonsense: held increases by an amount that has already left the account, and total does not change even though the money is no longer in the account.

### Decision for the task

**Status: agreed (2026-11-07).**

- **Only a deposit** can be disputed. A dispute pointing to a withdrawal is ignored with a warning to stderr.
- So for each transaction only deposit data is kept in memory: amount, client and dispute state. Withdrawals do not need to be stored, which saves memory on large input files.

---

## 5. The client in a dispute does not match the transaction owner

### How real systems do it

- A dispute refers to the original transaction by its identifier (in card schemes this is the ARN, acquirer reference number). The account being acted on is determined **by the original transaction**, not by what is stated in the dispute message.
- A mismatch between the stated client and the transaction owner is treated as a data integrity error: the message is sent to an exception queue for manual investigation. Nobody automatically debits money from either of the two clients.
- Applying the dispute to someone else's account would be the worst outcome: a single message with a typo, or a forged message, could freeze or take the money of an unrelated client.

### Decision for the task

**Status: agreed (2026-11-07).**

- If the client in a dispute, resolve or chargeback does not match the owner of the tx, **the row is ignored** and a warning is written to stderr. Neither of the two accounts changes.
- No account is created for the client in such a row: the row is rejected before the account is accessed.

---

## 6. Can available go negative

A withdrawal cannot: the spec explicitly requires rejecting it when funds are insufficient. What remains is the case where a client made a deposit, withdrew part of the money, and then that deposit was disputed.

### How real systems do it

- A chargeback **does not depend on the balance** of the recipient's account. The card scheme takes the money from the acquirer in any case, and if the client has already withdrawn it, the client ends up with a negative balance, that is, a debt to the platform.
- This is exactly why acquirers and payment services keep **reserves** (rolling reserve): part of the merchant's turnover is held back against future chargebacks. The negative balance is then recovered (collections) or written off as a fraud loss.
- A negative balance automatically blocks further withdrawals until the debt is repaid.

### Decision for the task

**Status: agreed (2026-11-07).**

- **A dispute can take available negative.** The full deposit amount is moved to held, so total does not change, and `total = available + held` always holds.
- Alternative: freeze only `min(available, amount)`. It is worse, because resolve and chargeback would then have to remember how much was actually frozen, and the resulting figures diverge from the amount of the original transaction.
- A withdrawal still requires `available >= amount`, so an account with negative available cannot withdraw anything.

---

## 7. Repeated dispute on the same transaction

### How real systems do it

- Each disputed transaction has a **lifecycle**, and only one dispute can be open on it at a time. Card schemes reject a repeated dispute message on a transaction that is already disputed as a duplicate.
- After a chargeback the transaction is closed for good: the money has already been returned to the cardholder and cannot be taken a second time.
- If the merchant won the dispute (representment), the cardholder can continue it as pre-arbitration and then arbitration. Formally this is a continuation of the same dispute, not a new dispute, but at these stages money can again be frozen and debited.

### Decision for the task

**Status: agreed (2026-11-07).**

A deposit transaction goes through these states:

```
Processed --dispute--> Disputed --resolve----> Processed
                          |
                          +------chargeback--> ChargedBack (final)
```

- A dispute on a transaction in the `Disputed` state is ignored (it is a duplicate).
- A dispute on a transaction in the `ChargedBack` state is ignored (the transaction is closed).
- **After a resolve the transaction can be disputed again, that is, the dispute is reopened.** A repeated dispute works the same way as the first: the deposit amount is again moved from available to held, total does not change, the transaction returns to the `Disputed` state, and resolve or chargeback are again allowed on it. The number of such cycles is not limited. The task's model has no separate pre-arbitration and arbitration stages, and a repeated dispute after a resolve is the closest thing to them. Ignoring it could lose a legitimate chargeback that arrives later.
- Alternative: allow only one dispute over the whole lifetime of a transaction. It is simpler but reflects the real dispute cycle less well. The README should describe the choice and the alternative.

### Limit on the number of disputes per transaction

**Status: decision revised 2026-11-08: no limit is introduced, only a `dispute_count` counter.**

Earlier (2026-11-07) a configurable limit on disputes per transaction was agreed. It was dropped: there is no configuration parameter, no separate rejection reason and no limit in the engine.

- **A repeated dispute after a resolve remains allowed**, the number of cycles is not limited. The rationale is the same as above: a repeated dispute is the closest thing to pre-arbitration and arbitration, and ignoring it may cost a legitimate chargeback.
- **Counter.** Each deposit stores `dispute_count: u32`: the number of disputes opened, including the first one. It grows via `saturating_add` and is informational only; it does not affect processing.
- **Memory.** For each deposit transaction this counter (`u32`) is stored in addition.

---

## 8. Representation of monetary amounts

**Status: agreed (2026-11-07), the `rust_decimal` crate is used.**

### Why not `f32` / `f64`

The primitive floating-point types are unsuitable for money because of the properties of the IEEE 754 standard they are built on.

- **Binary base.** A number is stored as a binary mantissa and exponent. Most decimal fractions (`0.1`, `0.2`, `0.0001`) have no finite binary representation, just as `1/3` has no finite decimal notation. So memory holds the nearest representable value, not the number itself: `0.1_f64` is actually equal to `0.1000000000000000055511151231257827…`.
- **Error accumulation.** Each arithmetic operation rounds the result to the nearest representable value, and the errors add up. The classic example: `0.1 + 0.2 == 0.30000000000000004`, not `0.3`. A million deposits of `0.1` in `f64` will not add up to exactly `100000.0`. For a balance that millions of operations pass through, this is a reconciliation discrepancy that cannot be explained.
- **Unreliable comparison.** The `available >= amount` check for a withdrawal can give the wrong answer at the boundary if one of the values has accumulated error. As a result, a client is refused a withdrawal when there is enough money, or allowed one when there is not.
- **Limited precision.** The `f64` mantissa has 53 bits, that is, about 15–17 significant decimal digits. On large amounts the low-order digits (our 4 decimal places) start to be lost entirely.
- **Implicit rounding on output.** Formatting with `{:.4}` hides the error but does not remove it: with different computation paths the same amount may round in different directions.

### Why `rust_decimal`

- A number is stored as a **96-bit integer plus a decimal scale** (0–28 decimal places). The base is decimal, so any amount with 4 decimal places is represented exactly, and addition and subtraction are performed without error.
- The range is about ±7.9·10²⁸, which covers any real balances many times over.
- There is `serde` support (via a feature), so amounts are parsed directly from the CSV.
- There is overflow-checked arithmetic (`checked_add`, `checked_sub`). The ordinary `+` and `-` operators panic on overflow, so the engine uses `checked_*`, and an overflow is treated as a transaction error, not a reason to crash.

### Alternative considered

A custom fixed-point type: `i64` in units of `0.0001`. It is more compact (8 bytes versus 16) and faster, but requires its own parsing, formatting and edge-case tests. For this task clear and reliable code matters more, so a proven crate was chosen. The alternative should be mentioned in the README.

### How it is used in the solution

- **Precision is controlled only at input.** A row with a too-precise amount is rejected and written to the log, as decided in item 2. So only amounts with at most 4 decimal places reach the engine.
- **The engine does not increase precision.** It only adds and subtracts amounts: on deposit, withdrawal, dispute, resolve and chargeback. There is no multiplication, division or conversion. The sum or difference of numbers with at most 4 decimal places also has at most 4 decimal places. So rounding inside the engine is never needed, and there is none.
- **Output:** amounts are printed with exactly 4 decimal places. This is formatting only (zero padding), not rounding.

---

## 9. Uniqueness of `tx` for withdrawals (Q-1 in the spec)

**Status: agreed (2026-11-07), option B.**

The spec guarantees that `tx` is globally unique. The question is whether to trust this guarantee or check it, and what the check costs for withdrawals, whose data we no longer need.

### How real systems do it

- **Idempotency is mandatory for all operations.** Every operation has an idempotency key: a payment identifier, `EndToEndId` in ISO 20022, `Idempotency-Key` in payment service APIs. A repeated message with the same key is not executed a second time. The sender gets back the result of the first execution or a rejection as a duplicate.
- **Counterparty guarantees are not trusted, they are checked.** Duplicates occur constantly in real flows: a resend after a timeout, a repeated upload of the same file, a bug in the sender's system. So the check is done regardless of what the format promises.
- **A duplicate withdrawal is more dangerous than a duplicate deposit.** A repeated deposit creates money out of nothing (a loss for the platform). A repeated withdrawal debits the client's money twice (direct harm to the client and a claim against the platform). Both need protection.
- **The key is consumed together with the result.** If an operation is rejected, for example because of insufficient funds, its key is still considered used, and the "rejected" result is stored. A retry with the same key gets the same rejection, not a new attempt. A new attempt goes with a new key.

### Options for the task

| Option | Memory | Pros | Cons |
|---|---|---|---|
| A. Do not check withdrawals, trust the spec | 0 | Simplest | A duplicate withdrawal debits money twice. A withdrawal with the `tx` of an existing deposit goes unnoticed |
| B. `HashSet<u32>` with the `tx` of all withdrawals plus the already existing `HashMap` of deposits | ≈ 5–11 bytes per withdrawal | Exact check. Memory grows with input volume. A deposit record takes several times more, so the cost is moderate | In the worst case, with billions of withdrawals, gigabytes |
| C. A bitmap over the whole `u32` range | 512 MiB fixed | Constant memory in the worst case | 512 MiB is allocated even for a three-row file |
| D. A compressed bitmap (`roaring` crate) | from a few bytes to ≈ 512 MiB | Compact at any volume | An extra dependency and complexity for a load the task most likely does not have |

### Proposal

- **Option B.** Uniqueness is checked across all `deposit` and `withdrawal` rows. The `HashMap` of deposits, which is already needed for disputes, is used, plus a separate `HashSet<u32>` for withdrawal `tx` values. A `tx` found in either of the two structures is a duplicate. Deposit data is not duplicated in the set.
- **A `tx` is consumed whatever the business-rule outcome.** The `tx` of every `deposit` or `withdrawal` row that passed parsing is registered, even if the operation was rejected (insufficient funds, locked account). This matches real idempotency semantics. A rejected deposit goes into the set of used `tx` values, not into the `HashMap` of deposits: there is nothing to dispute. So the set is more accurately named "used `tx` values other than processed deposits". Rows of category "data" do not register their `tx`: their content cannot be trusted (clarified in OQ-18).
- **The storage is hidden behind a small internal engine interface.** If needed, the `HashSet` can be replaced with `roaring` without changing the logic. The README should note this as a possible optimisation.

---

## 10. Account creation on a rejected row (Q-2 in the spec)

**Status: agreed (2026-11-07), option B, an account is created only by an accepted deposit.**

The spec says plainly: an account is created the first time a client is referenced. It is unclear whether a row the engine rejected counts as a reference. Examples: a failed withdrawal by a new client, a dispute by a new client on an unknown `tx`, a deposit with a repeated `tx`.

### How real systems do it

- **An account is never created by a transaction.** It is opened by a separate onboarding process with client verification (KYC). A transaction to a non-existent account is rejected: in ACH this is return code R03 "No account / unable to locate account".
- **A rejected operation does not change state.** It leaves an entry in the log (audit log) but does not create entities in the accounting system. An empty account that appeared because of a rejected attempt would be a "phantom" and would get in the way of reporting.
- Automatic account creation in the task is a simplification that models "the client appeared in the system at first contact". It has no real counterpart, so the wording of the spec has to be relied on.

### Options

| Option | Rule | Pros | Cons |
|---|---|---|---|
| A. On parse | An account is created for every parsed row, before business rules are checked | Follows the spec's wording literally ("referenced"). One simple rule with no exceptions | The output contains zero accounts for clients whose operations were all rejected |
| B. On success | An account is created only if the operation was accepted and changed state | Closer to real practice: a rejection materialises nothing | Diverges from the literal wording of the spec. The automated check may expect a row of zeros for such a client |

### Decision

- **Option B.** The spec says nothing about invalid rows. So by "referencing a client" it means a valid operation, and a rejected row does not count as a reference.
- **An account is created only by an accepted `deposit`.** If a client's first operation has any other type (`withdrawal`, `dispute`, `resolve`, `chargeback`), the row is ignored and a warning "account does not exist" is printed to stderr.
- A rejected `deposit` by a new client (repeated `tx`, invalid amount) does not create an account either.
- This matches real practice: an operation on a non-existent account is rejected (R03 in ACH), and the first deposit plays the role of opening the account.
- The decision is consistent with the other rules without exceptions:
  - a `withdrawal` by a new client would have been rejected for insufficient funds anyway, and is now rejected earlier and with a more precise reason;
  - `dispute`, `resolve` and `chargeback` require the client to own the deposit, which means their account already exists;
  - the decision in OQ-5 (no account is created on a client mismatch) stays in force unchanged.
- Zero accounts "out of nowhere" never appear in the output. An account with a zero balance appears only if the client deposited money and then withdrew all of it or lost it to a chargeback.

---

## 11. `amount` on dispute, resolve and chargeback (Q-3 in the spec)

**Status: agreed (2026-11-07) after revision. A row with an `amount` is rejected, category "data", exit code `4`.** The originally agreed option A is cancelled, see "Revision" at the end of the item.

The spec says that these rows have an empty or missing `amount`. What should be done if it is filled in anyway?

### How real systems do it

- In card schemes a dispute message **contains an amount**, and it may be smaller than the original: a partial chargeback, for example when one item of an order is disputed. So an amount in a dispute is a normal part of the real protocol.
- The task's model has no partial disputes: a dispute always concerns the full deposit amount.
- A message that does not conform to the format goes to an exception queue in reality. But a chargeback cannot be refused: the scheme will take the money anyway. So rejecting a dispute because of an extra field leads to a divergence from reality.

### Options

| Option | Pros | Cons |
|---|---|---|
| A. Ignore the value and process the row, writing a warning to stderr | A legitimate dispute is not lost because of an extra field. The discrepancy is visible in the log | Not strict: the row formally does not conform to the format |
| B. Reject the row as invalid | Strict validation, as in OQ-2 | Loss of a legitimate dispute. For a chargeback this diverges from the real movement of money |
| C. Partial dispute for the stated amount | Closer to real schemes | Goes beyond the scope of the task, complicates resolve and chargeback, contradicts the spec |

### Decision

- **Option A.** The dispute applies to the full amount of the original deposit. The `amount` value in such a row is not used, and a warning is written to stderr.
- The value is not parsed or checked: even if it contains garbage, the row is processed by `type`, `client` and `tx`.

### Revision

The system does not support partial refunds or partially won disputes. If a `dispute`, `resolve` or `chargeback` carries an amount, it is a sign that the data was produced for a system with a different dispute model, that is, the data is incompatible with our system. So such a row belongs to category "data" and yields exit code `4`.

Two options for handling the row itself were considered:

| Option | Pros | Cons |
|---|---|---|
| Apply for the full amount, code `4` | A legitimate dispute is not lost | If the sender meant a partial dispute (for example, 30 out of 100), more than necessary will be frozen or debited |
| Reject the row, code `4` | An operation whose meaning we do not understand is not applied. Consistent with rejecting too-precise amounts (OQ-2) | If the dispute is legitimate, it will not be reflected, and the balance will diverge from reality until a manual investigation |

**Decision: the row is rejected**, state does not change, a warning is printed to stderr, exit code `4`. If the data is incompatible with the system's model, applying the operation for the full amount may be worse than not applying it at all. The discrepancy is visible from the exit code and in the log, and is investigated manually.

Details:

- "An amount is present" means the `amount` field exists and is non-empty after trimming whitespace. An empty field and a 3-field row are allowed.
- The value is not parsed: any non-empty content is rejected, whether a number or garbage.
- The `tx` of such a row is not registered, and the transaction state does not change.
- **Where it is checked (2026-11-08).** The check is done by the parser (the infrastructure layer, CSV reading), not by the domain. Whether a dispute carries an amount is a property of the row format: the parser rejects such a row as a parse error, and it never reaches the engine. The domain methods `dispute`, `resolve` and `charge_back` do not take an amount.

---

## 12. Input file header (Q-4 in the spec)

**Status: agreed (2026-11-07), including the details below. The header is optional, the column order is fixed.**

The spec shows the input file with the header `type, client, tx, amount`, but does not say whether it is mandatory or whether the column order may change.

### How real systems do it

- **In file formats a control record at the start of the file is mandatory.** In NACHA a file starts with a File Header Record (type 1), each batch with a Batch Header (type 5), and the file ends with a File Control Record (type 9) with control totals. In ISO 20022 a message starts with a Group Header, which states the number of transactions (`NbOfTxs`) and a control sum (`CtrlSum`).
- **A file without a correct header is rejected entirely.** Without a header there is no way to confirm that the file is of the expected format and version, so no row can be trusted.
- **Fields are determined by the schema, not by guessing.** Detecting automatically whether a header is present (for example, checking whether the first row looks like data) is considered bad practice: on unexpected data it breaks unpredictably.

### Options

| Option | Pros | Cons |
|---|---|---|
| A. Header mandatory, columns matched by name | Columns may be in any order. A missing column is detected immediately. This is the standard behaviour of `csv` + `serde` | A file without a header is not processed |
| B. Header mandatory, column order fixed | The strictest format check | Breaks on a harmless reordering of columns |
| C. Header optional, presence determined from the first row | Accepts more files | Fragile heuristic, contradicts practice |

### Decision

An option not in the table above was chosen: **the header is optional, the column order is fixed** (`type, client, tx, amount`).

**Why the decision diverges from real practice.** In real systems the file format is strictly standardised (NACHA, ISO 20022), and a file without a correct header is rejected entirely. Here, however, the input file is supplied by an automated check, and it is not known in advance whether its files will have a header and how exactly it will match the example. If the file is rejected because of the header, the check fails entirely. So the behaviour chosen accepts the file in both cases. The README should state that in a real system this would be handled by strict header validation against the format standard.

**Control totals.** Besides the header, the standards require checking the file's control totals. They protect against truncated, corrupted and substituted files:

- in NACHA the batch (Batch Control) and file (File Control) trailer records contain the record count, an entry hash (the sum of the receiving bank numbers), and the total debit and credit amounts;
- in ISO 20022 the number of transactions (`NbOfTxs`) and the sum of all transactions (`CtrlSum`) are stated in the Group Header and in each Payment Information block.

The processor recomputes these values from the actual records, and on any mismatch the file is rejected entirely before any operations are posted. The task's input format has neither a trailer record nor control totals, so such a check is impossible. This should also be noted in the README as a difference from a real system.

Details:

- **Columns are positional:** 1st is `type`, 2nd is `client`, 3rd is `tx`, 4th is `amount`. The column names from the header are not used.
- **Header recognition.** The first row is treated as a header and skipped if its fields, after trimming whitespace and ignoring case, match `type`, `client`, `tx` and, if a 4th field is present, `amount`. This is an exact comparison, not a "does it look like data" heuristic. A false positive is impossible because `type` is not a valid transaction type.
- If the first row is not recognised as a header, it is processed as an ordinary data row. If it does not parse (for example, a header with different column names), the row is rejected with a warning, and processing continues (OQ-1).
- **Number of fields in a row:**
  - a 3-field row is allowed for `dispute`, `resolve` and `chargeback` (`amount` is missing);
  - for `deposit` and `withdrawal` a 3-field row means the amount is missing, and the row is rejected (OQ-1);
  - a row with fewer than 3 or more than 4 fields is rejected as unparsed.

---

## 13. Exit code when rows are rejected (Q-5 in the spec)

**Status: agreed (2026-11-07), option B and the code table below. Revised 2026-11-09, see the note below.**

If the file is processed to the end but some rows are rejected, should the program report this through the exit code?

### How real systems do it

- **The result of batch processing usually has three outcomes: success, partial success and failure.** In ISO 20022 the `pain.002` status report gives a group of transactions the status `ACCP` (all accepted), `PART` (partially accepted) or `RJCT` (all rejected).
- **For batch jobs there is an established convention for return codes**, which came from mainframes (JCL) and is used by schedulers (Control-M and similar): `0` is success, `4` is a warning (the job completed, but there are remarks), `8` is an error, `12` and above is a serious error. The scheduler uses this code to decide whether to run the next jobs in the chain and whether to raise an alarm.
- Details of exactly which records were rejected are passed not through the return code but through a report or log.

### Options

| Option | Pros | Cons |
|---|---|---|
| A. `0` if there were no fatal errors. A summary (how many rows were processed and rejected) is printed to stderr | Does not interfere with the automated check. A caller that needs details takes them from stderr | The exit code does not distinguish "all clean" from "some rows rejected" |
| B. A separate non-zero code for partial success (for example, `2`), with the result in stdout printed in full | Follows the return code convention. The scheduler sees the problem without parsing logs | The automated check may treat any non-zero code as a failure and discard the output |

### Decision

- **Option B.** Partial success is reported with a separate non-zero code, while the result in stdout is printed in full. The exit code honestly reflects the outcome, as is customary for batch jobs.
- The risk that the automated check treats a non-zero code as a failure is knowingly accepted. The README must describe the codes explicitly, so the reviewer understands that a non-zero code on partial success does not mean a failure.
- At the end of processing a summary line is printed to stderr: how many rows were processed, how many were rejected and in which categories.

### Exit code table

Fatal errors use codes from `sysexits.h`. This is a standard Unix convention understood by system utilities and schedulers. There is no standard code for partial success, so small values were chosen. The values `1` and `2` are not used: `1` is a generic error code produced by many tools (and by Rust when `main` returns `Err`), and `2` by convention means command-line misuse in shell utilities. This way the program's own codes cannot be confused with others'.

| Code | Name | When | stdout |
|---|---|---|---|
| `0` | Success | All rows accepted and applied | Full result |
| `3` | Partial success: business-rule rejections | All rows parsed, but some operations were rejected by business rules: insufficient funds, locked account, account does not exist, unknown `tx`, client mismatch, invalid transition, overflow | Full result |
| `4` | Partial success: invalid data | At least one row contains invalid data: wrong number of fields, unknown `type`, non-numeric `client` or `tx`, missing or invalid `amount`, too-precise amount, repeated `tx`. Business-rule rejections may also be present | Full result |
| `64` | `EX_USAGE` | Wrong number of command-line arguments | Empty |
| `66` | `EX_NOINPUT` | The input file does not exist or cannot be opened | Empty |
| `74` | `EX_IOERR` | I/O error while reading input or writing the result | Empty or incomplete output (see below) |
| ~~`78`~~ | ~~`EX_CONFIG`~~ | ~~Invalid value of a configuration parameter (`LOG_FILE`, `RUST_LOG`)~~ — removed 2026-11-09, see OQ-29 | Empty |
| `101` | Rust panic | Should not happen. This is the code Rust produces on a panic by default | Empty |

Notes:

- **Why two partial-success codes.** A business-rule rejection is a normal outcome of an operation: the client tried to withdraw more than they have. It does not require intervention, like return code R01 "insufficient funds" in ACH. An invalid row means a problem in the sender's data, and it needs to be investigated. Different codes let the scheduler react to these situations differently.
- **A repeated `tx` counts as invalid data.** The spec guarantees `tx` uniqueness, so a duplicate means an error in the sender's data (a resend or a fault in their system), not a normal outcome of an operation. It is detected in the engine, because the check needs state, but by category it is a data error.
- **Precedence.** If the file has both invalid rows and business-rule rejections, `4` is returned, as the more serious one.
- **Error writing the result.** An error writing to stdout (for example, a closed pipe) yields `74`. The output may be incomplete in this case, and the caller must not use it.
- `cargo run` passes the program's exit code through unchanged, so the codes are visible when running through cargo as well.

**Revised (2026-11-09):** code `78` is not used, because the configuration is no longer validated (OQ-29). Arguments are parsed by `clap`: any parse error yields `64`, and `--help` and `--version` print help to stdout with code `0`.

---

## 14. Row order in the output (Q-6 in the spec)

**Status: agreed (2026-11-07), option A: output without sorting.**

The spec says that the order of rows in the output does not matter. The question is whether to leave it arbitrary.

### How real systems do it

- **Reports and statements are produced deterministically**, usually in account number order. Reconciliation files are compared line by line with the other side's data, and an arbitrary order makes such a comparison impossible without sorting first.
- **Reproducibility is needed for audit.** Rerunning on the same data must produce a byte-identical result. Otherwise there is no quick way to prove that nothing changed, and investigating discrepancies becomes harder.

### Rust specifics

`HashMap` uses a random hashing seed (`RandomState`) by default to protect against HashDoS attacks. So the iteration order changes **from run to run** even on the same data. If accounts are printed in `HashMap` iteration order, the same input produces different output.

### Options

| Option | Pros | Cons |
|---|---|---|
| A. Arbitrary order (`HashMap` iteration) | Nothing needs to be done | Output changes from run to run. Tests must compare the result ignoring order |
| B. Sort by `client` ascending | Deterministic output. Tests compare against the expected file directly. Convenient for a human | A small sorting cost |

### Decision

- **Option A: rows are printed in the iteration order of the account store, without sorting.** The spec explicitly says that output order does not matter, so no extra work is needed.
- Consequence for tests: the result is compared with the expected one **ignoring row order**, for example by comparing sets of rows or sorting both sides inside the test.
- The README should note that the output may differ in row order from run to run, and that in a real system the output would be made deterministic for reconciliation and audit.

---

## 15. Logging

**Status: agreed (2026-11-07), including the details below. The default level is `error`. Revised 2026-11-09, see the note below.**

### Crates

- `tracing` — the facade for log events (`warn!`, `info!`, etc.).
- `tracing-subscriber` with the `env-filter` feature — formatting, output and level filtering.

### Configuration parameters

Read only from the process environment variables (OQ-23, revised 2026-11-09).

| Parameter | Default | Meaning |
|---|---|---|
| `LOG_FILE` | not set, that is, output to stderr | Path to the log file. If set, the log is written to the file rather than to stderr |
| `RUST_LOG` | `error` | Log level and filter in the standard `EnvFilter` syntax (`warn`, `info`, `debug`, `<crate_name>=debug`, etc.) |

The standard name `RUST_LOG` was chosen for the level rather than a custom one: every Rust developer knows it, and `EnvFilter` understands it without extra code.

**The default level is `error`.** Rejected rows are written at the `error` level, so without configuration the log gets both fatal errors and all rejected rows. The summary line is visible with `RUST_LOG=info`. An invalid filter in `RUST_LOG` is treated as a fatal configuration error (`78`), like any other invalid configuration value: the program must not run with settings other than intended.

**Revised (2026-11-09):** `RUST_LOG` is not validated. The lenient `EnvFilter::new` skips invalid directives with a warning to stderr; an empty value equals `error` (OQ-29).

### Event levels

| Level | Events |
|---|---|
| `error` | Fatal errors (spec section 10.1) and rejected rows (10.2) with category and reason. Distinguished by the field `kind=fatal` or `kind=rejected` |
| `warn` | Not used |
| `info` | Summary line: how many rows were processed and how many rejected, by category |
| `debug` | Accepted operations (for debugging, off by default) |

### Format

- Text, one event per line, with structured fields. Example: `ERROR kind=rejected line=5 client=1 tx=7 category=business reason="insufficient funds"`.
- No ANSI colours when writing to a file.
- During implementation (2026-11-09): each line starts with the time (RFC 3339, UTC), the module name is not printed, ANSI colours are disabled in stderr as well. Accepted operations are written as `DEBUG kind=accepted line=.. client=.. tx=.. type=..`, the summary line as `INFO kind=summary ...`, a fatal error as `ERROR kind=fatal error="..."`.
- The fields allow filtering the log (`grep category=data`, `grep kind=rejected`) without parsing free text.

Earlier items (OQ-1–OQ-14) use the phrase "warning to stderr". It dates from before this decision. The rule in force: rejected rows are written to the log at the `error` level, and the log goes to stderr or to the file from `LOG_FILE`.

### Behaviour

- **stdout is never used for the log.** This is a requirement of the CLI contract.
- **Fatal errors are always duplicated to stderr**, even if the log is written to a file. Otherwise, when run from a terminal, the program would exit with a non-zero code with no visible reason.
- **Errors before log initialisation** (for example, invalid configuration parameters, including `LOG_FILE` itself) are printed directly to stderr.
- **The log file is opened for appending** and is not overwritten. The log is an audit trail, and the results of past runs must not be erased.
- **The log file cannot be opened** (no directory, no permissions): fatal error `73` (`EX_CANTCREAT` from `sysexits.h`), processing does not start.
- **Writing is synchronous**, without a background thread (`tracing-appender::non_blocking` is not used). The program exits via `std::process::exit` with the code from OQ-13, and that does not run destructors. With asynchronous writing the tail of the log could be lost.
- There is no log rotation: the program runs once per file; in a real system rotation is done by an external tool (`logrotate`).

### The engine does not write to the log

- The engine returns a typed result for each operation: accepted, or rejected with a category and reason. The CLI layer writes events to the log.
- This keeps the engine pure: its tests check the returned rejection reasons rather than capturing the log. A server that reuses the engine decides for itself how and where to report rejections (log, response to the client, metrics).

---

## 16. Order of row checks (A in the spec review)

**Status: agreed (2026-11-07).**

If several rejection reasons apply to a row at once, the order of checks determines the reason in the log and the category, and therefore the exit code (OQ-13). Example: is a withdrawal by a new client with a repeated `tx` a "repeated `tx`" (data, `4`) or "account does not exist" (business, `3`)?

### How real systems do it

- **Validation proceeds in stages, from cheap and general checks to expensive and specific ones:**
  1. technical (syntactic) — the message conforms to the format schema;
  2. idempotency check — whether this is a repeat of an already processed message;
  3. account check — whether it exists and in what status (open, frozen, closed);
  4. business rules — balance, limits, anti-fraud.
- **The idempotency check comes before business rules.** A repeat of an already processed message must not be evaluated anew: otherwise it may get a different result than the original (for example, the first time there was not enough money, but by the time of the repeat there is).
- **Fail-fast: the rejection is recorded at the first failed check**, and one reason is returned to the sender. In ACH a return has one return code. In `pain.002` several reasons can formally be passed, but in practice usually the first one is passed.
- **Checks that disclose other parties' data come after checks of one's own account.** First it is checked that the sender has a right to the operation at all, and only then the details of someone else's transaction.

### Proposal

Checks run strictly in the order given. A row is rejected at the first failed check, and only that check's reason is written to the log. Thanks to this order, the more serious category ("data") always wins over the less serious one ("business").

**Stage 1. Row parsing (category "data"):**

1. The number of fields is 3 or 4.
2. `type` is one of the allowed values.
3. `client` is a valid `u16`.
4. `tx` is a valid `u32`.
5. `amount` per the rules of the type:
   - for `deposit` and `withdrawal`: present, parses, greater than zero, at most 4 significant decimal places;
   - for `dispute`, `resolve` and `chargeback`: missing or empty.

**Stage 2. Idempotency (category "data"):**

6. Only for `deposit` and `withdrawal`: the `tx` has not been used yet (OQ-9).

**Stage 3. Account (category "business"):**

7. For all types except `deposit`: the client's account exists (OQ-10).
8. Only for `deposit` and `withdrawal`: the account is not locked (OQ-3). For a `deposit` by a new client the check is skipped, because there is no account yet.

**Stage 4. Rules per operation type (category "business"):**

| Type | Checks in order |
|---|---|
| `deposit` | 9. No overflow of `available + amount` |
| `withdrawal` | 9. `available ≥ amount` |
| `dispute` | 9. `tx` is a processed deposit. 10. `client` matches the owner of `tx`. 11. State is `Processed`. 12. No overflow |
| `resolve` | 9. `tx` is a processed deposit. 10. `client` matches the owner of `tx`. 11. State is `Disputed`. 12. No overflow |
| `chargeback` | 9. `tx` is a processed deposit. 10. `client` matches the owner of `tx`. 11. State is `Disputed`. 12. No overflow |

Notes:

- **An unknown `tx` and a withdrawal `tx` are not distinguished.** For `dispute`, `resolve` and `chargeback`, an unknown `tx`, a withdrawal `tx` and the `tx` of a rejected deposit all give the same reason: "`tx` is not a processed deposit". There is no need to store separately which `tx` was a withdrawal just for a more precise message (OQ-9).
- **A withdrawal has no overflow check.** If `available ≥ amount > 0`, the subtraction cannot overflow.
- **The account check comes before the `tx` owner check.** If the client in the row does not exist, the reason is "account does not exist", and information about someone else's transaction does not get into the log.
- **Link to question G.** If a repeated dispute on a transaction in the `Disputed` state is assigned to category "data", its check will need to be reconciled with this order. This will be decided when G is discussed.

---

## 17. Order of startup checks (B in the spec review)

**Status: agreed (2026-11-07). Revised 2026-11-09, see the note below.**

If several things are wrong at once (arguments, configuration, log file, input file), the order of checks determines which exit code is returned.

### How real systems do it

- **Fail fast at startup.** A service validates all configuration at startup and refuses to start if it is invalid (the 12-factor app principle: configuration comes from the environment and is validated before work begins). A configuration error discovered in the middle of processing is worse: part of the data has already been processed under the wrong rules.
- **Input data is not touched until the environment has been checked.** A batch job first makes sure it can run and keep a log, and only then opens the input file. Then all subsequent errors, including an error opening the input, get into the log.
- **All configuration errors are reported at once.** The operator fixes them in one go rather than restarting the job after each error found.

### Proposal

| Step | Action | Code on error |
|---|---|---|
| 1 | Check arguments: exactly one | `64` |
| 2 | ~~Load `.env`~~ (removed 2026-11-09, see OQ-23) | — |
| 3 | ~~Validate all configuration parameters (`LOG_FILE`, `RUST_LOG`)~~ (removed 2026-11-09, see OQ-29) | — |
| 4 | Initialise the log: open `LOG_FILE` if it is set | `73` |
| 5 | Open the input file | `66` |
| 6 | Process the file and print the result | `0`, `3`, `4`, `74` |

Notes:

- **Arguments are checked first:** this is the cheapest check, and it does not depend on the environment.
- **Configuration is checked before the log,** because the log parameters are themselves part of the configuration.
- **All configuration errors are printed in one message.** If both `LOG_FILE` (for example, an empty value) and `RUST_LOG` are invalid, both errors go to stderr, and the program exits with code `78`.
- **Errors in steps 1–3 are printed directly to stderr,** because the log is not yet initialised. Errors in steps 5–6 are written to the log and to stderr per the rules of spec 3.1.
- **The input file is opened after the log,** so that an error opening it also gets into the log if the log is written to a file.

**Revised (2026-11-09):** steps 2 and 3 were removed (OQ-23, OQ-29). The startup order in force: arguments (`64`) → log (`73`) → input file (`66`) → processing (`0`, `3`, `4`, `74`). `main` reads the log parameters before step 1, without validation. A directory in place of the input file also yields `66`.

---

## 18. Registering the `tx` of rows with a data error (V in the spec review)

**Status: agreed (2026-11-07).**

OQ-9 says that rows that "failed parsing" do not register their `tx`. It is unclear whether this applies to a `deposit` or `withdrawal` whose `tx` parsed but whose amount is zero, negative or too precise.

### How real systems do it

- **An idempotency key is stored only if the request passed validation and started executing.** This is how, for example, payment service APIs work: if the request parameters fail validation, no result is stored for the key, and the sender can retry the corrected request with the same key.
- The logic is: an invalid request is not yet an operation but a data error. If the key is "burned" on it, the sender cannot send the corrected operation under the original identifier and has to invent a new one, which breaks end-to-end tracing.
- **A request that passed validation and was rejected by a business rule consumes the key** (OQ-9). It is already a completed operation with the result "rejected".

### Proposal

- **A `tx` is registered only if the row passed checks 1–6 from OQ-16 (the "parsing" and "idempotency" stages).** In other words, rows of category "data" do not register their `tx`, and rows of category "business" do.
- This coincides with the stage boundary in OQ-16, so in code the rule is expressed in one place: the `tx` is registered right after check 6, before the account and business-rule checks.
- **Consequence.** If the sender sent a `deposit` with `tx = 5` and an invalid amount, and then a corrected `deposit` with the same `tx = 5`, the second one is accepted. This is the expected behaviour: correcting a data error under the original identifier.
- In OQ-9 and spec section 5.3, replace the wording "rows that failed parsing" with "rows of category 'data'".

---

## 19. Category of repeated and invalid dispute transitions (G in the spec review)

**Status: agreed (2026-11-07), option A.**

Currently all invalid transitions from spec section 8 belong to category "business" (code `3`). But some of them are in essence a resent message, like a repeated `tx`, which is assigned to "data" (code `4`, OQ-13). The question is whether to move them to "data".

### What transitions occur

| Row | `tx` state | What it most likely is |
|---|---|---|
| `dispute` | `Disputed` | Repeat of the same `dispute` |
| `chargeback` | `ChargedBack` | Repeat of the same `chargeback` |
| `resolve` | `Processed`, a dispute existed and was closed | Repeat of the same `resolve` |
| `resolve` | `Processed`, there was no dispute | Sequence error: closing a non-existent dispute |
| `chargeback` | `Processed` | Sequence error |
| `dispute`, `resolve` | `ChargedBack` | Message on a closed transaction |

### How real systems do it

- Card schemes reject a repeated chargeback on the same transaction with a separate "duplicate" code. That is, a repeat is recognised and marked separately from other errors.
- Out-of-order messages (closing a dispute that does not exist; a dispute on a closed transaction) go to an exception queue as a violation of the dispute lifecycle.
- In both cases the message is not executed. The only difference is how it is classified for investigation.

### Options

| Option | Pros | Cons |
|---|---|---|
| A. All invalid transitions are "business" (as now) | Simple and uniform: everything that depends on engine state is "business". The property of OQ-16 is preserved: "data" is detected at stages 1–2 and always wins over "business". The task's spec itself describes these cases as normal ("ignore if the dispute is not open") | A repeated dispute and a repeated `tx` are classified differently, although they are of the same nature |
| B. Exact repeats (the first three rows of the table) are "data", the rest "business" | Uniform with a repeated `tx`. Closer to card scheme practice | A repeat can only be recognised at stage 4, after the account check, the owner check and the `tx` lookup. The OQ-16 property "data always wins over business" no longer holds. For `resolve`, "a dispute existed and was closed" must be distinguished from "there was no dispute", which needs `dispute_count` |

### Proposal

- **Option A.** The deciding argument is the agreed order of checks (OQ-16). A repeated dispute is detected only by the transaction state, that is, at the business-rules stage. If it is assigned to "data", the exit code starts to depend on which check fired first rather than on the severity of the category.
- An additional argument: `tx` uniqueness is guaranteed by the spec, so its violation is an unambiguous data error. There is no such guarantee for `dispute`, `resolve` and `chargeback`: the spec explicitly describes their invalid variants as situations to be ignored.
- So that repeats can still be distinguished during investigation, the log states a separate reason for them, for example `reason="dispute already open"` or `reason="already charged back"`.

---

## 20. CSV errors within a row (D in the spec review)

**Status: agreed (2026-11-07). Quote parsing in `csv` is disabled, quotes around values are stripped when fields are parsed. Revised 2026-11-09, see the note below.**

Besides errors in field values, a row can be corrupted at the level of the CSV itself: invalid UTF-8, quotes, a BOM at the start of the file, empty lines. It needs to be decided which of these errors are fatal and which reject a single row.

### How real systems do it

- **The encoding is fixed by the format.** ISO 20022 requires UTF-8, NACHA fixed-width ASCII. Invalid characters in a record lead to its rejection, and a violation of the file structure to rejection of the whole file.
- **Structural corruption is more dangerous than an error in a value,** because it can silently shift record boundaries. So payment file formats try to rule out parsing ambiguity: fixed width in NACHA, a strict schema in ISO 20022.

### A quirk of the `csv` crate: an unclosed quote

The `csv` parser supports quoted fields by default and handles them "leniently": an unclosed quote does not produce an error but swallows everything up to the next quote or to the end of the file. One stray quote on line 10 turns lines 10–1 000 000 into one giant field of a single record. That record is rejected for a wrong number of fields, and all the other lines silently disappear.

Our fields (type, integers, decimal amount) never contain commas, line breaks or quotes, so our format has no need for quotes.

### Proposal

| Situation | Handling |
|---|---|
| Quotes | Quote parsing is disabled (`quoting(false)`). The `"` character becomes an ordinary field character, the field does not parse, and only this row is rejected (category "data"). A single quote can no longer "eat" the rest of the file |
| Invalid UTF-8 in a row | The row is rejected, category "data". Processing continues |
| UTF-8 BOM at the start of the file | Stripped before header recognition, otherwise the first column would be `\u{feff}type` and the header would not be recognised. During implementation, check whether `csv` does this itself |
| Empty lines | Skipped silently: counted neither as processed nor as rejected (the default behaviour of `csv`) |
| I/O error while reading | Fatal error, code `74` (as now) |

In total, only I/O errors remain fatal. Any corruption of an individual row rejects only that row.

If quotes are disabled, a file with a quoted amount (`deposit,1,1,"1.0"`) will be rejected. This is acceptable: our format does not provide for quotes. But if the automated check may send quoted fields, this becomes a risk comparable to the header decision (OQ-12).

### Decision: stripping quotes when parsing fields

To avoid losing files where values are wrapped in quotes, while still not allowing the rest of the file to be "swallowed", quotes are stripped by our code when each field is parsed, not by the `csv` parser:

1. The field value is trimmed of whitespace.
2. If it then starts and ends with the `"` character and its length is at least 2, one outer pair of quotes is removed, and the value is trimmed of whitespace again.
3. The value is then parsed as usual.

Examples:

| Raw field | After processing | Result |
|---|---|---|
| `"1.0"` | `1.0` | Accepted |
| ` " 1.0 " ` | `1.0` | Accepted |
| `""` | empty | Empty field (allowed for `amount` on `dispute`, `resolve`, `chargeback`) |
| `"1.0` | `"1.0` | An unpaired quote is not stripped, the field does not parse, the row is rejected ("data") |
| `"de"posit"` | `de"posit` | Does not parse as `type`, the row is rejected |

Properties of the decision:

- A record boundary is determined only by a line break, and a field boundary only by a comma. A quote never affects splitting, so one error affects exactly one row.
- A quoted value with a comma inside (`"1,0"`) is split into two fields, and the row is rejected for a wrong number of fields. This is correct: in our format a comma inside a value is impossible.
- Escaped quotes (`""` inside a value) are not supported: our fields cannot contain quotes.
- Quote stripping applies to all fields, including the first row during header recognition (OQ-12), so the header `"type","client","tx","amount"` is recognised too.

**During implementation (2026-11-09):** the `csv` crate is not used at all. With quotes disabled, all it provides is splitting on commas, and it counts line numbers incorrectly (OQ-21). Lines are read via `BufRead::read_until(b'\n')` and split with `split(',')`. The reader handles the BOM at the start of the file and empty lines (zero-length or whitespace-only) itself; the rules from the table above have not changed.

---

## 21. Line number in the log (E in the spec review)

**Status: agreed (2026-11-07). Revised 2026-11-09, see the note below.**

### How real systems do it

- A report on rejected records refers to a record so that it can be found unambiguously in the source file: by the record's sequence number in the file (in NACHA each record has a fixed position and a trace number) or by the operation identifier (`pain.002` states the identifiers of the original message and transaction).
- Usually both are given: the number is needed to open the file at the right place, and the identifier to link the rejection to the operation in other systems.

### Proposal

- Each message about a rejected row states the **physical line number in the file, starting from 1**. All lines are counted: the header, if present, and empty lines. The number matches what a text editor shows and the output of `sed -n '<N>p'`.
- Since quote parsing is disabled (OQ-20), one CSV record always occupies exactly one physical line, and the number is unambiguous. The number is taken from the record position reported by `csv`.

**Revised (2026-11-09):** `csv` counts the number incorrectly: for `a\r\nb\r\nc` it gives 1, 1, 2, and it skips empty lines without counting them. So the reader is our own and counts lines itself, one per `read_until` (OQ-20).
- If `client` and `tx` could be parsed, they are also stated in the message. If not, only the line number is stated.
- Example: `ERROR kind=rejected line=12 client=3 tx=41 category=business reason="insufficient funds"`.

---

## 22. Test plan (Zh in the spec review)

**Status: agreed (2026-11-07), including `proptest` as a dev dependency. Revised 2026-11-09, see the note below.**

Spec section 12 does not cover much of what was agreed later: header recognition, quote stripping, the order of checks, exit codes, configuration and the log.

### How real systems do it

- **Unit tests of the state machine** for every transition, including forbidden ones.
- **Invariant checks (property-based testing).** Random sequences of operations are generated, and after each one it is checked that monetary invariants are not violated: `total = available + held`, money neither appears nor disappears. This finds combinations nobody thought of.
- **Golden files**: a set of inputs with pre-verified results, run on every change to catch regressions.
- **End-to-end tests** run the finished program as a black box and check everything the caller sees: stdout, stderr, exit code.
- **Parser fuzzing**: on arbitrary bytes the parser must not crash.

### Proposal

**1. Engine unit tests** (no I/O, on typed transactions):

- every transition in spec section 8, including all rejected ones, checking the rejection reason;
- locked account: rejection of `deposit` and `withdrawal`, processing of disputes (7.2);
- negative `available` after a dispute and rejection of a withdrawal (7.3);
- client mismatch (7.4) and missing account (5.1);
- reopening a dispute after a resolve and growth of `dispute_count`;
- `tx` registration: an operation rejected by a business rule consumes the `tx`, and a repeated `tx` is rejected (5.3);
- order of checks: rows to which several reasons apply are rejected by the first one (10.3);
- `Decimal` overflow: the operation is rejected, state does not change.

**2. Row parsing unit tests:**

- `amount`: precision, trailing zeros, zero, negative values, garbage, an empty value on a deposit, a non-empty value on a dispute;
- whitespace trimming and quote stripping (all examples from OQ-20);
- header recognition: exact match, different case, quoted, with 3 fields, similar but not matching;
- number of fields, unknown `type`, invalid `client` and `tx`, invalid UTF-8;
- `tx` registration does not happen for rows of category "data" (OQ-18).

**3. Configuration unit tests** (removed 2026-11-09 together with configuration validation, see OQ-29):

- `LOG_FILE`: not set, valid path, empty value;
- invalid `RUST_LOG`;
- several errors at once are printed in one message.

The configuration parsing function takes parameter values as an argument (for example, a lookup function by name) rather than reading the process environment. The environment is global to the process, and Rust tests run in parallel by default, so tests that change environment variables would interfere with each other.

**4. CLI end-to-end tests** (the `tests/` directory, running the built program via `std::process::Command` with an explicitly set environment):

- golden sets in `tests/data/<scenario>/`: `input.csv`, `expected.csv` and `exit_code`. The test compares stdout with `expected.csv` ignoring row order and checks the exit code;
- set scenarios: the example from the task, a full dispute cycle, chargeback and lock, reopening a dispute, a file without a header, quotes and whitespace, a mix of data errors and business rejections;
- fatal error codes: `64` (arguments), `66` (no input file), `73` (inaccessible `LOG_FILE`), ~~`78` (invalid configuration)~~ (removed 2026-11-09), checking that stdout is empty;
- log: nothing but CSV ever gets into stdout; with `LOG_FILE` the log is appended to the file; a fatal error is visible in stderr even when logging to a file.

**5. Invariant tests** (the `proptest` crate, dev dependency only; not implemented as of 2026-11-09):

- on random sequences of operations, after each one `total = available + held`, `held ≥ 0` and conservation of money are checked: the sum of `total` over all accounts equals the sum of accepted deposits minus accepted withdrawals and chargebacks;
- on random strings, row parsing does not panic.

**6. Streaming check:** a test generating a large input (millions of rows), marked `#[ignore]` so as not to slow down the normal run. Run manually via `cargo test -- --ignored`.

**7. Code quality:** `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` with no findings.

---

## 23. Locating the `.env` file (Z in the second spec review)

**Status: agreed (2026-11-07). Revised 2026-11-09, see the note below.**

`dotenvy::dotenv()` by default searches for `.env` not only in the current directory but also in all parent directories up the tree. The program could silently pick up someone else's `.env` (for example, from the home directory or from another project's directory) and process payments with someone else's settings.

In real systems the configuration source is set explicitly and unambiguously. Implicit configuration lookup up the directory tree is considered a risk: the result depends on where the program is run from and what lies higher up the tree.

**Decision:** `.env` is loaded only from the current directory (`dotenvy::from_path(".env")`). If the file is not there, it is not an error; parameters are taken from the environment only.

**Revised (2026-11-09):** `.env` is not supported at all; parameters are read only from the process environment (`std::env::var_os`). The `dotenvy` crate and the `.env` loading step have been removed from the startup order (OQ-17). Reasons: the program has two parameters, which are simpler to set in the environment; there is no second source and no precedence rules; there is no implicit dependency on the current directory.

---

## 24. Content of the log summary line (I in the second spec review)

**Status: agreed (2026-11-07).**

The wording "how many rows were processed" was ambiguous: it was unclear whether the header, empty rows and rejected rows were counted.

**Decision:** the summary line at the `info` level contains the fields:

| Field | Meaning |
|---|---|
| `read` | Data rows read, excluding the header and empty lines |
| `accepted` | Accepted and applied |
| `rejected_data` | Rejected, category "data" |
| `rejected_business` | Rejected, category "business" |

`read = accepted + rejected_data + rejected_business` always holds. This is also a self-check: if the equality is violated, some row was lost without being counted. It is the counterpart of control totals in payment files (OQ-12).

Example: `INFO kind=summary read=1000 accepted=990 rejected_data=4 rejected_business=6`.

---

## 25. `total` overflow (K in the third spec review)

**Status: agreed (2026-11-07).**

`deposit` checked only for overflow of `available + amount`. But `total = available + held` is computed separately and can overflow on its own: `deposit MAX` → `dispute` (`held = MAX`, `available = 0`) → another `deposit MAX`. The check passes, and on output `total = 2·MAX` causes a panic.

In real systems an account balance is bounded by the width of the storage, and an operation that would take it beyond the limit is rejected before posting, not discovered at reporting time.

**Decision:** `deposit` checks for overflow of both `available + amount` and `available + held + amount`. The invariant "`total` is always representable" has been added to the invariant tests. Other operations do not increase `total`.

**Clarification (2026-11-08).** The bound on `total` does not protect `available` and `held` individually. Example with `M = Decimal::MAX - 1`: `deposit M` (tx1) → `withdrawal M` → `deposit M` (tx2) → `withdrawal M` → `dispute tx1` (`available = -M`, `held = M`) → `dispute tx2`. The new values `available = -2M` and `held = 2M` are not representable, even though `total = 0`. The ordinary `Decimal` operators panic at this point.

Two options were considered: put an upper bound on `Amount` so that the sum of even 2³² deposits does not reach `Decimal::MAX`, or check for overflow in every operation. The check was chosen: `hold`, `release` and `charge_back` compute the new `available` and `held` via `checked_*` and return `Overflow`. The row is rejected with category "business", the account and the dispute state do not change, `dispute_count` does not grow.

---

## 26. Number grammar (L in the third spec review)

**Status: agreed (2026-11-07). The grammar comes from the libraries and is not defined separately.**

`Decimal` accepts forms such as `+5` and `1_000`, and `u16`/`u32::from_str` accept `+5` and leading zeros.

**Decision:** no custom grammar is introduced; the rules of the standard library and `rust_decimal` apply. This is recorded in the spec (4.1) and the README.

**Clarification:** `Decimal::from_str` silently rounds values with more than ~28 significant digits. For example, `1.00000000000000000000000000001` would become `1`, which violates OQ-2. So `Decimal::from_str_exact` is used: the grammar is the same, but on excess precision it returns an error, and the row is rejected with category "data".

---

## 27. Case of `type` (M in the third spec review)

**Status: agreed (2026-11-07).**

The header is recognised case-insensitively, but no rule was set for `type` values.

**Decision:** `type` is lowercased before comparison. `Deposit` and `DEPOSIT` are allowed.

---

## 28. Negative zero in the output (N in the third spec review)

**Status: agreed (2026-11-07).**

`rust_decimal` can produce `-0` (the `normalize` method turns it into `0`). Without normalisation, `-0.0000` could appear in the output.

**Decision:** every amount is normalised before output. `-0.0000` is never printed.

---

## 29. Empty value of a configuration parameter (O in the third spec review)

**Status: agreed (2026-11-07). Revised 2026-11-09, see the note below.**

It was unclear what an empty parameter value (for example, `RUST_LOG=`) means: "not set" or "invalid". The question arose with the example `MAX_DISPUTES_PER_TX=`; this parameter was later abolished (OQ-7, 2026-11-08).

**Decision:** an empty value of any parameter is treated as invalid, exit code `78`. An explicit empty string usually means a typo. To make a parameter have no effect, it is not set at all.

**Revised (2026-11-09):** configuration validation has been removed entirely, code `78` is no longer used. `main` reads the environment variables into a simple `AppConfig` struct. An empty `LOG_FILE` cannot be opened and yields `73`; an empty or invalid `RUST_LOG` is parsed leniently by `EnvFilter::new` (invalid directives are skipped, the default is `error`). Reason: two log parameters are not worth a separate validation layer.

---

## 30. Empty lines and the header (P in the third spec review)

**Status: agreed (2026-11-07).**

It was not defined whether a line consisting only of whitespace counts as empty, and where the header is looked for if the file starts with empty lines.

**Decision:**

- an empty line is a line of zero length or consisting only of whitespace characters; it is skipped and not counted in the totals;
- a line consisting only of delimiters (`,,,`) is not considered empty and is rejected;
- the header is looked for in the first non-empty line;
- a line break is LF or CRLF.

---

## 31. Summary line on a fatal error (R in the third spec review)

**Status: agreed (2026-11-07), the summary line is not written on a fatal error.**

It was not defined whether the log summary line (OQ-24) is written if processing was cut short with code `74`.

Code `74` is possible in two cases:

- **an error reading the input in the middle of the file:** a disk failure (`EIO`), a file on a network FS became unavailable (for example, a stale NFS handle), the input is a named pipe or a device that returned an error;
- **an error writing the result:** out of disk space or quota with `> accounts.csv` (`ENOSPC`, `EDQUOT`), the pipe reader closed early (`cargo run -- in.csv | head` gives `EPIPE`: Rust ignores `SIGPIPE`, so the write returns an error rather than killing the process).

Errors in the file content (invalid UTF-8, wrong number of fields, quotes) do not lead to `74`: with quote parsing disabled and raw records being read, the `csv` parser returns only I/O errors, and everything else is rejected row by row (OQ-1, OQ-20).

**Decision:** the summary line is written only with codes `0`, `3` and `4`. On a fatal error it is absent: the totals of an incomplete run mean nothing, and the cause is reported by the fatal error message.
