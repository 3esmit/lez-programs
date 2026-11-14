#[path = "../../../src/coordinator.rs"]
pub mod coordinator;
#[path = "../../../src/payment.rs"]
mod payment;
#[path = "../../../src/receipts.rs"]
pub mod receipts;
lee_core::actor_entrypoint!(raw payment::handle_message);
