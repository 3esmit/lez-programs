# Payment composition without Token changes

Run the real public-actor examples:

```bash
RISC0_DEV_MODE=1 cargo test -p integration_tests --test token_composition
```

The payer starts with 1,000 tokens. The fixed operation pays 60 to a beneficiary,
burns 10, and leaves or returns 30 of a 100-token budget. The final payer balance
is 930, the beneficiary holds 60, and supply is 990. These examples use the
existing Token Program and the pinned LEZ runtime without modifications.

## Three execution paths

| Command | Funding and authority | Context | Failure boundary |
| --- | --- | --- | --- |
| `Direct` | Payer credential in the execution transaction | Fixed, approved parameters | All application effects of this transaction roll back |
| `Prefunded` | Vault funded in a previous transaction; execute without payer credential | Approved configuration and one-use phase | Prior deposit remains; execution effects roll back |
| `WithPolicy` | Same separate funding; no vault grant passed to policy | `AwaitingPolicy` stores context; exact policy actor must reply | Invalid or missing reply rejects execution; prior deposit remains |

`Direct` emits typed Token commands from the processor. It still needs this small
program to compose the calls under one root. It is not a client transaction with
several arbitrary roots. The client can calculate the amounts, but the processor
executes only the approved instructions. The unused 30 never moves: there is no
gross deposit or refund event, and only 70 tokens are required at execution.

`Prefunded` spends 100 from its own PDA, pays 60, burns 10, and returns 30. The
owner initializes immutable configuration; anyone can trigger the approved
distribution once. Recovery requires the processor owner's authorization and
can transfer funds only to the configured payer. Recovery can also collect later
donations. A second successful distribution is rejected.

`WithPolicy` calls an ordinary external actor, validates its proposed pay and
burn against the configured limits, and then issues the same settlement. The
policy can write its own state: this is cooperative calculation, not enforced
purity. The example omits the payer credential and passes no vault seed to the
policy. Including the payer credential would remove that protection. The policy
identity is the complete account/program pair.

The shared `settle` function demonstrates library reuse across these modes. A
helper can reduce repeated code, but it does not remove authorization checks or
the context needed across invocations.

## What the checks establish

Tests execute actual Token and example guests in development mode. They cover
successful distribution, reduced balance requirements for direct execution,
missing payer authorization, late rollback, recovery, one-use execution,
excessive quotes, attempted wallet spending and missing policy replies.

All participating actors in these examples are public and declared. Token
credits retain their normal asynchronous semantics; this example does not add
a general immediate-delivery guarantee for private recipients. Application
rollback does not promise reversal of runtime fees or nonces in production.
Development mode does not validate production proof generation.

These fixed amounts demonstrate authorization and settlement. For a separate
example whose result depends on reserve balances and current supply, see
[proportional redemption](../redemption/README.md).
