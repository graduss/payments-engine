use thiserror::Error;

use crate::domain::DomainError;

/// Errors the engine finds itself, on top of the domain rules.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ApplicationError {
    #[error("duplicate tx")]
    DuplicateTx,

    #[error("account does not exist")]
    AccountNotFound,

    #[error("tx is not a processed deposit")]
    NotADeposit,

    #[error(transparent)]
    Domain(#[from] DomainError),
}
