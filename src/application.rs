//! Use cases: the payment engine and the processing of a row stream.

mod engine;
mod error;
mod process;
mod used_tx;

pub use engine::PaymentEngine;
pub use process::{Accepted, Category, InvalidRow, Outcome, Rejection, Row, Summary, process};
