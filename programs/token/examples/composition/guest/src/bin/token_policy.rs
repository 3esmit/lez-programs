#[path = "../../../src/processor.rs"]
pub mod processor;
lee_core::actor_entrypoint!(raw processor::policy);
