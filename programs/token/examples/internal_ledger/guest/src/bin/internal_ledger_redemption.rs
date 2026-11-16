#[path = "../../../src/coordinator.rs"]
pub mod coordinator;
#[path = "../../../src/ledger.rs"]
pub mod ledger;
#[path = "../../../src/redemption.rs"]
mod redemption;
lee_core::actor_entrypoint!(raw redemption::handle_message);
