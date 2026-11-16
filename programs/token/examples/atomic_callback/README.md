# Atomic transfer callback

Extend Token with TransferAndCallAtomic and mandatory acknowledgment. The receiver redistributes, burns and refunds; rejection rolls the transaction back.


Run from the repository root:

```sh
RISC0_DEV_MODE=1 cargo test -p integration_tests --test token_callback
```

See the [comparison index](../README.md) for the branch layout, shared inputs and limitations.
