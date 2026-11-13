//! A public payment processor. Token supplies authenticated transfer facts; this program
//! owns distribution rules. No rule in this file is compiled into the Token Program.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};
use token_core::{Message as TokenMessage, TokensReceived};

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Command {
    Initialize {
        token_program: AccountId,
        definition: AccountId,
        beneficiary: AccountId,
    },
    /// Owner-controlled outgoing atomic transfer, also used to demonstrate PDA grant isolation.
    SendFromVault {
        to: AccountId,
        amount: u128,
        callback_account: AccountId,
        callback_program: AccountId,
    },
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Distribution {
    pub pay: u128,
    pub burn: u128,
    /// Demonstration switch: fail after all emitted token operations have executed.
    pub reject_at_completion: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct State {
    pub token_program: AccountId,
    pub definition: AccountId,
    pub beneficiary: AccountId,
    pub completed: u64,
    pub last_received: u128,
    pub busy: bool,
}

#[derive(BorshSerialize, BorshDeserialize)]
enum Internal {
    Complete { reject: bool },
}

pub fn vault_seed(processor: AccountId) -> PdaSeed {
    // This example has one PDA kind: one vault per processor actor.
    PdaSeed::new(processor.to_bytes())
}

pub fn vault(processor: Actor) -> AccountId {
    AccountId::for_public_pda(
        &processor.program_account_id,
        &vault_seed(processor.account_id),
    )
}

fn write(state: &State) -> Response {
    Response::set_state(borsh::to_vec(state).expect("example state encodes"))
}

fn token_call(input: &ReceiveInput, state: &State, message: &TokenMessage) -> SyncMessage {
    SyncMessage::new(
        Actor::new(vault(input.receiver), state.token_program),
        message,
    )
    .with_pda_seeds([vault_seed(input.receiver.account_id)])
}

pub fn handle_message(input: &ReceiveInput) -> Response {
    if input.pre_state.is_empty() {
        assert!(
            input.is_authorized,
            "processor owner must authorize initialization"
        );
        let Command::Initialize {
            token_program,
            definition,
            beneficiary,
        } = borsh::from_slice(&input.message).expect("initialization must decode")
        else {
            panic!("processor must be initialized")
        };
        let state = State {
            token_program,
            definition,
            beneficiary,
            completed: 0,
            last_received: 0,
            busy: false,
        };
        return write(&state).send(token_call(
            input,
            &state,
            &TokenMessage::InitializeHolding { definition },
        ));
    }
    let mut state: State = borsh::from_slice(&input.pre_state).expect("example state decodes");
    if input.from == Some(input.receiver) {
        let Internal::Complete { reject } =
            borsh::from_slice(&input.message).expect("completion decodes");
        assert!(state.busy, "no distribution in progress");
        assert!(!reject, "example rejects after token effects");
        state.busy = false;
        state.completed = state
            .completed
            .checked_add(1)
            .expect("completion count overflow");
        return write(&state);
    }
    assert!(!state.busy, "distribution in progress");
    let expected = Actor::new(vault(input.receiver), state.token_program);
    if input.from == Some(expected) {
        let receipt: TokensReceived =
            borsh::from_slice(&input.message).expect("token receipt decodes");
        assert_eq!(
            receipt.destination, expected.account_id,
            "receipt names another vault"
        );
        assert_eq!(
            receipt.definition, state.definition,
            "unexpected token definition"
        );
        let Distribution {
            pay,
            burn,
            reject_at_completion,
        } = borsh::from_slice(&receipt.data).expect("distribution request decodes");
        let refund = receipt
            .amount
            .checked_sub(pay)
            .and_then(|left| left.checked_sub(burn))
            .expect("distribution exceeds received amount");
        state.busy = true;
        state.last_received = receipt.amount;
        // The Token actor stages the credit before calling this handler. The processor
        // spends its own vault with its own PDA grant; it does not debit the payer again.
        return write(&state)
            .send(token_call(
                input,
                &state,
                &TokenMessage::Transfer {
                    to: state.beneficiary,
                    amount: pay,
                },
            ))
            .send(token_call(
                input,
                &state,
                &TokenMessage::Burn { amount: burn },
            ))
            .send(token_call(
                input,
                &state,
                &TokenMessage::Transfer {
                    to: receipt.source,
                    amount: refund,
                },
            ))
            .send_sync(
                input.receiver,
                &Internal::Complete {
                    reject: reject_at_completion,
                },
            );
    }
    assert!(
        input.is_authorized,
        "processor owner must authorize commands"
    );
    let command: Command = borsh::from_slice(&input.message).expect("processor command decodes");
    match command {
        Command::Initialize { .. } => panic!("processor already initialized"),
        Command::SendFromVault {
            to,
            amount,
            callback_account,
            callback_program,
        } => Response::keep_state().send(token_call(
            input,
            &state,
            &TokenMessage::TransferAndCallAtomic {
                to,
                amount,
                callback_account,
                callback_program,
                data: Vec::new(),
            },
        )),
    }
}
