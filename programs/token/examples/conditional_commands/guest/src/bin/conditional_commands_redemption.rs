#[path = "../../../src/redemption.rs"]
mod redemption;
lee_core::actor_entrypoint!(raw redemption::handle_message);
