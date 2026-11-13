//! A controller-authorized actor which spends another account's bounded permission.
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{ReceiveInput, Response},
};
use token_core::Message;

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Command {
    Spend {
        token_program: AccountId,
        source: AccountId,
        generation: u64,
        nonce: u64,
        pay: u128,
        burn: u128,
        reject_after: bool,
    },
    /// Negative example: an allowance does not authorize the ordinary Transfer entrypoint.
    Bypass {
        token_program: AccountId,
        source: AccountId,
        destination: AccountId,
        amount: u128,
    },
}

pub fn handle_message(input: &ReceiveInput) -> Response {
    if input.from == Some(input.receiver) {
        let reject: bool = borsh::from_slice(&input.message).expect("completion decodes");
        assert!(!reject, "example rejects after spending");
        return Response::keep_state();
    }
    assert!(input.is_authorized, "spender controller must authorize");
    match borsh::from_slice(&input.message).expect("command decodes") {
        Command::Spend {
            token_program,
            source,
            generation,
            nonce,
            pay,
            burn,
            reject_after,
        } => Response::keep_state()
            .send_sync(
                Actor::new(source, token_program),
                &Message::Spend {
                    generation,
                    nonce,
                    pay,
                    burn,
                },
            )
            .send_sync(input.receiver, &reject_after),
        Command::Bypass {
            token_program,
            source,
            destination,
            amount,
        } => Response::keep_state().send_sync(
            Actor::new(source, token_program),
            &Message::Transfer {
                to: destination,
                amount,
            },
        ),
    }
}
