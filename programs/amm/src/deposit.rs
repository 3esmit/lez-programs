use amm_core::{
    AmmState, DepositConfigRead, Internal, Message, Pending, PoolDefinition, SwapDeposit,
};
use lee_core::{
    account::Actor,
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
    let request: SwapDeposit =
        borsh::from_slice(&receipt.data).expect("swap deposit parameters must decode");
    let accounts = vec![
        request.user_output_holding,
        pool.vault_a_id,
        pool.vault_b_id,
    ];
    let pending = Pending {
        config_id: pool.config_id,
        config: Some(config),
        pool: Some(pool),
        request: Message::SwapExactInput {
            user_input_holding: receipt.source,
            user_output_holding: request.user_output_holding,
            swap_amount_in: receipt.amount,
            min_amount_out: request.min_amount_out,
            deadline: request.deadline,
        },
        expected_account: input.receiver.account_id,
        expected_program: input.receiver.program_account_id,
        remaining: vec![],
        holdings: vec![],
    };
    let response = reads::read_holdings(input, pending, accounts, Some(input_definition));
    reads::with_deadline(response, Some(request.deadline))
}
