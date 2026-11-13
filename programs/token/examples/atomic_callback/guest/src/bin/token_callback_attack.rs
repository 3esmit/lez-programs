//! Deliberately try to spend the payer's remaining balance during its pending transfer.
use lee_core::{
    account::Actor,
    program::{ReceiveInput, Response},
};
use token_core::{Message, TokensReceived};

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, receipt: TokensReceived) -> Response {
    let token = input
        .from
        .expect("a token callback has a sender")
        .program_account_id;
    Response::keep_state().send_sync(
        Actor::new(receipt.source, token),
        &Message::Transfer {
            to: receipt.destination,
            amount: 1,
        },
    )
}
