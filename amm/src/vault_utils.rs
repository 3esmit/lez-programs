use nssa_core::account::{AccountId, AccountWithMetadata};

pub fn read_fungible_holding(account: &AccountWithMetadata, context: &str) -> (AccountId, u128) {
    let token_holding = token_core::TokenHolding::try_from(&account.account.data)
        .unwrap_or_else(|_| panic!("{context}: AMM Program expects a valid Token Holding Account"));

    let token_core::TokenHolding::Fungible {
        definition_id,
        balance,
    } = token_holding
    else {
        panic!("{context}: AMM Program expects a valid Fungible Token Holding Account");
    };

    (definition_id, balance)
}

pub fn read_vault_fungible_balances(
    context: &str,
    vault_a: &AccountWithMetadata,
    vault_b: &AccountWithMetadata,
) -> (u128, u128) {
    let vault_a_context = format!("{context}: Vault A");
    let vault_b_context = format!("{context}: Vault B");
    let (_, vault_a_balance) = read_fungible_holding(vault_a, &vault_a_context);
    let (_, vault_b_balance) = read_fungible_holding(vault_b, &vault_b_context);

    (vault_a_balance, vault_b_balance)
}
