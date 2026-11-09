//! Account output as CSV (spec 6).

use std::{
    fmt,
    io::{self, BufWriter, Write},
};

use rust_decimal::Decimal;

use crate::domain::Account;

const HEADER: &str = "client,available,held,total,locked";

/// Writes the header and one line per account, in the iterator's order.
pub fn write_accounts<'a, W: Write>(
    out: W,
    accounts: impl IntoIterator<Item = &'a Account>,
) -> io::Result<()> {
    let mut out = BufWriter::new(out);
    writeln!(out, "{HEADER}")?;

    for account in accounts {
        writeln!(
            out,
            "{},{},{},{},{}",
            account.client_id(),
            Fixed4(account.available()),
            Fixed4(account.held()),
            Fixed4(account.total()),
            account.is_locked()
        )?;
    }

    out.flush()
}

/// An amount with exactly 4 decimal places. Zero is normalised first so that
/// `-0.0000` never shows up. Amounts never have more than 4 places, so
/// nothing gets rounded.
struct Fixed4(Decimal);

impl fmt::Display for Fixed4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        let value = if self.0.is_zero() {
            Decimal::ZERO
        } else {
            self.0
        };
        write!(f, "{value:.4}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Amount, ClientId};
    use rust_decimal_macros::dec;

    fn amount(value: Decimal) -> Amount {
        Amount::try_from(value).unwrap()
    }

    fn render(accounts: &[Account]) -> String {
        let mut out = Vec::new();
        write_accounts(&mut out, accounts).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn writes_header_only_when_there_are_no_accounts() {
        assert_eq!(render(&[]), "client,available,held,total,locked\n");
    }

    #[test]
    fn writes_amounts_with_exactly_four_places() {
        let mut account = Account::new(ClientId::from(7));
        account.deposit(amount(dec!(1.5))).unwrap();
        account.deposit(amount(dec!(2))).unwrap();
        account.hold(amount(dec!(0.0001))).unwrap();

        assert_eq!(
            render(&[account]),
            "client,available,held,total,locked\n7,3.4999,0.0001,3.5000,false\n"
        );
    }

    #[test]
    fn writes_locked_flag() {
        let mut account = Account::new(ClientId::from(1));
        account.deposit(amount(dec!(1))).unwrap();
        account.hold(amount(dec!(1))).unwrap();
        account.charge_back(amount(dec!(1))).unwrap();

        assert_eq!(
            render(&[account]),
            "client,available,held,total,locked\n1,0.0000,0.0000,0.0000,true\n"
        );
    }

    #[test]
    fn writes_negative_available() {
        let mut account = Account::new(ClientId::from(2));
        account.deposit(amount(dec!(10))).unwrap();
        account.withdraw(amount(dec!(4))).unwrap();
        account.hold(amount(dec!(10))).unwrap();

        assert_eq!(
            render(&[account]),
            "client,available,held,total,locked\n2,-4.0000,10.0000,6.0000,false\n"
        );
    }

    #[test]
    fn negative_zero_is_printed_as_zero() {
        let negative_zero = -dec!(0.0000);
        assert!(negative_zero.is_sign_negative());
        assert_eq!(Fixed4(negative_zero).to_string(), "0.0000");
    }

    #[test]
    fn writes_one_row_per_account() {
        let accounts: Vec<_> = (1..=3).map(|id| Account::new(ClientId::from(id))).collect();
        assert_eq!(render(&accounts).lines().count(), 4);
    }

    #[test]
    fn writes_accounts_straight_from_the_engine() {
        use crate::{
            application::PaymentEngine,
            domain::{Event, EventKind, TxId},
        };

        let mut engine = PaymentEngine::new();
        engine
            .apply(Event {
                client: ClientId::from(5),
                tx: TxId::from(1),
                kind: EventKind::Deposit,
                amount: Some(amount(dec!(2.5))),
            })
            .unwrap();

        let mut out = Vec::new();
        write_accounts(&mut out, engine.accounts()).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "client,available,held,total,locked\n5,2.5000,0.0000,2.5000,false\n"
        );
    }

    /// Accepts `limit` bytes, then fails, like a disk that fills up.
    struct FillsUp {
        limit: usize,
    }

    impl Write for FillsUp {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.limit == 0 {
                return Err(io::Error::other("disk full"));
            }
            let n = buf.len().min(self.limit);
            self.limit -= n;
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_error_is_returned() {
        let accounts = [Account::new(ClientId::from(1))];
        let error = write_accounts(FillsUp { limit: 0 }, &accounts).unwrap_err();
        assert_eq!(error.to_string(), "disk full");
    }

    #[test]
    fn write_error_in_the_middle_of_output_is_returned() {
        // More rows than BufWriter holds, so the error surfaces while rows are written.
        let accounts: Vec<_> = (1..=1000)
            .map(|id| Account::new(ClientId::from(id)))
            .collect();
        let error = write_accounts(FillsUp { limit: 100 }, &accounts).unwrap_err();
        assert_eq!(error.to_string(), "disk full");
    }
}
