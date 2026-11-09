use rust_decimal::Decimal;

use crate::domain::{error::DomainError, money::Amount};

use super::ids::ClientId;

/// A client account. `total` is not stored, it is always `available + held`.
#[derive(Debug)]
pub struct Account {
    client_id: ClientId,
    available: Decimal,
    held: Decimal,
    locked: bool,
}

impl Account {
    pub fn new(client_id: ClientId) -> Self {
        Self {
            client_id,
            available: Decimal::ZERO,
            held: Decimal::ZERO,
            locked: false,
        }
    }

    pub fn client_id(&self) -> ClientId {
        self.client_id
    }

    pub fn available(&self) -> Decimal {
        self.available
    }

    pub fn held(&self) -> Decimal {
        self.held
    }

    pub fn total(&self) -> Decimal {
        self.available + self.held
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Refused on a locked account. The new total is checked too, so `total()`
    /// cannot overflow later.
    pub fn deposit(&mut self, amount: Amount) -> Result<(), DomainError> {
        self.ensure_unlocked()?;
        let available = self
            .available
            .checked_add(amount.into())
            .and_then(|a| a.checked_add(self.held()).map(|_| a))
            .ok_or(DomainError::Overflow)?;

        self.available = available;

        Ok(())
    }

    /// Refused on a locked account or when available funds are short.
    pub fn withdraw(&mut self, amount: Amount) -> Result<(), DomainError> {
        self.ensure_unlocked()?;

        if self.available < amount.into() {
            return Err(DomainError::InsufficientFunds);
        }

        self.available -= Decimal::from(amount);

        Ok(())
    }

    /// Dispute: moves funds from available to held. Works on a locked account,
    /// and available may go negative if the money is already gone.
    pub fn hold(&mut self, amount: Amount) -> Result<(), DomainError> {
        let available = self
            .available
            .checked_sub(amount.into())
            .ok_or(DomainError::Overflow)?;
        let held = self
            .held
            .checked_add(amount.into())
            .ok_or(DomainError::Overflow)?;

        self.available = available;
        self.held = held;
        Ok(())
    }

    /// Resolve: moves held funds back to available.
    pub fn release(&mut self, amount: Amount) -> Result<(), DomainError> {
        let available = self
            .available
            .checked_add(amount.into())
            .ok_or(DomainError::Overflow)?;
        let held = self
            .held
            .checked_sub(amount.into())
            .ok_or(DomainError::Overflow)?;

        self.available = available;
        self.held = held;
        Ok(())
    }

    /// Chargeback: removes held funds and locks the account.
    pub fn charge_back(&mut self, amount: Amount) -> Result<(), DomainError> {
        let held = self
            .held
            .checked_sub(amount.into())
            .ok_or(DomainError::Overflow)?;

        self.held = held;
        self.locked = true;

        Ok(())
    }

    fn ensure_unlocked(&self) -> Result<(), DomainError> {
        if self.locked {
            Err(DomainError::AccountLocked)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn amount(value: Decimal) -> Amount {
        Amount::try_from(value).unwrap()
    }

    fn account_with(available: Decimal) -> Account {
        let mut account = Account::new(ClientId::from(1));
        account.deposit(amount(available)).unwrap();
        account
    }

    fn assert_balances(account: &Account, available: Decimal, held: Decimal) {
        assert_eq!(account.available(), available);
        assert_eq!(account.held(), held);
        assert_eq!(account.total(), available + held);
    }

    #[test]
    fn new_account_is_empty_and_unlocked() {
        let account = Account::new(ClientId::from(7));
        assert_eq!(account.client_id(), ClientId::from(7));
        assert_balances(&account, dec!(0), dec!(0));
        assert!(!account.is_locked());
    }

    #[test]
    fn deposit_increases_available_and_total() {
        let mut account = account_with(dec!(1.5));
        account.deposit(amount(dec!(2.0001))).unwrap();
        assert_balances(&account, dec!(3.5001), dec!(0));
    }

    #[test]
    fn deposit_overflowing_available_fails_and_changes_nothing() {
        let mut account = account_with(Decimal::MAX);
        assert_eq!(account.deposit(amount(dec!(1))), Err(DomainError::Overflow));
        assert_balances(&account, Decimal::MAX, dec!(0));
    }

    #[test]
    fn deposit_overflowing_total_fails_and_changes_nothing() {
        let mut account = account_with(Decimal::MAX - dec!(10));
        account.hold(amount(dec!(5))).unwrap();
        let available = account.available();

        assert_eq!(
            account.deposit(amount(dec!(12))),
            Err(DomainError::Overflow)
        );
        assert_balances(&account, available, dec!(5));
    }

    #[test]
    fn withdraw_decreases_available_and_total() {
        let mut account = account_with(dec!(10));
        account.withdraw(amount(dec!(3.25))).unwrap();
        assert_balances(&account, dec!(6.75), dec!(0));
    }

    #[test]
    fn withdraw_of_entire_balance_is_allowed() {
        let mut account = account_with(dec!(10));
        account.withdraw(amount(dec!(10))).unwrap();
        assert_balances(&account, dec!(0), dec!(0));
    }

    #[test]
    fn withdraw_with_insufficient_funds_fails_and_changes_nothing() {
        let mut account = account_with(dec!(1));
        assert_eq!(
            account.withdraw(amount(dec!(1.0001))),
            Err(DomainError::InsufficientFunds)
        );
        assert_balances(&account, dec!(1), dec!(0));
    }

    #[test]
    fn withdraw_from_negative_available_fails() {
        let mut account = account_with(dec!(10));
        account.withdraw(amount(dec!(8))).unwrap();
        account.hold(amount(dec!(10))).unwrap();
        assert_eq!(
            account.withdraw(amount(dec!(0.0001))),
            Err(DomainError::InsufficientFunds)
        );
        assert_balances(&account, dec!(-8), dec!(10));
    }

    #[test]
    fn hold_moves_funds_from_available_to_held() {
        let mut account = account_with(dec!(10));
        account.hold(amount(dec!(4))).unwrap();
        assert_balances(&account, dec!(6), dec!(4));
    }

    #[test]
    fn hold_may_make_available_negative() {
        let mut account = account_with(dec!(10));
        account.withdraw(amount(dec!(8))).unwrap();
        account.hold(amount(dec!(10))).unwrap();
        assert_balances(&account, dec!(-8), dec!(10));
    }

    #[test]
    fn release_moves_funds_from_held_back_to_available() {
        let mut account = account_with(dec!(10));
        account.hold(amount(dec!(4))).unwrap();
        account.release(amount(dec!(4))).unwrap();
        assert_balances(&account, dec!(10), dec!(0));
    }

    #[test]
    fn charge_back_removes_held_funds_and_locks() {
        let mut account = account_with(dec!(10));
        account.hold(amount(dec!(4))).unwrap();
        account.charge_back(amount(dec!(4))).unwrap();
        assert_balances(&account, dec!(6), dec!(0));
        assert!(account.is_locked());
    }

    #[test]
    fn locked_account_rejects_deposit_and_withdraw() {
        let mut account = account_with(dec!(10));
        account.hold(amount(dec!(4))).unwrap();
        account.charge_back(amount(dec!(4))).unwrap();

        assert_eq!(
            account.deposit(amount(dec!(1))),
            Err(DomainError::AccountLocked)
        );
        assert_eq!(
            account.withdraw(amount(dec!(1))),
            Err(DomainError::AccountLocked)
        );
        assert_balances(&account, dec!(6), dec!(0));
    }
}
