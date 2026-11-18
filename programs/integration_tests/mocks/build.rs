fn main() {
    println!("cargo:rerun-if-changed=src/oracle.rs");
    println!("cargo:rerun-if-changed=guest/src/bin/read_state_probe.rs");
    risc0_build::embed_methods();
}
