# Bounded calculation policy

Run an owner-configured, bounded arithmetic policy over authenticated inputs. The consumer enforces authorization and output constraints.

This folder contains both payment and two-asset redemption guests.

Run from the repository root:

```sh
RISC0_DEV_MODE=1 cargo test -p integration_tests --test bounded_policy_payment --test bounded_policy_redemption
```

See the [comparison index](../README.md) for the branch layout, shared inputs and limitations.
