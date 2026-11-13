#[path = "../../../src/spender.rs"]
mod spender;
lee_core::actor_entrypoint!(raw spender::handle_message);
