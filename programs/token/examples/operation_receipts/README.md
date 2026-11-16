# Operation receipts

Fund a unique escrow in one transaction, then execute an immutable request in another. Execution failure preserves the earlier deposit; cancellation and expiry provide recovery.

This folder contains both payment and two-asset redemption guests.

Run from the repository root:

```sh
RISC0_DEV_MODE=1 cargo test -p integration_tests --test operation_receipts_payment --test operation_receipts_redemption
```

See the [comparison index](../README.md) for the branch layout, shared inputs and limitations.
