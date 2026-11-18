use amm_core::{
    AmmState, DepositConfigRead, DepositFunding, FundedDeposit, Internal, Message, Pending,
    PoolDefinition, SwapDeposit,
};
use lee_core::{
    account::{AccountId, Actor},
    program::{ReceiveInput, Response, StateReply},
};
use token_core::TokensReceived;

use crate::{actor, reads};

pub(crate) fn start(input: &ReceiveInput, pool: PoolDefinition, sender: Actor) -> Response {
    let config = actor(input, pool.config_id);
    let pending = DepositConfigRead {
        pool,
        sender_account: sender.account_id,
        sender_program: sender.program_account_id,
        receipt: input.message.clone(),
    };
    Response::set_state(AmmState::ReadingDepositConfig(Box::new(pending)).encode())
        .send_sync(config, &Internal::ReadState)
}

pub(crate) fn config_reply(input: &ReceiveInput, pending: DepositConfigRead) -> Response {
    let DepositConfigRead {
        pool,
        sender_account,
        sender_program,
        receipt,
    } = pending;
    assert_eq!(
        input.from,
        Some(actor(input, pool.config_id)),
        "unexpected deposit config reply sender"
    );
    let reply: StateReply = borsh::from_slice(&input.message).expect("state reply must decode");
    let Some(AmmState::Config(config)) = AmmState::decode(&reply.state) else {
        panic!("AMM config must be initialized");
    };
    let input_definition = if sender_account == pool.vault_a_id {
        pool.definition_token_a_id
    } else {
        assert_eq!(sender_account, pool.vault_b_id, "unexpected deposit vault");
        pool.definition_token_b_id
    };
    assert_eq!(
        sender_program, config.token_program_id,
        "deposit must come from the configured Token program"
    );
    let receipt: TokensReceived = borsh::from_slice(&receipt).expect("Token receipt must decode");
    assert_eq!(
        receipt.destination, sender_account,
        "receipt names another vault"
    );
    assert_eq!(
        receipt.definition, input_definition,
        "receipt has the wrong asset"
    );
    assert_ne!(
        receipt.source, sender_account,
        "swap deposit cannot pay itself"
    );
    let funding = DepositFunding {
        definition: input_definition,
        amount: receipt.amount,
    };
    let (request, mut accounts, deadline) = deposit_request(&pool, receipt, input_definition);
    accounts.extend([pool.vault_a_id, pool.vault_b_id]);
    let pending = Pending {
        config_id: pool.config_id,
        config: Some(config),
        pool: Some(pool),
        request,
        expected_account: input.receiver.account_id,
        expected_program: input.receiver.program_account_id,
        remaining: vec![],
        holdings: vec![],
    };
    let response = reads::read_holdings(input, pending, accounts, Some(funding));
    reads::with_deadline(response, Some(deadline))
}

fn check_user_holding(pool: &PoolDefinition, holding: AccountId) {
    assert!(
        holding != pool.vault_a_id && holding != pool.vault_b_id,
        "callback user holding cannot be a pool vault"
    );
}

fn deposit_request(
    pool: &PoolDefinition,
    receipt: TokensReceived,
    input_definition: AccountId,
) -> (Message, Vec<AccountId>, u64) {
    // The old untagged payload is shorter than either tagged payload. Borsh
    // requires the entire payload to decode, so their formats do not overlap.
    if let Ok(request) = borsh::from_slice::<SwapDeposit>(&receipt.data) {
        return (
            Message::SwapExactInput {
                user_input_holding: receipt.source,
                user_output_holding: request.user_output_holding,
                swap_amount_in: receipt.amount,
                min_amount_out: request.min_amount_out,
                deadline: request.deadline,
            },
            vec![request.user_output_holding],
            request.deadline,
        );
    }
    check_user_holding(pool, receipt.source);
    match borsh::from_slice::<FundedDeposit>(&receipt.data)
        .expect("funded deposit parameters must decode")
    {
        FundedDeposit::SwapExactOutput {
            user_output_holding,
            exact_amount_out,
            max_amount_in,
            deadline,
        } => {
            check_user_holding(pool, user_output_holding);
            (
                Message::SwapExactOutput {
                    user_input_holding: receipt.source,
                    user_output_holding,
                    exact_amount_out,
                    max_amount_in,
                    deadline,
                },
                vec![user_output_holding],
                deadline,
            )
        }
        FundedDeposit::AddLiquidity {
            other_holding,
            user_holding_lp,
            min_amount_liquidity,
            max_amount_to_add_token_a,
            max_amount_to_add_token_b,
            deadline,
        } => {
            check_user_holding(pool, other_holding);
            check_user_holding(pool, user_holding_lp);
            assert_ne!(
                other_holding, receipt.source,
                "callback add needs a separate holding for the other asset"
            );
            let (user_holding_a, user_holding_b) = if input_definition == pool.definition_token_a_id
            {
                (receipt.source, other_holding)
            } else {
                (other_holding, receipt.source)
            };
            (
                Message::AddLiquidity {
                    user_holding_a,
                    user_holding_b,
                    user_holding_lp,
                    min_amount_liquidity,
                    max_amount_to_add_token_a,
                    max_amount_to_add_token_b,
                    deadline,
                },
                vec![other_holding, user_holding_lp],
                deadline,
            )
        }
    }
}
