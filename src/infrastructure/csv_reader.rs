//! Transaction input as CSV (spec 4).

use std::io::{self, BufRead};

use rust_decimal::Decimal;
use thiserror::Error;

use crate::{
    application::{InvalidRow, Row},
    domain::{Amount, ClientId, DomainError, Event, EventKind, TxId},
};

const BOM: &[u8] = b"\xEF\xBB\xBF";
const HEADER: [&str; 4] = ["type", "client", "tx", "amount"];

/// Why the parser rejected a line. Always a data error.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RowError {
    #[error("invalid UTF-8")]
    InvalidUtf8,

    #[error("expected 3 or 4 fields, got {0}")]
    FieldCount(usize),

    #[error("unknown type")]
    UnknownType,

    #[error("invalid client")]
    InvalidClient,

    #[error("invalid tx")]
    InvalidTx,

    #[error("missing amount")]
    MissingAmount,

    #[error("invalid amount")]
    InvalidAmount,

    #[error(transparent)]
    Amount(#[from] DomainError),

    #[error("amount is not allowed for dispute, resolve and chargeback")]
    UnexpectedAmount,
}

/// Reads transactions line by line and yields one [`Row`] per data line.
/// Blank lines and the header are skipped.
///
/// The `csv` crate is not used: quoting is off anyway, and its
/// `position().line()` is wrong after CRLF endings and blank lines, while
/// rejections must point at the real line of the file.
pub struct CsvTransactionReader<R> {
    input: R,
    buf: Vec<u8>,
    line: u64,
    seen_data: bool,
}

impl<R: BufRead> CsvTransactionReader<R> {
    pub fn new(input: R) -> Self {
        Self {
            input,
            buf: Vec::new(),
            line: 0,
            seen_data: false,
        }
    }
}

impl<R: BufRead> Iterator for CsvTransactionReader<R> {
    type Item = io::Result<Row<RowError>>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            self.buf.clear();
            match self.input.read_until(b'\n', &mut self.buf) {
                Ok(0) => return None,
                Ok(_) => self.line += 1,
                Err(err) => return Some(Err(err)),
            }

            let mut bytes = self.buf.as_slice();
            if self.line == 1 {
                bytes = bytes.strip_prefix(BOM).unwrap_or(bytes);
            }

            let content = match str::from_utf8(bytes) {
                Ok(text) if text.trim().is_empty() => continue,
                Ok(text) => {
                    if !self.seen_data {
                        self.seen_data = true;
                        if is_header(text) {
                            continue;
                        }
                    }
                    parse_row(text)
                }
                Err(_) => {
                    self.seen_data = true;
                    Err(InvalidRow {
                        client: None,
                        tx: None,
                        error: RowError::InvalidUtf8,
                    })
                }
            };

            return Some(Ok(Row {
                line: self.line,
                content,
            }));
        }
    }
}

/// Row checks 1-5 of spec 10.3, in that order. `client` and `tx` are read
/// up front so that a rejection can still report them.
fn parse_row(text: &str) -> Result<Event, InvalidRow<RowError>> {
    let count = text.split(',').count();
    let mut fields = fields(text);

    let (Some(kind), Some(client), Some(tx), amount, None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return Err(InvalidRow {
            client: None,
            tx: None,
            error: RowError::FieldCount(count),
        });
    };

    let client = client.parse::<u16>().ok().map(ClientId::from);
    let tx = tx.parse::<u32>().ok().map(TxId::from);
    let invalid = |error| InvalidRow { client, tx, error };

    let kind = parse_kind(kind).ok_or_else(|| invalid(RowError::UnknownType))?;
    let client = client.ok_or_else(|| invalid(RowError::InvalidClient))?;
    let tx = tx.ok_or_else(|| invalid(RowError::InvalidTx))?;
    let amount = parse_amount(kind, amount).map_err(invalid)?;

    Ok(Event {
        client,
        tx,
        kind,
        amount,
    })
}

/// `type,client,tx` with an optional `amount`, in any case.
fn is_header(text: &str) -> bool {
    let names: Vec<&str> = fields(text).collect();
    (3..=4).contains(&names.len())
        && names
            .iter()
            .zip(HEADER)
            .all(|(name, expected)| name.eq_ignore_ascii_case(expected))
}

fn fields(text: &str) -> impl Iterator<Item = &str> {
    text.split(',').map(clean)
}

/// Trims a field and strips one pair of outer quotes. An unpaired quote is
/// left in place, so the field fails to parse.
fn clean(field: &str) -> &str {
    let field = field.trim();
    field
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .map_or(field, str::trim)
}

fn parse_kind(text: &str) -> Option<EventKind> {
    [
        ("deposit", EventKind::Deposit),
        ("withdrawal", EventKind::Withdrawal),
        ("dispute", EventKind::Dispute),
        ("resolve", EventKind::Resolve),
        ("chargeback", EventKind::Chargeback),
    ]
    .into_iter()
    .find(|(name, _)| text.eq_ignore_ascii_case(name))
    .map(|(_, kind)| kind)
}

fn parse_amount(kind: EventKind, text: Option<&str>) -> Result<Option<Amount>, RowError> {
    let text = text.filter(|t| !t.is_empty());
    match (kind, text) {
        (EventKind::Deposit | EventKind::Withdrawal, None) => Err(RowError::MissingAmount),
        (EventKind::Deposit | EventKind::Withdrawal, Some(text)) => {
            // from_str would silently round very long numbers.
            let value = Decimal::from_str_exact(text).map_err(|_| RowError::InvalidAmount)?;
            Ok(Some(Amount::try_from(value)?))
        }
        (_, Some(_)) => Err(RowError::UnexpectedAmount),
        (_, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    /// A parsed row in a form that is easy to compare.
    #[derive(Debug, PartialEq, Eq)]
    enum Parsed {
        Ok(EventKind, ClientId, TxId, Option<Decimal>),
        Err(Option<ClientId>, Option<TxId>, RowError),
    }

    fn ok(kind: EventKind, client: u16, tx: u32, amount: Option<Decimal>) -> Parsed {
        Parsed::Ok(kind, ClientId::from(client), TxId::from(tx), amount)
    }

    fn err(client: Option<u16>, tx: Option<u32>, error: RowError) -> Parsed {
        Parsed::Err(client.map(ClientId::from), tx.map(TxId::from), error)
    }

    fn read(input: &[u8]) -> Vec<(u64, Parsed)> {
        CsvTransactionReader::new(input)
            .map(|row| {
                let row = row.unwrap();
                let parsed = match row.content {
                    Ok(event) => Parsed::Ok(
                        event.kind,
                        event.client,
                        event.tx,
                        event.amount.map(Decimal::from),
                    ),
                    Err(invalid) => Parsed::Err(invalid.client, invalid.tx, invalid.error),
                };
                (row.line, parsed)
            })
            .collect()
    }

    /// Parses a single data line (no header) and returns the outcome.
    fn one(line: &str) -> Parsed {
        let mut rows = read(line.as_bytes());
        assert_eq!(rows.len(), 1, "expected one row for {line:?}");
        rows.remove(0).1
    }

    fn error(line: &str) -> RowError {
        match one(line) {
            Parsed::Err(_, _, error) => error,
            parsed => panic!("expected an error for {line:?}, got {parsed:?}"),
        }
    }

    fn amount_of(line: &str) -> Option<Decimal> {
        match one(line) {
            Parsed::Ok(_, _, _, amount) => amount,
            parsed => panic!("expected a valid row for {line:?}, got {parsed:?}"),
        }
    }

    // --- Rows and fields ---

    #[test]
    fn parses_every_type() {
        assert_eq!(
            read(b"deposit,1,10,1.5\nwithdrawal,1,11,0.5\ndispute,1,10,\nresolve,1,10\nchargeback,1,10,\n"),
            vec![
                (1, ok(EventKind::Deposit, 1, 10, Some(dec!(1.5)))),
                (2, ok(EventKind::Withdrawal, 1, 11, Some(dec!(0.5)))),
                (3, ok(EventKind::Dispute, 1, 10, None)),
                (4, ok(EventKind::Resolve, 1, 10, None)),
                (5, ok(EventKind::Chargeback, 1, 10, None)),
            ]
        );
    }

    #[test]
    fn trims_whitespace_around_fields() {
        assert_eq!(
            one(" deposit ,\t2 ,  3,  4.25 "),
            ok(EventKind::Deposit, 2, 3, Some(dec!(4.25)))
        );
    }

    #[test]
    fn type_is_case_insensitive() {
        assert_eq!(
            one("DePoSiT,1,1,1"),
            ok(EventKind::Deposit, 1, 1, Some(dec!(1)))
        );
        assert_eq!(one("CHARGEBACK,1,1"), ok(EventKind::Chargeback, 1, 1, None));
    }

    #[test]
    fn strips_one_pair_of_outer_quotes() {
        assert_eq!(
            one(r#""deposit", " 1 ",  "2" ,"3.5""#),
            ok(EventKind::Deposit, 1, 2, Some(dec!(3.5)))
        );
    }

    #[test]
    fn unpaired_quote_fails_the_field() {
        assert_eq!(error(r#""deposit,1,1,1"#), RowError::UnknownType);
        assert_eq!(error(r#"deposit,"1,1,1"#), RowError::InvalidClient);
        assert_eq!(error(r#"deposit,1,1,1""#), RowError::InvalidAmount);
        assert_eq!(error(r#"dispute,1,1,""#), RowError::UnexpectedAmount);
    }

    #[test]
    fn quotes_do_not_protect_commas() {
        assert_eq!(error(r#"deposit,1,1,"1,5""#), RowError::FieldCount(5));
    }

    #[test]
    fn rejects_wrong_field_count() {
        assert_eq!(error("deposit"), RowError::FieldCount(1));
        assert_eq!(error("deposit,1"), RowError::FieldCount(2));
        assert_eq!(error("deposit,1,1,1,"), RowError::FieldCount(5));
        assert_eq!(error(",,,"), RowError::UnknownType);
        assert_eq!(error(",,,,"), RowError::FieldCount(5));
    }

    #[test]
    fn rejects_invalid_utf8() {
        assert_eq!(
            read(b"deposit,1,1,1\ndeposit,1,2,\xFF\n"),
            vec![
                (1, ok(EventKind::Deposit, 1, 1, Some(dec!(1)))),
                (2, err(None, None, RowError::InvalidUtf8)),
            ]
        );
    }

    #[test]
    fn rejects_unknown_type() {
        assert_eq!(error("transfer,1,1,1"), RowError::UnknownType);
        assert_eq!(error(",1,1,1"), RowError::UnknownType);
    }

    #[test]
    fn rejects_invalid_client() {
        assert_eq!(error("deposit,65536,1,1"), RowError::InvalidClient);
        assert_eq!(error("deposit,-1,1,1"), RowError::InvalidClient);
        assert_eq!(error("deposit,,1,1"), RowError::InvalidClient);
        assert_eq!(error("deposit,1.0,1,1"), RowError::InvalidClient);
    }

    #[test]
    fn rejects_invalid_tx() {
        assert_eq!(error("deposit,1,4294967296,1"), RowError::InvalidTx);
        assert_eq!(error("deposit,1,abc,1"), RowError::InvalidTx);
    }

    #[test]
    fn accepts_library_integer_grammar() {
        assert_eq!(
            one("deposit,+5,007,1"),
            ok(EventKind::Deposit, 5, 7, Some(dec!(1)))
        );
        assert_eq!(
            one("deposit,65535,4294967295,1"),
            ok(EventKind::Deposit, 65535, 4294967295, Some(dec!(1)))
        );
    }

    // --- Check order and context ---

    #[test]
    fn first_failed_check_wins() {
        // type before client before tx before amount (spec 10.3).
        assert_eq!(error("transfer,x,y,z"), RowError::UnknownType);
        assert_eq!(error("deposit,x,y,z"), RowError::InvalidClient);
        assert_eq!(error("deposit,1,y,z"), RowError::InvalidTx);
        assert_eq!(error("deposit,1,1,z"), RowError::InvalidAmount);
    }

    #[test]
    fn rejection_reports_client_and_tx_when_parsed() {
        assert_eq!(
            one("transfer,3,41,1"),
            err(Some(3), Some(41), RowError::UnknownType)
        );
        assert_eq!(
            one("deposit,x,41,1"),
            err(None, Some(41), RowError::InvalidClient)
        );
        assert_eq!(
            one("deposit,3,41,0"),
            err(
                Some(3),
                Some(41),
                RowError::Amount(DomainError::AmountNotPositive)
            )
        );
        assert_eq!(one("deposit,3"), err(None, None, RowError::FieldCount(2)));
    }

    // --- Amount ---

    #[test]
    fn deposit_and_withdrawal_require_amount() {
        assert_eq!(error("deposit,1,1"), RowError::MissingAmount);
        assert_eq!(error("deposit,1,1,"), RowError::MissingAmount);
        assert_eq!(error("withdrawal,1,1,  "), RowError::MissingAmount);
        assert_eq!(error(r#"withdrawal,1,1,"""#), RowError::MissingAmount);
    }

    #[test]
    fn disputes_reject_non_empty_amount() {
        assert_eq!(error("dispute,1,1,1"), RowError::UnexpectedAmount);
        assert_eq!(error("resolve,1,1,0"), RowError::UnexpectedAmount);
        assert_eq!(error("chargeback,1,1,x"), RowError::UnexpectedAmount);
        assert_eq!(one("dispute,1,1,  "), ok(EventKind::Dispute, 1, 1, None));
    }

    #[test]
    fn rejects_non_positive_amount() {
        let not_positive = RowError::Amount(DomainError::AmountNotPositive);
        assert_eq!(error("deposit,1,1,0"), not_positive);
        assert_eq!(error("deposit,1,1,0.0000"), not_positive);
        assert_eq!(error("withdrawal,1,1,-1"), not_positive);
    }

    #[test]
    fn rejects_more_than_four_places() {
        let too_precise = RowError::Amount(DomainError::AmountTooPrecise(Amount::MAX_SCALE));
        assert_eq!(error("deposit,1,1,1.00005"), too_precise);
        assert_eq!(amount_of("deposit,1,1,1.50000"), Some(dec!(1.5)));
    }

    #[test]
    fn rejects_unparsable_amount() {
        for amount in ["abc", "1.2.3", "1e3", "0x10", "1 000", "NaN", "--1"] {
            assert_eq!(
                error(&format!("deposit,1,1,{amount}")),
                RowError::InvalidAmount,
                "amount {amount:?}"
            );
        }
    }

    #[test]
    fn rejects_amount_that_would_be_rounded() {
        // More digits than Decimal holds: from_str would round these.
        assert_eq!(
            error("deposit,1,1,1.00000000000000000000000000001"),
            RowError::InvalidAmount
        );
        assert_eq!(
            error("deposit,1,1,99999999999999999999999999999"),
            RowError::InvalidAmount
        );
    }

    #[test]
    fn accepts_library_decimal_grammar() {
        assert_eq!(amount_of("deposit,1,1,+5"), Some(dec!(5)));
        assert_eq!(amount_of("deposit,1,1,1_000"), Some(dec!(1000)));
        assert_eq!(amount_of("deposit,1,1,.5"), Some(dec!(0.5)));
        assert_eq!(amount_of("deposit,1,1,5."), Some(dec!(5)));
        assert_eq!(amount_of("deposit,1,1,007.25"), Some(dec!(7.25)));
    }

    // --- Lines, header, blank lines ---

    #[test]
    fn skips_header_in_any_case_with_or_without_amount() {
        for header in [
            "type,client,tx,amount",
            "TYPE, Client ,tx , AMOUNT",
            "type,client,tx",
            r#""type","client","tx","amount""#,
        ] {
            let input = format!("{header}\ndeposit,1,1,1\n");
            assert_eq!(
                read(input.as_bytes()),
                vec![(2, ok(EventKind::Deposit, 1, 1, Some(dec!(1))))],
                "header {header:?}"
            );
        }
    }

    #[test]
    fn header_is_optional() {
        assert_eq!(
            read(b"deposit,1,1,1\n"),
            vec![(1, ok(EventKind::Deposit, 1, 1, Some(dec!(1))))]
        );
    }

    #[test]
    fn header_is_looked_for_only_in_first_non_empty_line() {
        assert_eq!(
            read(b"\n  \ntype,client,tx,amount\ntype,client,tx,amount\n"),
            vec![(4, err(None, None, RowError::UnknownType))]
        );
        assert_eq!(
            read(b"deposit,1,1,1\ntype,client,tx,amount\n"),
            vec![
                (1, ok(EventKind::Deposit, 1, 1, Some(dec!(1)))),
                (2, err(None, None, RowError::UnknownType)),
            ]
        );
    }

    #[test]
    fn near_header_is_a_data_row() {
        assert_eq!(error("type,client,tx,amt"), RowError::UnknownType);
        assert_eq!(error("type,client"), RowError::FieldCount(2));
        assert_eq!(error("client,type,tx,amount"), RowError::UnknownType);
    }

    #[test]
    fn invalid_utf8_first_line_is_not_a_header() {
        assert_eq!(
            read(b"\xFF\ntype,client,tx,amount\n"),
            vec![
                (1, err(None, None, RowError::InvalidUtf8)),
                (2, err(None, None, RowError::UnknownType)),
            ]
        );
    }

    #[test]
    fn skips_blank_lines_and_counts_physical_lines() {
        assert_eq!(
            read(b"type,client,tx,amount\n\n   \n\t\ndeposit,1,1,1\n\n"),
            vec![(5, ok(EventKind::Deposit, 1, 1, Some(dec!(1))))]
        );
    }

    #[test]
    fn handles_crlf_line_endings() {
        assert_eq!(
            read(b"type,client,tx,amount\r\n\r\ndeposit,1,1,1\r\ndispute,1,1\r\n"),
            vec![
                (3, ok(EventKind::Deposit, 1, 1, Some(dec!(1)))),
                (4, ok(EventKind::Dispute, 1, 1, None)),
            ]
        );
    }

    #[test]
    fn last_line_without_newline_is_read() {
        assert_eq!(
            read(b"deposit,1,1,1\ndeposit,1,2,2"),
            vec![
                (1, ok(EventKind::Deposit, 1, 1, Some(dec!(1)))),
                (2, ok(EventKind::Deposit, 1, 2, Some(dec!(2)))),
            ]
        );
    }

    #[test]
    fn ignores_bom_before_header_or_data() {
        assert_eq!(
            read(b"\xEF\xBB\xBFtype,client,tx,amount\ndeposit,1,1,1\n"),
            vec![(2, ok(EventKind::Deposit, 1, 1, Some(dec!(1))))]
        );
        assert_eq!(
            read(b"\xEF\xBB\xBFdeposit,1,1,1\n"),
            vec![(1, ok(EventKind::Deposit, 1, 1, Some(dec!(1))))]
        );
    }

    #[test]
    fn bom_is_only_ignored_at_start_of_file() {
        assert_eq!(
            read(b"deposit,1,1,1\n\xEF\xBB\xBFdeposit,1,2,1\n"),
            vec![
                (1, ok(EventKind::Deposit, 1, 1, Some(dec!(1)))),
                (2, err(Some(1), Some(2), RowError::UnknownType)),
            ]
        );
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert_eq!(read(b""), vec![]);
        assert_eq!(read(b"type,client,tx,amount\n"), vec![]);
        assert_eq!(read(b"\xEF\xBB\xBF"), vec![]);
    }

    // --- I/O errors ---

    #[test]
    fn read_error_is_yielded_as_fatal() {
        let input = io::Read::chain(&b"deposit,1,1,1\n"[..], FailingReader);
        let mut reader = CsvTransactionReader::new(io::BufReader::new(input));

        assert!(reader.next().unwrap().is_ok());
        let error = reader.next().unwrap().unwrap_err();
        assert_eq!(error.to_string(), "disk failure");
    }

    struct FailingReader;

    impl io::Read for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("disk failure"))
        }
    }

    // --- Streaming ---

    /// An endless source: the reader must yield rows without reading to the end.
    struct Endless;

    impl io::Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let line = b"deposit,1,1,1\n";
            let n = buf.len().min(line.len());
            buf[..n].copy_from_slice(&line[..n]);
            Ok(n)
        }
    }

    #[test]
    fn reads_lazily_one_line_at_a_time() {
        let rows: Vec<_> = CsvTransactionReader::new(io::BufReader::new(Endless))
            .take(3)
            .map(|row| row.unwrap().line)
            .collect();
        assert_eq!(rows, vec![1, 2, 3]);
    }

    // --- Together with the application layer ---

    #[test]
    fn rows_feed_process() {
        use crate::application::{PaymentEngine, process};

        let input =
            b"type,client,tx,amount\ndeposit,1,1,10\nwithdrawal,1,2,4\ndispute,1,1,\nfoo,1,3,1\n";
        let mut engine = PaymentEngine::new();
        let mut rejected = Vec::new();
        let summary = process(
            &mut engine,
            CsvTransactionReader::new(&input[..]),
            |rejection| rejected.push((rejection.line, rejection.reason.to_string())),
            |_| {},
        )
        .unwrap();

        assert_eq!(summary.read, 4);
        assert_eq!(summary.accepted, 3);
        assert_eq!(rejected, vec![(5, "unknown type".to_string())]);

        let account = engine.accounts().next().unwrap();
        assert_eq!(account.client_id(), ClientId::from(1));
        assert_eq!(account.available(), dec!(-4));
        assert_eq!(account.held(), dec!(10));
    }
}
