# Internal ledger

Track backed claims and reserves separately. Outputs can be withdrawn immediately or retained as claims; later withdrawal is a separate transaction.

This folder contains both payment and two-asset redemption guests.

Run from the repository root:

```sh
RISC0_DEV_MODE=1 cargo test -p integration_tests --test internal_ledger_payment --test internal_ledger_redemption
```

See the [comparison index](../README.md) for the branch layout, shared inputs and limitations.
