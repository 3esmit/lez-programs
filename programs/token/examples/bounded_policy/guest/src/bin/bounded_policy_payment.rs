#[path = "../../../src/payment.rs"]
mod payment;
#[path = "../../../src/policy.rs"]
pub mod policy;
lee_core::actor_entrypoint!(raw payment::handle_message);
