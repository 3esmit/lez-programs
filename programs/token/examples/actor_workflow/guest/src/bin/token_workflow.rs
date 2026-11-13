#[path = "../../../src/receiver.rs"]
mod receiver;

lee_core::actor_entrypoint!(raw receiver::handle_message);
