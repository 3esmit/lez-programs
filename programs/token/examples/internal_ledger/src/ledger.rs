//! Custodial claims and protocol reserves are separate accounting categories.
use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::account::AccountId;

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Ledger {
    pub claims: BTreeMap<(AccountId, AccountId), u128>,
    pub reserves: BTreeMap<AccountId, u128>,
}

impl Ledger {
    pub fn claim(&self, owner: AccountId, asset: AccountId) -> u128 {
        self.claims
            .get(&(owner, asset))
            .copied()
            .unwrap_or_default()
    }

    pub fn credit(&mut self, owner: AccountId, asset: AccountId, amount: u128) {
        if amount == 0 {
            return;
        }
        let value = self
            .claim(owner, asset)
            .checked_add(amount)
            .expect("claim overflow");
        assert!(
            self.claims.contains_key(&(owner, asset)) || self.claims.len() < 64,
            "ledger claim limit"
        );
        self.claims.insert((owner, asset), value);
    }

    pub fn debit(&mut self, owner: AccountId, asset: AccountId, amount: u128) {
        let value = self
            .claim(owner, asset)
            .checked_sub(amount)
            .expect("insufficient internal claim");
        if value == 0 {
            self.claims.remove(&(owner, asset));
        } else {
            self.claims.insert((owner, asset), value);
        }
    }

    pub fn transfer(&mut self, from: AccountId, to: AccountId, asset: AccountId, amount: u128) {
        self.debit(from, asset, amount);
        self.credit(to, asset, amount);
    }

    pub fn sync_reserve(&mut self, asset: AccountId, actual: u128) -> u128 {
        let liabilities = self
            .claims
            .iter()
            .filter(|((_, definition), _)| *definition == asset)
            .try_fold(0_u128, |sum, (_, amount)| sum.checked_add(*amount))
            .expect("claim sum overflow");
        // Unallocated donations to a redemption vault belong to all share holders.
        let reserve = actual
            .checked_sub(liabilities)
            .expect("custody below outstanding claims");
        self.reserves.insert(asset, reserve);
        reserve
    }

    pub fn allocate(&mut self, owner: AccountId, asset: AccountId, amount: u128) {
        let reserve = self
            .reserves
            .get_mut(&asset)
            .expect("reserve was synchronized");
        *reserve = reserve
            .checked_sub(amount)
            .expect("insufficient internal reserve");
        self.credit(owner, asset, amount);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn several_internal_transfers_only_change_claims() {
        let [a, b, c, asset] = [1, 2, 3, 4].map(|n| AccountId::new([n; 32]));
        let mut ledger = Ledger::default();
        ledger.credit(a, asset, 100);
        ledger.transfer(a, b, asset, 100);
        ledger.transfer(b, c, asset, 70);
        ledger.transfer(c, a, asset, 40);
        assert_eq!(
            (
                ledger.claim(a, asset),
                ledger.claim(b, asset),
                ledger.claim(c, asset)
            ),
            (40, 30, 30)
        );
        assert_eq!(ledger.sync_reserve(asset, 100), 0);
    }
    #[test]
    fn claims_are_excluded_from_reserves() {
        let asset = AccountId::new([1; 32]);
        let owner = AccountId::new([2; 32]);
        let mut ledger = Ledger::default();
        ledger.credit(owner, asset, 100);
        assert_eq!(ledger.sync_reserve(asset, 1_000), 900);
    }
}
