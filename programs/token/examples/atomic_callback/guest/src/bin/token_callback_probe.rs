//! Runs on the source PDA's account under a different program. The source PDA grant
//! must not arrive here, even though this callback executes in the same transaction.
use lee_core::program::{ReceiveInput, Response};
use token_core::TokensReceived;

lee_core::actor_entrypoint!(handle_message);

fn handle_message(input: &ReceiveInput, _: TokensReceived) -> Response {
    assert!(!input.is_authorized, "source PDA grant reached the hook");
    Response::keep_state()
}
