#[path = "../../../src/policy.rs"]
pub mod policy;
#[path = "../../../src/redemption.rs"]
mod redemption;
lee_core::actor_entrypoint!(raw redemption::handle_message);
