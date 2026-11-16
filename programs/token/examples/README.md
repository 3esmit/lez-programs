# Token composition examples

Each approach has a separate folder and unique crate, guest and test names. All folders on this branch build together.

## Branch layout

```text
refactor/lez-v0.4.x at 4c0b8c7d
└── examples/token-composition                 unchanged Token
    ├── examples/token-atomic-callback         atomic callback extension
    ├── examples/token-conditional-commands    conditional command extension
    └── examples/token-batch-permissions       batch/permission extension
```

Each extension is an independent child of the shared examples branch. Every child retains all shared examples and tests. The extensions are alternatives; they are not combined. All branches retain the same pinned LEZ runtime dependency.

## Examples available on this branch

| Folder | Approach | Integration targets |
| --- | --- | --- |
| [actor_workflow](actor_workflow/) | Actor workflow | `token_workflow` |
| [composition](composition/) | Payment composition | `token_composition` |
| [redemption](redemption/) | Proportional redemption | `token_redemption` |
| [candidate_plans](candidate_plans/) | Client-proposed plans | `candidate_plans_payment`, `candidate_plans_redemption` |
| [bounded_policy](bounded_policy/) | Bounded calculation policy | `bounded_policy_payment`, `bounded_policy_redemption` |
| [script_coordinator](script_coordinator/) | Reusable continuation coordinator | `script_coordinator_payment`, `script_coordinator_redemption` |
| [internal_ledger](internal_ledger/) | Internal ledger | `internal_ledger_payment`, `internal_ledger_redemption` |
| [operation_receipts](operation_receipts/) | Operation receipts | `operation_receipts_payment`, `operation_receipts_redemption` |
| [permissions](permissions/) | Batch and restricted spending permissions | `token_permissions` |

## Shared scenarios

The simple payment starts with 1,000 tokens. The operation allocates 100: pay 60 to a beneficiary, burn 10, and return 30. The external-settlement result is payer 930, beneficiary 60, empty vault, and supply 990.

The complex scenario redeems 100 shares against reserves of 1,000 A and 2,000 B and a share supply of 1,000. The proportional result is 100 A and 200 B. Tests also cover changed reserves/supply, user minimums, unauthorized calls, and rollback where applicable. The callback and original actor-workflow examples demonstrate the simple payment; the other approaches retain their existing scenario coverage.

Ledger and receipt modes deliberately change custody or transaction boundaries. Retained ledger claims are not external balances. An escrow funded in an earlier transaction does not roll back when later execution fails. Consult the individual tests before comparing final balances.

These are comparison prototypes using public accounts and real zkVM guests in development mode. They do not establish production-proof behavior, private-flow support, or performance improvements.

## Run and validate

Run all shared and branch-specific tests from the repository root:

```sh
RISC0_DEV_MODE=1 cargo test --workspace
```

Run a single target using the name in the table:

```sh
RISC0_DEV_MODE=1 cargo test -p integration_tests --test token_redemption
```

Use a different `CARGO_TARGET_DIR` for each branch/worktree so embedded Token guests cannot be confused across variants. `cargo clippy --workspace` does not compile excluded guest crates; validate each example's `guest/Cargo.toml` too.

## Original comparison branches

The original snapshots remain available as `feat/token-atomic-callback`, `feat/token-actor-workflow`, `feat/token-composition-examples`, `feat/token-batch-permissions`, `feat/token-candidate-plans`, `feat/token-conditional-commands`, `feat/token-bounded-policy`, `feat/token-script-coordinator`, `feat/token-internal-ledger`, and `feat/token-operation-receipts`.
