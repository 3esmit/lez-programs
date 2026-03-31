use amm_core::{compute_vault_pda_seed, PoolDefinition, RecoverSurplusMode};
use nssa_core::{
    account::AccountWithMetadata,
    program::{AccountPostState, ChainedCall},
};

use crate::vault_utils::{read_fungible_holding, read_vault_fungible_balances};

#[expect(clippy::too_many_arguments, reason = "TODO: Fix later")]
pub fn recover_surplus(
    pool: AccountWithMetadata,
    vault_a: AccountWithMetadata,
    vault_b: AccountWithMetadata,
    to_holding_a: AccountWithMetadata,
    to_holding_b: AccountWithMetadata,
    mode: RecoverSurplusMode,
) -> (Vec<AccountPostState>, Vec<ChainedCall>) {
    let pool_def_data = PoolDefinition::try_from(&pool.account.data)
        .expect("Recover surplus: AMM Program expects a valid Pool Definition Account");

    assert_eq!(
        vault_a.account_id, pool_def_data.vault_a_id,
        "Vault A was not provided"
    );
    assert_eq!(
        vault_b.account_id, pool_def_data.vault_b_id,
        "Vault B was not provided"
    );

    let token_program_id = vault_a.account.program_owner;
    assert_eq!(
        vault_b.account.program_owner, token_program_id,
        "Vaults must use the same Token Program"
    );
    assert_eq!(
        to_holding_a.account.program_owner, token_program_id,
        "Recipient A holding must use the same Token Program"
    );
    assert_eq!(
        to_holding_b.account.program_owner, token_program_id,
        "Recipient B holding must use the same Token Program"
    );

    let (vault_a_definition_id, _) = read_fungible_holding(&vault_a, "Recover surplus Vault A");
    let (vault_b_definition_id, _) = read_fungible_holding(&vault_b, "Recover surplus Vault B");
    assert_eq!(
        vault_a_definition_id, pool_def_data.definition_token_a_id,
        "Vault A token definition mismatch"
    );
    assert_eq!(
        vault_b_definition_id, pool_def_data.definition_token_b_id,
        "Vault B token definition mismatch"
    );

    let (recipient_a_definition_id, _) =
        read_fungible_holding(&to_holding_a, "Recover surplus Recipient A");
    let (recipient_b_definition_id, _) =
        read_fungible_holding(&to_holding_b, "Recover surplus Recipient B");
    assert_eq!(
        recipient_a_definition_id, pool_def_data.definition_token_a_id,
        "Recipient holding A token definition mismatch"
    );
    assert_eq!(
        recipient_b_definition_id, pool_def_data.definition_token_b_id,
        "Recipient holding B token definition mismatch"
    );

    match mode {
        RecoverSurplusMode::InactiveOrZeroSupplyOnly => {
            assert!(
                !pool_def_data.active || pool_def_data.liquidity_pool_supply == 0,
                "Recover surplus is only allowed for inactive or zero-supply pools"
            );
        }
    }

    let (vault_a_balance, vault_b_balance) =
        read_vault_fungible_balances("Recover surplus", &vault_a, &vault_b);
    assert!(
        vault_a_balance >= pool_def_data.reserve_a,
        "Recover surplus: vault A balance is less than its reserve"
    );
    assert!(
        vault_b_balance >= pool_def_data.reserve_b,
        "Recover surplus: vault B balance is less than its reserve"
    );
    let surplus_a = vault_a_balance - pool_def_data.reserve_a;
    let surplus_b = vault_b_balance - pool_def_data.reserve_b;

    let mut chained_calls = Vec::new();

    if surplus_a > 0 {
        let mut vault_a_auth = vault_a.clone();
        vault_a_auth.is_authorized = true;
        chained_calls.push(
            ChainedCall::new(
                token_program_id,
                vec![vault_a_auth, to_holding_a.clone()],
                &token_core::Instruction::Transfer {
                    amount_to_transfer: surplus_a,
                },
            )
            .with_pda_seeds(vec![compute_vault_pda_seed(
                pool.account_id,
                pool_def_data.definition_token_a_id,
            )]),
        );
    }

    if surplus_b > 0 {
        let mut vault_b_auth = vault_b.clone();
        vault_b_auth.is_authorized = true;
        chained_calls.push(
            ChainedCall::new(
                token_program_id,
                vec![vault_b_auth, to_holding_b.clone()],
                &token_core::Instruction::Transfer {
                    amount_to_transfer: surplus_b,
                },
            )
            .with_pda_seeds(vec![compute_vault_pda_seed(
                pool.account_id,
                pool_def_data.definition_token_b_id,
            )]),
        );
    }

    // Surplus recovery only transfers balances above the tracked reserves, so the pool reserves
    // remain unchanged and no follow-up sync is required.
    (
        vec![
            AccountPostState::new(pool.account.clone()),
            AccountPostState::new(vault_a.account.clone()),
            AccountPostState::new(vault_b.account.clone()),
            AccountPostState::new(to_holding_a.account.clone()),
            AccountPostState::new(to_holding_b.account.clone()),
        ],
        chained_calls,
    )
}
