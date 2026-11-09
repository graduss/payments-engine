use std::collections::{HashMap, hash_map::Entry};

use crate::{
    application::{error::ApplicationError, used_tx::UsedTxIds},
    domain::{
        Account, ClientId, Deposit, DomainError, Event, EventKind, Transaction, TxId, Withdrawal,
    },
};

/// Applies events to client accounts. Use one engine per event stream.
///
/// Only what disputes need is kept in memory: the accounts, processed deposits
/// and the ids of all other used transactions.
#[derive(Debug, Default)]
pub struct PaymentEngine {
    accounts: HashMap<ClientId, Account>,
    deposits: HashMap<TxId, Transaction<Deposit>>,
    used: UsedTxIds,
}

impl PaymentEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one event. A rejected event changes no balance, but a deposit or
    /// withdrawal that passed the duplicate check still uses up its tx id.
    pub fn apply(&mut self, event: Event) -> Result<(), ApplicationError> {
        match event.kind {
            EventKind::Deposit => self.deposit(event),
            EventKind::Withdrawal => self.withdraw(event),
            EventKind::Dispute | EventKind::Resolve | EventKind::Chargeback => self.settle(event),
        }
    }

    /// All accounts, in no particular order.
    pub fn accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.values()
    }

    fn deposit(&mut self, event: Event) -> Result<(), ApplicationError> {
        let deposit = Transaction::<Deposit>::try_from(event)?;
        self.ensure_unused(&deposit.tx_id())?;

        match self.accounts.entry(deposit.client_id()) {
            Entry::Occupied(mut entry) => entry.get_mut().deposit(deposit.amount()),
            Entry::Vacant(entry) => {
                let mut account = Account::new(deposit.client_id());
                account.deposit(deposit.amount()).map(|_| {
                    entry.insert(account);
                })
            }
        }
        .map_err(|err| {
            // A rejected deposit still uses up its tx id (spec 5.3).
            self.used.insert(deposit.tx_id());
            ApplicationError::from(err)
        })?;

        self.deposits.insert(deposit.tx_id(), deposit);
        Ok(())
    }

    fn withdraw(&mut self, event: Event) -> Result<(), ApplicationError> {
        let withdrawal = Transaction::<Withdrawal>::try_from(event)?;
        self.ensure_unused(&withdrawal.tx_id())?;
        // The tx id is used up once it passes the duplicate check, whatever
        // the outcome (spec 5.3).
        self.used.insert(withdrawal.tx_id());

        self.accounts
            .get_mut(&withdrawal.client_id())
            .ok_or(ApplicationError::AccountNotFound)?
            .withdraw(withdrawal.amount())?;

        Ok(())
    }

    fn settle(&mut self, event: Event) -> Result<(), ApplicationError> {
        let account = self
            .accounts
            .get_mut(&event.client)
            .ok_or(ApplicationError::AccountNotFound)?;
        let deposit = self
            .deposits
            .get_mut(&event.tx)
            .ok_or(ApplicationError::NotADeposit)?;
        deposit.ensure_owned_by(&event)?;

        match event.kind {
            EventKind::Dispute => deposit.dispute(account),
            EventKind::Resolve => deposit.resolve(account),
            EventKind::Chargeback => deposit.charge_back(account),
            // apply() never sends these here.
            EventKind::Deposit | EventKind::Withdrawal => Err(DomainError::UnsupportedEvent),
        }
        .map_err(|e| e.into())
    }

    fn ensure_unused(&self, tx: &TxId) -> Result<(), ApplicationError> {
        if self.deposits.contains_key(tx) || self.used.contains(tx) {
            Err(ApplicationError::DuplicateTx)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Amount;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    fn event(kind: EventKind, client: u16, tx: u32, amount: Option<Decimal>) -> Event {
        Event {
            client: ClientId::from(client),
            tx: TxId::from(tx),
            kind,
            amount: amount.map(|value| Amount::try_from(value).unwrap()),
        }
    }

    fn deposit(client: u16, tx: u32, amount: Decimal) -> Event {
        event(EventKind::Deposit, client, tx, Some(amount))
    }

    fn withdrawal(client: u16, tx: u32, amount: Decimal) -> Event {
        event(EventKind::Withdrawal, client, tx, Some(amount))
    }

    fn dispute(client: u16, tx: u32) -> Event {
        event(EventKind::Dispute, client, tx, None)
    }

    fn resolve(client: u16, tx: u32) -> Event {
        event(EventKind::Resolve, client, tx, None)
    }

    fn chargeback(client: u16, tx: u32) -> Event {
        event(EventKind::Chargeback, client, tx, None)
    }

    fn engine_with(events: impl IntoIterator<Item = Event>) -> PaymentEngine {
        let mut engine = PaymentEngine::new();
        for event in events {
            engine.apply(event).unwrap();
        }
        engine
    }

    fn account(engine: &PaymentEngine, client: u16) -> Option<&Account> {
        engine
            .accounts()
            .find(|account| account.client_id() == ClientId::from(client))
    }

    /// Asserts `available`, `held`, `locked` and `total = available + held`.
    fn assert_account(
        engine: &PaymentEngine,
        client: u16,
        available: Decimal,
        held: Decimal,
        locked: bool,
    ) {
        let account = account(engine, client).expect("account exists");
        assert_eq!(account.available(), available, "available");
        assert_eq!(account.held(), held, "held");
        assert_eq!(account.total(), available + held, "total");
        assert_eq!(account.is_locked(), locked, "locked");
    }

    fn domain(error: DomainError) -> ApplicationError {
        ApplicationError::Domain(error)
    }

    // --- deposit ---

    #[test]
    fn new_engine_has_no_accounts() {
        assert_eq!(PaymentEngine::new().accounts().count(), 0);
    }

    #[test]
    fn first_deposit_creates_account() {
        let engine = engine_with([deposit(1, 1, dec!(1.5))]);
        assert_eq!(engine.accounts().count(), 1);
        assert_account(&engine, 1, dec!(1.5), dec!(0), false);
    }

    #[test]
    fn deposits_accumulate_per_client() {
        let engine = engine_with([
            deposit(1, 1, dec!(1)),
            deposit(2, 2, dec!(2)),
            deposit(1, 3, dec!(0.0001)),
        ]);
        assert_eq!(engine.accounts().count(), 2);
        assert_account(&engine, 1, dec!(1.0001), dec!(0), false);
        assert_account(&engine, 2, dec!(2), dec!(0), false);
    }

    #[test]
    fn duplicate_deposit_tx_is_rejected_and_changes_nothing() {
        let mut engine = engine_with([deposit(1, 1, dec!(5))]);
        assert_eq!(
            engine.apply(deposit(1, 1, dec!(7))),
            Err(ApplicationError::DuplicateTx)
        );
        assert_eq!(
            engine.apply(deposit(2, 1, dec!(7))),
            Err(ApplicationError::DuplicateTx)
        );
        assert_account(&engine, 1, dec!(5), dec!(0), false);
        assert!(account(&engine, 2).is_none());
    }

    #[test]
    fn deposit_without_amount_is_rejected_and_does_not_use_tx() {
        let mut engine = PaymentEngine::new();
        assert_eq!(
            engine.apply(event(EventKind::Deposit, 1, 1, None)),
            Err(domain(DomainError::UnsupportedEvent))
        );
        assert_eq!(engine.accounts().count(), 0);

        // A corrected row with the same tx is accepted later (spec 5.3).
        engine.apply(deposit(1, 1, dec!(3))).unwrap();
        assert_account(&engine, 1, dec!(3), dec!(0), false);
    }

    #[test]
    fn deposit_overflow_does_not_create_account_but_uses_tx() {
        let mut engine = engine_with([deposit(1, 1, Decimal::MAX)]);
        assert_eq!(
            engine.apply(deposit(1, 2, dec!(1))),
            Err(domain(DomainError::Overflow))
        );
        assert_account(&engine, 1, Decimal::MAX, dec!(0), false);

        // Tx 2 is used up, and as a rejected deposit it cannot be disputed.
        assert_eq!(
            engine.apply(deposit(1, 2, dec!(1))),
            Err(ApplicationError::DuplicateTx)
        );
        assert_eq!(
            engine.apply(dispute(1, 2)),
            Err(ApplicationError::NotADeposit)
        );
    }

    // --- withdrawal ---

    #[test]
    fn withdrawal_decreases_available() {
        let engine = engine_with([deposit(1, 1, dec!(10)), withdrawal(1, 2, dec!(3.5))]);
        assert_account(&engine, 1, dec!(6.5), dec!(0), false);
    }

    #[test]
    fn withdrawal_of_whole_balance_is_allowed() {
        let engine = engine_with([deposit(1, 1, dec!(10)), withdrawal(1, 2, dec!(10))]);
        assert_account(&engine, 1, dec!(0), dec!(0), false);
    }

    #[test]
    fn withdrawal_from_unknown_account_is_rejected_and_creates_nothing() {
        let mut engine = PaymentEngine::new();
        assert_eq!(
            engine.apply(withdrawal(1, 1, dec!(1))),
            Err(ApplicationError::AccountNotFound)
        );
        assert_eq!(engine.accounts().count(), 0);
    }

    #[test]
    fn withdrawal_with_insufficient_funds_is_rejected() {
        let mut engine = engine_with([deposit(1, 1, dec!(1))]);
        assert_eq!(
            engine.apply(withdrawal(1, 2, dec!(1.0001))),
            Err(domain(DomainError::InsufficientFunds))
        );
        assert_account(&engine, 1, dec!(1), dec!(0), false);
    }

    #[test]
    fn rejected_withdrawal_still_uses_its_tx() {
        let mut engine = engine_with([deposit(1, 1, dec!(1))]);
        // Insufficient funds and unknown account: both use up the tx.
        engine.apply(withdrawal(1, 2, dec!(5))).unwrap_err();
        engine.apply(withdrawal(9, 3, dec!(5))).unwrap_err();

        for tx in [2, 3] {
            assert_eq!(
                engine.apply(deposit(1, tx, dec!(1))),
                Err(ApplicationError::DuplicateTx)
            );
            assert_eq!(
                engine.apply(withdrawal(1, tx, dec!(1))),
                Err(ApplicationError::DuplicateTx)
            );
        }
        assert_account(&engine, 1, dec!(1), dec!(0), false);
    }

    #[test]
    fn withdrawal_tx_ids_are_unique_across_types() {
        let mut engine = engine_with([deposit(1, 1, dec!(10)), withdrawal(1, 2, dec!(1))]);
        assert_eq!(
            engine.apply(withdrawal(1, 1, dec!(1))),
            Err(ApplicationError::DuplicateTx)
        );
        assert_eq!(
            engine.apply(deposit(1, 2, dec!(1))),
            Err(ApplicationError::DuplicateTx)
        );
        assert_account(&engine, 1, dec!(9), dec!(0), false);
    }

    #[test]
    fn duplicate_check_comes_before_account_check() {
        let mut engine = engine_with([deposit(1, 1, dec!(10))]);
        // Client 2 has no account, but the duplicate tx is reported first.
        assert_eq!(
            engine.apply(withdrawal(2, 1, dec!(1))),
            Err(ApplicationError::DuplicateTx)
        );
    }

    // --- dispute ---

    #[test]
    fn dispute_moves_deposit_to_held() {
        let engine = engine_with([
            deposit(1, 1, dec!(10)),
            deposit(1, 2, dec!(4)),
            dispute(1, 2),
        ]);
        assert_account(&engine, 1, dec!(10), dec!(4), false);
    }

    #[test]
    fn dispute_after_withdrawal_makes_available_negative() {
        let engine = engine_with([
            deposit(1, 1, dec!(10)),
            withdrawal(1, 2, dec!(8)),
            dispute(1, 1),
        ]);
        assert_account(&engine, 1, dec!(-8), dec!(10), false);
    }

    #[test]
    fn dispute_of_unknown_tx_is_rejected() {
        let mut engine = engine_with([deposit(1, 1, dec!(10))]);
        assert_eq!(
            engine.apply(dispute(1, 99)),
            Err(ApplicationError::NotADeposit)
        );
        assert_account(&engine, 1, dec!(10), dec!(0), false);
    }

    #[test]
    fn dispute_of_withdrawal_is_rejected() {
        let mut engine = engine_with([deposit(1, 1, dec!(10)), withdrawal(1, 2, dec!(3))]);
        assert_eq!(
            engine.apply(dispute(1, 2)),
            Err(ApplicationError::NotADeposit)
        );
        assert_account(&engine, 1, dec!(7), dec!(0), false);
    }

    #[test]
    fn dispute_from_client_without_account_is_rejected_first() {
        let mut engine = engine_with([deposit(1, 1, dec!(10))]);
        assert_eq!(
            engine.apply(dispute(2, 1)),
            Err(ApplicationError::AccountNotFound)
        );
        // Unknown client and unknown tx: the account check wins (spec 10.3).
        assert_eq!(
            engine.apply(dispute(2, 99)),
            Err(ApplicationError::AccountNotFound)
        );
        assert!(account(&engine, 2).is_none());
        assert_account(&engine, 1, dec!(10), dec!(0), false);
    }

    #[test]
    fn dispute_by_another_client_is_rejected_and_changes_no_account() {
        let mut engine = engine_with([deposit(1, 1, dec!(10)), deposit(2, 2, dec!(5))]);
        assert_eq!(
            engine.apply(dispute(2, 1)),
            Err(domain(DomainError::ClientMismatch))
        );
        assert_account(&engine, 1, dec!(10), dec!(0), false);
        assert_account(&engine, 2, dec!(5), dec!(0), false);

        // The deposit is still undisputed, so its owner can dispute it.
        engine.apply(dispute(1, 1)).unwrap();
        assert_account(&engine, 1, dec!(0), dec!(10), false);
    }

    #[test]
    fn second_dispute_while_open_is_rejected() {
        let mut engine = engine_with([deposit(1, 1, dec!(10)), dispute(1, 1)]);
        assert_eq!(
            engine.apply(dispute(1, 1)),
            Err(domain(DomainError::DisputeAlreadyOpen))
        );
        assert_account(&engine, 1, dec!(0), dec!(10), false);
    }

    // --- resolve ---

    #[test]
    fn resolve_releases_held_funds() {
        let engine = engine_with([deposit(1, 1, dec!(10)), dispute(1, 1), resolve(1, 1)]);
        assert_account(&engine, 1, dec!(10), dec!(0), false);
    }

    #[test]
    fn resolve_without_dispute_is_rejected() {
        let mut engine = engine_with([deposit(1, 1, dec!(10))]);
        assert_eq!(
            engine.apply(resolve(1, 1)),
            Err(domain(DomainError::NotDisputed))
        );
        assert_account(&engine, 1, dec!(10), dec!(0), false);
    }

    #[test]
    fn resolve_of_unknown_tx_is_rejected() {
        let mut engine = engine_with([deposit(1, 1, dec!(10)), dispute(1, 1)]);
        assert_eq!(
            engine.apply(resolve(1, 99)),
            Err(ApplicationError::NotADeposit)
        );
        assert_account(&engine, 1, dec!(0), dec!(10), false);
    }

    #[test]
    fn resolve_by_another_client_is_rejected() {
        let mut engine = engine_with([
            deposit(1, 1, dec!(10)),
            deposit(2, 2, dec!(1)),
            dispute(1, 1),
        ]);
        assert_eq!(
            engine.apply(resolve(2, 1)),
            Err(domain(DomainError::ClientMismatch))
        );
        assert_account(&engine, 1, dec!(0), dec!(10), false);
        assert_account(&engine, 2, dec!(1), dec!(0), false);
    }

    #[test]
    fn deposit_can_be_disputed_again_after_resolve() {
        let engine = engine_with([
            deposit(1, 1, dec!(10)),
            dispute(1, 1),
            resolve(1, 1),
            dispute(1, 1),
        ]);
        assert_account(&engine, 1, dec!(0), dec!(10), false);
    }

    // --- chargeback ---

    #[test]
    fn chargeback_removes_held_funds_and_locks_account() {
        let engine = engine_with([
            deposit(1, 1, dec!(10)),
            deposit(1, 2, dec!(4)),
            dispute(1, 2),
            chargeback(1, 2),
        ]);
        assert_account(&engine, 1, dec!(10), dec!(0), true);
    }

    #[test]
    fn chargeback_without_dispute_is_rejected() {
        let mut engine = engine_with([deposit(1, 1, dec!(10))]);
        assert_eq!(
            engine.apply(chargeback(1, 1)),
            Err(domain(DomainError::NotDisputed))
        );
        assert_account(&engine, 1, dec!(10), dec!(0), false);
    }

    #[test]
    fn chargeback_after_resolve_is_rejected() {
        let mut engine = engine_with([deposit(1, 1, dec!(10)), dispute(1, 1), resolve(1, 1)]);
        assert_eq!(
            engine.apply(chargeback(1, 1)),
            Err(domain(DomainError::NotDisputed))
        );
        assert_account(&engine, 1, dec!(10), dec!(0), false);
    }

    #[test]
    fn chargeback_by_another_client_is_rejected() {
        let mut engine = engine_with([
            deposit(1, 1, dec!(10)),
            deposit(2, 2, dec!(1)),
            dispute(1, 1),
        ]);
        assert_eq!(
            engine.apply(chargeback(2, 1)),
            Err(domain(DomainError::ClientMismatch))
        );
        assert_account(&engine, 1, dec!(0), dec!(10), false);
        assert_account(&engine, 2, dec!(1), dec!(0), false);
    }

    #[test]
    fn charged_back_deposit_is_final() {
        let mut engine = engine_with([deposit(1, 1, dec!(10)), dispute(1, 1), chargeback(1, 1)]);
        for (event, error) in [
            (dispute(1, 1), DomainError::AlreadyChargedBack),
            (resolve(1, 1), DomainError::AlreadyChargedBack),
            (chargeback(1, 1), DomainError::AlreadyChargedBack),
        ] {
            assert_eq!(engine.apply(event), Err(domain(error)));
        }
        assert_account(&engine, 1, dec!(0), dec!(0), true);
    }

    // --- locked account ---

    fn locked_engine() -> PaymentEngine {
        engine_with([
            deposit(1, 1, dec!(10)),
            deposit(1, 2, dec!(4)),
            dispute(1, 2),
            chargeback(1, 2),
        ])
    }

    #[test]
    fn locked_account_rejects_deposit_and_withdrawal() {
        let mut engine = locked_engine();
        assert_eq!(
            engine.apply(deposit(1, 3, dec!(1))),
            Err(domain(DomainError::AccountLocked))
        );
        assert_eq!(
            engine.apply(withdrawal(1, 4, dec!(1))),
            Err(domain(DomainError::AccountLocked))
        );
        assert_account(&engine, 1, dec!(10), dec!(0), true);

        // Both rejected rows used up their tx ids.
        assert_eq!(
            engine.apply(deposit(2, 3, dec!(1))),
            Err(ApplicationError::DuplicateTx)
        );
        assert_eq!(
            engine.apply(deposit(2, 4, dec!(1))),
            Err(ApplicationError::DuplicateTx)
        );
    }

    #[test]
    fn locked_account_still_handles_disputes_on_earlier_deposits() {
        let mut engine = locked_engine();
        engine.apply(dispute(1, 1)).unwrap();
        assert_account(&engine, 1, dec!(0), dec!(10), true);
        engine.apply(resolve(1, 1)).unwrap();
        assert_account(&engine, 1, dec!(10), dec!(0), true);
        engine.apply(dispute(1, 1)).unwrap();
        engine.apply(chargeback(1, 1)).unwrap();
        assert_account(&engine, 1, dec!(0), dec!(0), true);
    }

    #[test]
    fn lock_does_not_affect_other_clients() {
        let mut engine = locked_engine();
        engine.apply(deposit(2, 10, dec!(3))).unwrap();
        engine.apply(withdrawal(2, 11, dec!(1))).unwrap();
        assert_account(&engine, 2, dec!(2), dec!(0), false);
    }
}
