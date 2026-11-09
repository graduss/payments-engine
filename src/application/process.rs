use std::fmt;

use crate::{
    application::{engine::PaymentEngine, error::ApplicationError},
    domain::{ClientId, DomainError, Event, EventKind, TxId},
};

/// A line from the input: the parsed event, or why it could not be parsed.
/// `E` is the parse error of the input format.
#[derive(Debug)]
pub struct Row<E> {
    pub line: u64,
    pub content: Result<Event, InvalidRow<E>>,
}

/// A line that failed to parse. `client` and `tx` are kept if they could be
/// read, so the log can show them.
#[derive(Debug)]
pub struct InvalidRow<E> {
    pub client: Option<ClientId>,
    pub tx: Option<TxId>,
    pub error: E,
}

/// Bad input data or a refused operation. Decides the exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Data,
    Business,
}

/// An applied row, as `process` reports it to `on_accepted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
    pub line: u64,
    pub client: ClientId,
    pub tx: TxId,
    pub kind: EventKind,
}

/// What `on_rejected` gets for each rejected row.
pub struct Rejection<'a> {
    pub line: u64,
    pub client: Option<ClientId>,
    pub tx: Option<TxId>,
    pub category: Category,
    pub reason: &'a dyn fmt::Display,
}

/// Row counts for the summary line.
/// `read == accepted + rejected_data + rejected_business`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub read: u64,
    pub accepted: u64,
    pub rejected_data: u64,
    pub rejected_business: u64,
}

/// How the run went. The CLI turns this into an exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Clean,
    BusinessRejects,
    DataRejects,
}

impl Summary {
    /// Data errors win over business errors.
    pub fn outcome(&self) -> Outcome {
        if self.rejected_data > 0 {
            Outcome::DataRejects
        } else if self.rejected_business > 0 {
            Outcome::BusinessRejects
        } else {
            Outcome::Clean
        }
    }

    fn count(&mut self, category: Category) {
        match category {
            Category::Data => self.rejected_data += 1,
            Category::Business => self.rejected_business += 1,
        }
    }
}

fn category_of(error: &ApplicationError) -> Category {
    match error {
        ApplicationError::Domain(DomainError::AmountNotPositive)
        | ApplicationError::Domain(DomainError::AmountTooPrecise(_))
        | ApplicationError::Domain(DomainError::UnsupportedEvent)
        | ApplicationError::DuplicateTx => Category::Data,
        _ => Category::Business,
    }
}

enum Reason<E> {
    Parse(E),
    Application(ApplicationError),
}

impl<E: fmt::Display> fmt::Display for Reason<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reason::Parse(error) => error.fmt(f),
            Reason::Application(error) => error.fmt(f),
        }
    }
}

/// Feeds every row to the engine and counts what happened.
///
/// Rejected and accepted rows go to the two callbacks; `process` does not log
/// anything itself. An `Err` from `input` is fatal: processing stops and the
/// error is returned.
pub fn process<E, F>(
    engine: &mut PaymentEngine,
    input: impl IntoIterator<Item = Result<Row<E>, F>>,
    mut on_rejected: impl FnMut(&Rejection<'_>),
    mut on_accepted: impl FnMut(&Accepted),
) -> Result<Summary, F>
where
    E: fmt::Display,
{
    input
        .into_iter()
        .try_fold(Summary::default(), |mut summary, row| {
            let Row { line, content } = row?;
            summary.read += 1;

            let outcome = content
                .map_err(|invalid| {
                    (
                        Category::Data,
                        invalid.client,
                        invalid.tx,
                        Reason::Parse(invalid.error),
                    )
                })
                .and_then(|event| {
                    let accepted = Accepted {
                        line,
                        client: event.client,
                        tx: event.tx,
                        kind: event.kind,
                    };
                    engine.apply(event).map(|()| accepted).map_err(|err| {
                        (
                            category_of(&err),
                            Some(accepted.client),
                            Some(accepted.tx),
                            Reason::Application(err),
                        )
                    })
                });

            match outcome {
                Ok(accepted) => {
                    summary.accepted += 1;
                    on_accepted(&accepted);
                }
                Err((category, client, tx, reason)) => {
                    summary.count(category);
                    on_rejected(&Rejection {
                        line,
                        client,
                        tx,
                        category,
                        reason: &reason,
                    });
                }
            }

            Ok(summary)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Amount, EventKind};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    /// Parse error of the fake reader.
    type ParseError = &'static str;
    /// Fatal error of the fake reader.
    #[derive(Debug, PartialEq, Eq)]
    struct Fatal;

    type Input = Result<Row<ParseError>, Fatal>;

    /// A rejection with the reason rendered, so it can outlive the callback.
    #[derive(Debug, PartialEq, Eq)]
    struct Rejected {
        line: u64,
        client: Option<ClientId>,
        tx: Option<TxId>,
        category: Category,
        reason: String,
    }

    fn event(kind: EventKind, client: u16, tx: u32, amount: Option<Decimal>) -> Event {
        Event {
            client: ClientId::from(client),
            tx: TxId::from(tx),
            kind,
            amount: amount.map(|value| Amount::try_from(value).unwrap()),
        }
    }

    fn ok(line: u64, event: Event) -> Input {
        Ok(Row {
            line,
            content: Ok(event),
        })
    }

    fn invalid(line: u64, client: Option<u16>, tx: Option<u32>, error: ParseError) -> Input {
        Ok(Row {
            line,
            content: Err(InvalidRow {
                client: client.map(ClientId::from),
                tx: tx.map(TxId::from),
                error,
            }),
        })
    }

    fn deposit(line: u64, client: u16, tx: u32, amount: Decimal) -> Input {
        ok(line, event(EventKind::Deposit, client, tx, Some(amount)))
    }

    fn withdrawal(line: u64, client: u16, tx: u32, amount: Decimal) -> Input {
        ok(line, event(EventKind::Withdrawal, client, tx, Some(amount)))
    }

    fn dispute(line: u64, client: u16, tx: u32) -> Input {
        ok(line, event(EventKind::Dispute, client, tx, None))
    }

    fn run(
        engine: &mut PaymentEngine,
        input: Vec<Input>,
    ) -> (Result<Summary, Fatal>, Vec<Rejected>) {
        let mut rejected = Vec::new();
        let result = process(
            engine,
            input,
            |rejection| {
                rejected.push(Rejected {
                    line: rejection.line,
                    client: rejection.client,
                    tx: rejection.tx,
                    category: rejection.category,
                    reason: rejection.reason.to_string(),
                });
            },
            |_| {},
        );
        (result, rejected)
    }

    fn available(engine: &PaymentEngine, client: u16) -> Decimal {
        engine
            .accounts()
            .find(|account| account.client_id() == ClientId::from(client))
            .unwrap()
            .available()
    }

    fn counts(read: u64, accepted: u64, rejected_data: u64, rejected_business: u64) -> Summary {
        Summary {
            read,
            accepted,
            rejected_data,
            rejected_business,
        }
    }

    // --- process ---

    #[test]
    fn reports_each_accepted_row() {
        let mut engine = PaymentEngine::new();
        let mut accepted = Vec::new();
        let input = vec![
            deposit(1, 1, 1, dec!(5)),
            withdrawal(2, 1, 2, dec!(9)),
            dispute(4, 1, 1),
        ];
        let summary = process(&mut engine, input, |_| {}, |row| accepted.push(*row)).unwrap();

        assert_eq!(summary.accepted, 2);
        let tx = |line, tx, kind| Accepted {
            line,
            client: ClientId::from(1),
            tx: TxId::from(tx),
            kind,
        };
        assert_eq!(
            accepted,
            vec![tx(1, 1, EventKind::Deposit), tx(4, 1, EventKind::Dispute)]
        );
    }

    #[test]
    fn empty_input_gives_empty_clean_summary() {
        let mut engine = PaymentEngine::new();
        let (result, rejected) = run(&mut engine, vec![]);
        let summary = result.unwrap();
        assert_eq!(summary, Summary::default());
        assert_eq!(summary.outcome(), Outcome::Clean);
        assert!(rejected.is_empty());
        assert_eq!(engine.accounts().count(), 0);
    }

    #[test]
    fn accepted_rows_are_applied_and_counted() {
        let mut engine = PaymentEngine::new();
        let (result, rejected) = run(
            &mut engine,
            vec![
                deposit(2, 1, 1, dec!(10)),
                withdrawal(3, 1, 2, dec!(4)),
                dispute(4, 1, 1),
            ],
        );
        let summary = result.unwrap();
        assert_eq!(summary, counts(3, 3, 0, 0));
        assert_eq!(summary.outcome(), Outcome::Clean);
        assert!(rejected.is_empty());
        assert_eq!(available(&engine, 1), dec!(-4));
    }

    #[test]
    fn parse_error_is_a_data_rejection_with_the_known_ids() {
        let mut engine = PaymentEngine::new();
        let (result, rejected) = run(
            &mut engine,
            vec![
                invalid(2, Some(7), None, "invalid tx"),
                invalid(5, None, None, "unknown type"),
            ],
        );
        assert_eq!(result.unwrap(), counts(2, 0, 2, 0));
        assert_eq!(
            rejected,
            vec![
                Rejected {
                    line: 2,
                    client: Some(ClientId::from(7)),
                    tx: None,
                    category: Category::Data,
                    reason: "invalid tx".into(),
                },
                Rejected {
                    line: 5,
                    client: None,
                    tx: None,
                    category: Category::Data,
                    reason: "unknown type".into(),
                },
            ]
        );
        assert_eq!(engine.accounts().count(), 0);
    }

    #[test]
    fn business_error_is_a_business_rejection_with_row_ids() {
        let mut engine = PaymentEngine::new();
        let (result, rejected) = run(
            &mut engine,
            vec![deposit(2, 1, 1, dec!(1)), withdrawal(3, 1, 2, dec!(5))],
        );
        let summary = result.unwrap();
        assert_eq!(summary, counts(2, 1, 0, 1));
        assert_eq!(summary.outcome(), Outcome::BusinessRejects);
        assert_eq!(
            rejected,
            vec![Rejected {
                line: 3,
                client: Some(ClientId::from(1)),
                tx: Some(TxId::from(2)),
                category: Category::Business,
                reason: "insufficient funds".into(),
            }]
        );
        assert_eq!(available(&engine, 1), dec!(1));
    }

    #[test]
    fn duplicate_tx_is_a_data_rejection() {
        let mut engine = PaymentEngine::new();
        let (result, rejected) = run(
            &mut engine,
            vec![deposit(2, 1, 1, dec!(1)), deposit(3, 1, 1, dec!(1))],
        );
        assert_eq!(result.unwrap(), counts(2, 1, 1, 0));
        assert_eq!(rejected[0].category, Category::Data);
        assert_eq!(rejected[0].reason, "duplicate tx");
        assert_eq!(available(&engine, 1), dec!(1));
    }

    #[test]
    fn event_without_required_amount_is_a_data_rejection() {
        let mut engine = PaymentEngine::new();
        let (result, rejected) = run(
            &mut engine,
            vec![ok(2, event(EventKind::Withdrawal, 1, 1, None))],
        );
        assert_eq!(result.unwrap(), counts(1, 0, 1, 0));
        assert_eq!(rejected[0].category, Category::Data);
        assert_eq!(rejected[0].reason, "unsupported event");
    }

    #[test]
    fn data_rejects_take_priority_in_outcome() {
        let mut engine = PaymentEngine::new();
        let (result, _) = run(
            &mut engine,
            vec![
                withdrawal(2, 1, 1, dec!(1)),
                invalid(3, None, None, "bad row"),
                deposit(4, 1, 2, dec!(1)),
            ],
        );
        let summary = result.unwrap();
        assert_eq!(summary, counts(3, 1, 1, 1));
        assert_eq!(summary.outcome(), Outcome::DataRejects);
    }

    #[test]
    fn processing_continues_after_rejected_rows() {
        let mut engine = PaymentEngine::new();
        let (result, rejected) = run(
            &mut engine,
            vec![
                invalid(2, None, None, "bad row"),
                dispute(3, 1, 1),
                deposit(4, 1, 1, dec!(2)),
                deposit(5, 1, 2, dec!(3)),
            ],
        );
        assert_eq!(result.unwrap(), counts(4, 2, 1, 1));
        assert_eq!(
            rejected.iter().map(|r| r.line).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(available(&engine, 1), dec!(5));
    }

    #[test]
    fn fatal_error_stops_processing_and_keeps_earlier_rows() {
        let mut engine = PaymentEngine::new();
        let (result, rejected) = run(
            &mut engine,
            vec![
                deposit(2, 1, 1, dec!(2)),
                Err(Fatal),
                deposit(4, 1, 2, dec!(3)),
            ],
        );
        assert_eq!(result, Err(Fatal));
        assert!(rejected.is_empty());
        assert_eq!(available(&engine, 1), dec!(2));
    }

    #[test]
    fn read_equals_accepted_plus_rejected() {
        let mut engine = PaymentEngine::new();
        let (result, rejected) = run(
            &mut engine,
            vec![
                deposit(2, 1, 1, dec!(5)),
                deposit(3, 1, 1, dec!(5)),
                invalid(4, Some(1), Some(3), "bad amount"),
                withdrawal(5, 2, 4, dec!(1)),
                dispute(6, 1, 1),
                dispute(7, 1, 1),
            ],
        );
        let summary = result.unwrap();
        assert_eq!(summary, counts(6, 2, 2, 2));
        assert_eq!(
            summary.read,
            summary.accepted + summary.rejected_data + summary.rejected_business
        );
        assert_eq!(rejected.len() as u64, summary.read - summary.accepted);
    }

    // --- Summary::outcome ---

    #[test]
    fn outcome_reflects_worst_rejection() {
        assert_eq!(counts(5, 5, 0, 0).outcome(), Outcome::Clean);
        assert_eq!(counts(5, 4, 0, 1).outcome(), Outcome::BusinessRejects);
        assert_eq!(counts(5, 4, 1, 0).outcome(), Outcome::DataRejects);
        assert_eq!(counts(5, 3, 1, 1).outcome(), Outcome::DataRejects);
    }

    // --- category_of ---

    #[test]
    fn malformed_row_errors_are_data() {
        for error in [
            ApplicationError::DuplicateTx,
            ApplicationError::Domain(DomainError::AmountNotPositive),
            ApplicationError::Domain(DomainError::AmountTooPrecise(4)),
            ApplicationError::Domain(DomainError::UnsupportedEvent),
        ] {
            assert_eq!(category_of(&error), Category::Data, "{error:?}");
        }
    }

    #[test]
    fn rule_violations_are_business() {
        for error in [
            ApplicationError::AccountNotFound,
            ApplicationError::NotADeposit,
            ApplicationError::Domain(DomainError::AccountLocked),
            ApplicationError::Domain(DomainError::InsufficientFunds),
            ApplicationError::Domain(DomainError::ClientMismatch),
            ApplicationError::Domain(DomainError::TransactionMismatch),
            ApplicationError::Domain(DomainError::DisputeAlreadyOpen),
            ApplicationError::Domain(DomainError::NotDisputed),
            ApplicationError::Domain(DomainError::AlreadyChargedBack),
            ApplicationError::Domain(DomainError::Overflow),
        ] {
            assert_eq!(category_of(&error), Category::Business, "{error:?}");
        }
    }
}
