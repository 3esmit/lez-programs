# Conditional Token commands

Extend Token with balance/supply assertions and checked transfers/burns. Validate a client quote without ReadState callbacks; stale state fails. Required credits must acknowledge.

This folder contains both payment and two-asset redemption guests.

Run from the repository root:

```sh
RISC0_DEV_MODE=1 cargo test -p integration_tests --test conditional_commands_payment --test conditional_commands_redemption
```

See the [comparison index](../README.md) for the branch layout, shared inputs and limitations.
