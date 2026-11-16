# Client-proposed plans

Authenticate current Token state and validate the client proposal before settlement. Stale proposals fail.

This folder contains both payment and two-asset redemption guests.

Run from the repository root:

```sh
RISC0_DEV_MODE=1 cargo test -p integration_tests --test candidate_plans_payment --test candidate_plans_redemption
```

See the [comparison index](../README.md) for the branch layout, shared inputs and limitations.
