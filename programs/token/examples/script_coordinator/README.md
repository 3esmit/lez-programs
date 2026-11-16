# Reusable continuation coordinator

Embed a typed continuation machine in each consumer. It coordinates reads, calculations, effects and final checks.

This folder contains both payment and two-asset redemption guests.

Run from the repository root:

```sh
RISC0_DEV_MODE=1 cargo test -p integration_tests --test script_coordinator_payment --test script_coordinator_redemption
```

See the [comparison index](../README.md) for the branch layout, shared inputs and limitations.
