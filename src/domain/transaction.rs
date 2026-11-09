use crate::domain::{
    account::Account,
    error::DomainError,
    ids::{ClientId, TxId},
    money::Amount,
};

/// One parsed input row. `amount` is set only for deposit and withdrawal.
#[derive(Debug)]
pub struct Event {
    pub client: ClientId,
    pub tx: TxId,
    pub kind: EventKind,
    pub amount: Option<Amount>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}

/// A deposit or withdrawal accepted by the engine. Built from an [`Event`]
/// with `TryFrom`, which checks that kind and amount fit.
#[derive(Debug)]
pub struct Transaction<K> {
    client_id: ClientId,
    tx_id: TxId,
    kind: K,
}

impl<K> Transaction<K> {
    pub fn client_id(&self) -> ClientId {
        self.client_id
    }

    pub fn tx_id(&self) -> TxId {
        self.tx_id
    }
}

/// What a processed deposit keeps for later disputes.
#[derive(Debug)]
pub struct Deposit {
    amount: Amount,
    state: DisputeState,
    // Only counted; the number of disputes is not limited.
    dispute_count: u32,
}

/// `Processed` -> `Disputed` -> back to `Processed` (resolve) or `ChargedBack`,
/// which is final.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeState {
    Processed,
    Disputed,
    ChargedBack,
}

impl Transaction<Deposit> {
    pub fn amount(&self) -> Amount {
        self.kind.amount
    }

    pub fn state(&self) -> DisputeState {
        self.kind.state
    }

    pub fn dispute_count(&self) -> u32 {
        self.kind.dispute_count
    }

    /// Holds the deposit amount on the account. Can be repeated after a resolve.
    pub fn dispute(&mut self, account: &mut Account) -> Result<(), DomainError> {
        self.ensure_owned_account(account)?;

        match self.kind.state {
            DisputeState::Processed => {
                account.hold(self.amount())?;
                self.kind.state = DisputeState::Disputed;
                self.kind.dispute_count = self.kind.dispute_count.saturating_add(1);
                Ok(())
            }
            DisputeState::Disputed => Err(DomainError::DisputeAlreadyOpen),
            DisputeState::ChargedBack => Err(DomainError::AlreadyChargedBack),
        }
    }

    /// Releases the held amount and closes the dispute.
    pub fn resolve(&mut self, account: &mut Account) -> Result<(), DomainError> {
        self.ensure_owned_account(account)?;

        match self.kind.state {
            DisputeState::Disputed => {
                account.release(self.amount())?;
                self.kind.state = DisputeState::Processed;
                Ok(())
            }
            DisputeState::Processed => Err(DomainError::NotDisputed),
            DisputeState::ChargedBack => Err(DomainError::AlreadyChargedBack),
        }
    }

    /// Reverses the deposit and locks the account. No way back.
    pub fn charge_back(&mut self, account: &mut Account) -> Result<(), DomainError> {
        self.ensure_owned_account(account)?;

        match self.kind.state {
            DisputeState::Disputed => {
                account.charge_back(self.amount())?;
                self.kind.state = DisputeState::ChargedBack;
                Ok(())
            }
            DisputeState::Processed => Err(DomainError::NotDisputed),
            DisputeState::ChargedBack => Err(DomainError::AlreadyChargedBack),
        }
    }

    /// Checks that `event` refers to this transaction.
    pub fn ensure_transaction_by(&self, event: &Event) -> Result<(), DomainError> {
        if self.tx_id == event.tx {
            Ok(())
        } else {
            Err(DomainError::TransactionMismatch)
        }
    }

    /// Checks that `event` comes from the client who made the deposit.
    pub fn ensure_owned_by(&self, event: &Event) -> Result<(), DomainError> {
        if self.client_id == event.client {
            Ok(())
        } else {
            Err(DomainError::ClientMismatch)
        }
    }

    fn ensure_owned_account(&self, account: &Account) -> Result<(), DomainError> {
        if self.client_id == account.client_id() {
            Ok(())
        } else {
            Err(DomainError::ClientMismatch)
        }
    }
}

impl TryFrom<Event> for Transaction<Deposit> {
    type Error = DomainError;
    fn try_from(event: Event) -> Result<Self, Self::Error> {
        match (event.amount, event.kind) {
            (Some(amount), EventKind::Deposit) => Ok(Self {
                client_id: event.client,
                tx_id: event.tx,
                kind: Deposit {
                    amount,
                    state: DisputeState::Processed,
                    dispute_count: 0,
                },
            }),

            _ => Err(DomainError::UnsupportedEvent),
        }
    }
}

/// A withdrawal. Only its amount matters: withdrawals cannot be disputed.
#[derive(Debug)]
pub struct Withdrawal {
    amount: Amount,
}

impl Transaction<Withdrawal> {
    pub fn amount(&self) -> Amount {
        self.kind.amount
    }
}

impl TryFrom<Event> for Transaction<Withdrawal> {
    type Error = DomainError;

    fn try_from(event: Event) -> Result<Self, Self::Error> {
        match (event.amount, event.kind) {
            (Some(amount), EventKind::Withdrawal) => Ok(Self {
                client_id: event.client,
                tx_id: event.tx,
                kind: Withdrawal { amount },
            }),

            _ => Err(DomainError::UnsupportedEvent),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    const CLIENT: u16 = 1;
    const TX: u32 = 10;

    fn event(kind: EventKind, client: u16, tx: u32, amount: Option<Decimal>) -> Event {
        Event {
            client: ClientId::from(client),
            tx: TxId::from(tx),
            kind,
            amount: amount.map(|value| Amount::try_from(value).unwrap()),
        }
    }

    /// An account of `CLIENT` with one processed deposit `TX` of `amount`.
    fn deposited(amount: Decimal) -> (Transaction<Deposit>, Account) {
        let deposit = event(EventKind::Deposit, CLIENT, TX, Some(amount));
        let mut account = Account::new(ClientId::from(CLIENT));
        account.deposit(deposit.amount.unwrap()).unwrap();
        (Transaction::try_from(deposit).unwrap(), account)
    }

    fn assert_balances(account: &Account, available: Decimal, held: Decimal) {
        assert_eq!(account.available(), available);
        assert_eq!(account.held(), held);
        assert_eq!(account.total(), available + held);
    }

    // --- construction ---

    #[test]
    fn deposit_event_becomes_processed_deposit() {
        let (tx, _) = deposited(dec!(5));
        assert_eq!(tx.client_id(), ClientId::from(CLIENT));
        assert_eq!(tx.tx_id(), TxId::from(TX));
        assert_eq!(Decimal::from(tx.amount()), dec!(5));
        assert_eq!(tx.state(), DisputeState::Processed);
        assert_eq!(tx.dispute_count(), 0);
    }

    #[test]
    fn deposit_requires_deposit_kind_and_amount() {
        let missing_amount = event(EventKind::Deposit, CLIENT, TX, None);
        let wrong_kind = event(EventKind::Withdrawal, CLIENT, TX, Some(dec!(1)));
        assert!(matches!(
            Transaction::<Deposit>::try_from(missing_amount),
            Err(DomainError::UnsupportedEvent)
        ));
        assert!(matches!(
            Transaction::<Deposit>::try_from(wrong_kind),
            Err(DomainError::UnsupportedEvent)
        ));
    }

    #[test]
    fn withdrawal_event_becomes_withdrawal() {
        let tx = Transaction::<Withdrawal>::try_from(event(
            EventKind::Withdrawal,
            CLIENT,
            TX,
            Some(dec!(2)),
        ))
        .unwrap();
        assert_eq!(tx.client_id(), ClientId::from(CLIENT));
        assert_eq!(tx.tx_id(), TxId::from(TX));
        assert_eq!(Decimal::from(tx.amount()), dec!(2));
    }

    #[test]
    fn withdrawal_requires_withdrawal_kind_and_amount() {
        let missing_amount = event(EventKind::Withdrawal, CLIENT, TX, None);
        let wrong_kind = event(EventKind::Deposit, CLIENT, TX, Some(dec!(1)));
        assert!(matches!(
            Transaction::<Withdrawal>::try_from(missing_amount),
            Err(DomainError::UnsupportedEvent)
        ));
        assert!(matches!(
            Transaction::<Withdrawal>::try_from(wrong_kind),
            Err(DomainError::UnsupportedEvent)
        ));
    }

    // --- event checks used by the engine ---

    #[test]
    fn ensure_owned_by_accepts_owner_and_rejects_other_client() {
        let (tx, _) = deposited(dec!(5));
        let own = event(EventKind::Dispute, CLIENT, TX, None);
        let foreign = event(EventKind::Dispute, CLIENT + 1, TX, None);
        assert_eq!(tx.ensure_owned_by(&own), Ok(()));
        assert_eq!(
            tx.ensure_owned_by(&foreign),
            Err(DomainError::ClientMismatch)
        );
    }

    #[test]
    fn ensure_transaction_by_accepts_same_tx_and_rejects_other_tx() {
        let (tx, _) = deposited(dec!(5));
        let same = event(EventKind::Dispute, CLIENT, TX, None);
        let other = event(EventKind::Dispute, CLIENT, TX + 1, None);
        assert_eq!(tx.ensure_transaction_by(&same), Ok(()));
        assert_eq!(
            tx.ensure_transaction_by(&other),
            Err(DomainError::TransactionMismatch)
        );
    }

    // --- dispute ---

    #[test]
    fn dispute_holds_funds_and_counts() {
        let (mut tx, mut account) = deposited(dec!(5));
        tx.dispute(&mut account).unwrap();
        assert_eq!(tx.state(), DisputeState::Disputed);
        assert_eq!(tx.dispute_count(), 1);
        assert_balances(&account, dec!(0), dec!(5));
    }

    #[test]
    fn dispute_after_withdrawal_makes_available_negative() {
        let (mut tx, mut account) = deposited(dec!(5));
        account
            .withdraw(Amount::try_from(dec!(3)).unwrap())
            .unwrap();
        tx.dispute(&mut account).unwrap();
        assert_balances(&account, dec!(-3), dec!(5));
    }

    #[test]
    fn second_dispute_while_open_is_rejected() {
        let (mut tx, mut account) = deposited(dec!(5));
        tx.dispute(&mut account).unwrap();
        assert_eq!(
            tx.dispute(&mut account),
            Err(DomainError::DisputeAlreadyOpen)
        );
        assert_eq!(tx.dispute_count(), 1);
        assert_balances(&account, dec!(0), dec!(5));
    }

    #[test]
    fn dispute_can_be_reopened_after_resolve_and_count_grows() {
        let (mut tx, mut account) = deposited(dec!(5));
        for round in 1..=3 {
            tx.dispute(&mut account).unwrap();
            assert_eq!(tx.dispute_count(), round);
            tx.resolve(&mut account).unwrap();
        }
        assert_eq!(tx.state(), DisputeState::Processed);
        assert_balances(&account, dec!(5), dec!(0));
    }

    #[test]
    fn dispute_on_other_clients_account_is_rejected() {
        let (mut tx, _) = deposited(dec!(5));
        let mut stranger = Account::new(ClientId::from(CLIENT + 1));
        assert_eq!(tx.dispute(&mut stranger), Err(DomainError::ClientMismatch));
        assert_eq!(tx.state(), DisputeState::Processed);
        assert_eq!(tx.dispute_count(), 0);
        assert_balances(&stranger, dec!(0), dec!(0));
    }

    // --- resolve ---

    #[test]
    fn resolve_releases_held_funds() {
        let (mut tx, mut account) = deposited(dec!(5));
        tx.dispute(&mut account).unwrap();
        tx.resolve(&mut account).unwrap();
        assert_eq!(tx.state(), DisputeState::Processed);
        assert_eq!(tx.dispute_count(), 1);
        assert_balances(&account, dec!(5), dec!(0));
    }

    #[test]
    fn resolve_without_dispute_is_rejected() {
        let (mut tx, mut account) = deposited(dec!(5));
        assert_eq!(tx.resolve(&mut account), Err(DomainError::NotDisputed));
        assert_balances(&account, dec!(5), dec!(0));
    }

    #[test]
    fn resolve_on_other_clients_account_is_rejected() {
        let (mut tx, mut account) = deposited(dec!(5));
        tx.dispute(&mut account).unwrap();
        let mut stranger = Account::new(ClientId::from(CLIENT + 1));
        assert_eq!(tx.resolve(&mut stranger), Err(DomainError::ClientMismatch));
        assert_eq!(tx.state(), DisputeState::Disputed);
        assert_balances(&account, dec!(0), dec!(5));
        assert_balances(&stranger, dec!(0), dec!(0));
    }

    // --- chargeback ---

    #[test]
    fn chargeback_removes_held_funds_and_locks_account() {
        let (mut tx, mut account) = deposited(dec!(5));
        tx.dispute(&mut account).unwrap();
        tx.charge_back(&mut account).unwrap();
        assert_eq!(tx.state(), DisputeState::ChargedBack);
        assert!(account.is_locked());
        assert_balances(&account, dec!(0), dec!(0));
    }

    #[test]
    fn chargeback_without_dispute_is_rejected() {
        let (mut tx, mut account) = deposited(dec!(5));
        assert_eq!(tx.charge_back(&mut account), Err(DomainError::NotDisputed));
        assert!(!account.is_locked());
        assert_balances(&account, dec!(5), dec!(0));
    }

    #[test]
    fn chargeback_after_resolve_is_rejected() {
        let (mut tx, mut account) = deposited(dec!(5));
        tx.dispute(&mut account).unwrap();
        tx.resolve(&mut account).unwrap();
        assert_eq!(tx.charge_back(&mut account), Err(DomainError::NotDisputed));
        assert!(!account.is_locked());
    }

    #[test]
    fn chargeback_on_other_clients_account_is_rejected() {
        let (mut tx, mut account) = deposited(dec!(5));
        tx.dispute(&mut account).unwrap();
        let mut stranger = Account::new(ClientId::from(CLIENT + 1));
        assert_eq!(
            tx.charge_back(&mut stranger),
            Err(DomainError::ClientMismatch)
        );
        assert_eq!(tx.state(), DisputeState::Disputed);
        assert!(!stranger.is_locked());
    }

    #[test]
    fn charged_back_tx_is_final() {
        let (mut tx, mut account) = deposited(dec!(5));
        tx.dispute(&mut account).unwrap();
        tx.charge_back(&mut account).unwrap();

        assert_eq!(
            tx.dispute(&mut account),
            Err(DomainError::AlreadyChargedBack)
        );
        assert_eq!(
            tx.resolve(&mut account),
            Err(DomainError::AlreadyChargedBack)
        );
        assert_eq!(
            tx.charge_back(&mut account),
            Err(DomainError::AlreadyChargedBack)
        );
        assert_eq!(tx.dispute_count(), 1);
        assert_balances(&account, dec!(0), dec!(0));
    }

    #[test]
    fn locked_account_still_processes_disputes_on_other_deposits() {
        let (mut first, mut account) = deposited(dec!(5));
        let second_event = event(EventKind::Deposit, CLIENT, TX + 1, Some(dec!(3)));
        account.deposit(second_event.amount.unwrap()).unwrap();
        let mut second = Transaction::<Deposit>::try_from(second_event).unwrap();

        first.dispute(&mut account).unwrap();
        first.charge_back(&mut account).unwrap();
        assert!(account.is_locked());

        second.dispute(&mut account).unwrap();
        assert_balances(&account, dec!(0), dec!(3));
        second.resolve(&mut account).unwrap();
        assert_balances(&account, dec!(3), dec!(0));
    }

    #[test]
    fn dispute_overflow_is_rejected_and_changes_nothing() {
        let big = Decimal::MAX - dec!(1);
        let mut account = Account::new(ClientId::from(CLIENT));
        let mut deposits = Vec::new();
        for tx in [TX, TX + 1] {
            let deposit = event(EventKind::Deposit, CLIENT, tx, Some(big));
            account.deposit(deposit.amount.unwrap()).unwrap();
            account.withdraw(deposit.amount.unwrap()).unwrap();
            deposits.push(Transaction::<Deposit>::try_from(deposit).unwrap());
        }

        deposits[0].dispute(&mut account).unwrap();
        assert_eq!(
            deposits[1].dispute(&mut account),
            Err(DomainError::Overflow)
        );
        assert_eq!(deposits[1].state(), DisputeState::Processed);
        assert_eq!(deposits[1].dispute_count(), 0);
        assert_balances(&account, -big, big);
    }
}
