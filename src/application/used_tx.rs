use crate::domain::TxId;
use std::collections::HashSet;

/// Tx ids that are taken but not kept as deposits: withdrawals and rejected
/// deposits. Behind a type so the `HashSet` can later become something smaller.
#[derive(Debug, Default)]
pub(crate) struct UsedTxIds(HashSet<TxId>);

impl UsedTxIds {
    pub(crate) fn contains(&self, tx: &TxId) -> bool {
        self.0.contains(tx)
    }

    pub(crate) fn insert(&mut self, tx: TxId) -> bool {
        self.0.insert(tx)
    }
}
