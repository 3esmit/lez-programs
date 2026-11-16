#[path = "../../../src/coordinator.rs"]
pub mod coordinator;
#[path = "../../../src/receipts.rs"]
pub mod receipts;
#[path = "../../../src/redemption.rs"]
mod redemption;
lee_core::actor_entrypoint!(raw redemption::handle_message);
