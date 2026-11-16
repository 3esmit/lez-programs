#[path = "../../../src/coordinator.rs"]
pub mod coordinator;
#[path = "../../../src/payment.rs"]
mod payment;
lee_core::actor_entrypoint!(raw payment::handle_message);
