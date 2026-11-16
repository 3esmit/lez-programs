# Actor workflow

The consumer keeps Pending, Settling and completion state, authenticates Token replies, and checks settlement.


Run from the repository root:

```sh
RISC0_DEV_MODE=1 cargo test -p integration_tests --test token_workflow
```

See the [comparison index](../README.md) for the branch layout, shared inputs and limitations.
