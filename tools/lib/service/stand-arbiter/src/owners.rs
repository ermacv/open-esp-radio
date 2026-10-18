//! Renaming and forgetting an owner's balance and history; who an owner
//! is lives in `oer-stand-owners`.

use oer_stand_owners::Owner;

use crate::Arbiter;

impl Arbiter {
    /// Charge `old`'s balance to `new` and name `new` in `old`'s history,
    /// for an owner that was known under another name.
    pub fn merge_owner(&self, old: &str, new: &Owner) -> crate::Result<()> {
        self.transaction(|state| {
            if let Some(balance) = state.balances.remove(old) {
                let merged = state.balances.entry(new.as_str().to_owned()).or_default();
                merged.balance_ms += balance.balance_ms;
                merged.last_active_unix_ms =
                    merged.last_active_unix_ms.max(balance.last_active_unix_ms);
            }
            crate::history::rename_owner(&self.history_path(), old, new.as_str())
        })
    }

    /// Drop `name`'s balance, for an owner that never was an agent.
    pub fn forget_owner(&self, name: &str) -> crate::Result<bool> {
        self.transaction(|state| Ok(state.balances.remove(name).is_some()))
    }
}
