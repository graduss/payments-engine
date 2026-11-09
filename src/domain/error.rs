use thiserror::Error;

/// Why a domain rule refused an operation. The text is what ends up in the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("amount must be greater than zero")]
    AmountNotPositive,

    #[error("amount has more than {0} decimal places")]
    AmountTooPrecise(u32),

    #[error("account locked")]
    AccountLocked,

    #[error("insufficient funds")]
    InsufficientFunds,

    #[error("client does not own tx")]
    ClientMismatch,

    #[error("event refers to another tx")]
    TransactionMismatch,

    #[error("dispute already open")]
    DisputeAlreadyOpen,

    #[error("tx is not under dispute")]
    NotDisputed,

    #[error("already charged back")]
    AlreadyChargedBack,

    #[error("unsupported event")]
    /// The event lacks what the transaction needs, e.g. a deposit without an amount.
    UnsupportedEvent,

    #[error("arithmetic overflow")]
    Overflow,
}
