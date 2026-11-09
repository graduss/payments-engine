//! A toy payments engine: applies a CSV stream of client transactions and
//! reports the final state of every account.
//!
//! Layers, innermost first: `domain` (rules), `application` (engine and
//! row processing), `infrastructure` (CSV, configuration, log) and
//! `interfaces` (the command line). Dependencies point inwards only.

pub mod application;
pub mod domain;
mod infrastructure;
pub mod interfaces;
