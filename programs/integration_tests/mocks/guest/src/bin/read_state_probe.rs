//! Test actor that records a public AMM state reply from the requested actor.

use amm_core::Message;
use lee_core::{
    account::Actor,
    program::{ReceiveInput, Response, StateReply},
};

fn handle_message(input: &ReceiveInput) -> Response {
    let Some(sender) = input.from else {
        let target: Actor = borsh::from_slice(&input.message).expect("target decodes");
        return Response::set_state(borsh::to_vec(&target).expect("target encodes"))
            .send_sync(target, &Message::ReadState);
    };
    let expected: Actor = borsh::from_slice(&input.pre_state).expect("expected actor decodes");
    assert_eq!(sender, expected, "unexpected state reply sender");
    let reply: StateReply = borsh::from_slice(&input.message).expect("state reply decodes");
    Response::set_state(reply.state)
}

lee_core::actor_entrypoint!(raw handle_message);
