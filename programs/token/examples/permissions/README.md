# Typed batches and bounded spending permissions

These examples modify the Token Program only. They use the pinned LEZ runtime
without new protocol features.

```bash
RISC0_DEV_MODE=1 cargo test -p integration_tests --test token_permissions
```

## Independent batch effects

An owner sends `Message::Batch` to its holding. The Token checks authorization,
limits the list to 16 payments, checks the total, debits once, and produces typed
credits and a supply reduction. No arbitrary Token `Internal` bytes are accepted.

The example debits 100, pays 60, credits 30 back to the source, and burns 10.
It ends at payer 930, beneficiary 60, supply 990. There is no intermediate
protocol vault. The initial holding must cover the gross debit, including a
self-payment that will return later.

The effects are independent. The runtime executes the synchronous supply
reduction before promoted asynchronous credits. This is not an ordered script
for dependent actions. Credits retain existing Token delivery semantics,
including possible deferred delivery to private recipients. Public declared
recipients in these tests execute immediately; a failing recipient rolls back
the debit and burn in that transaction.

## Permission installed by the holding owner

In one transaction, the owner sends `ApproveSpending` with:

- Exact spender account and program deployment.
- Fixed destination, cumulative total budget and separate burn budget.
- Exclusive expiration timestamp in LEZ timestamp units.
- Initial spend nonce zero. Token assigns a new approval generation.

The source and token definition are fixed by the holding that stores the
permission. The example approves a total of 100 and a burn limit of 10.

In a later transaction, the spender calls `Spend` with generation, nonce,
payment and burn amounts. Token authenticates the full caller actor, validates
and consumes both budgets, advances the nonce, checks available balance, and
emits the effects. LEZ enforces the returned expiration window. The example
pays 60 and burns 10; 30 of the allowance remains and 30 of the original budget
never leaves the wallet.

The execution transaction signs for the spender's controller, **not the source
holding**. It must omit both the source's broad credential and any equivalent
PDA authorization. An allowance is not a runtime sandbox. A test explicitly
shows that including the owner credential permits an ordinary transfer that
bypasses the quantitative allowance.

The permission is cumulative. A fresh nonce permits another partial spend
within the remaining limits. Replaying a consumed nonce fails. Owner revocation
removes the permission; replacement advances the generation so an old approval
cannot become valid again. Failed application execution restores balances,
supply, budgets and nonce together. The separate earlier approval remains.

## Compatibility and limits

Existing enum tags and payload layouts remain unchanged. Approval opts a holding
into the appended `PermittedHolding` state variant. Old readers that understand
only `Holding` and `Definition` must be updated before using that holding.
Revocation preserves the new state and generation history. Ordinary owner
transfers, burns, self-credits and idempotent initialization preserve metadata.

There is one active permission per holding in this prototype. Deployment identity
does not pin an immutable ImageID: a spender program upgrade changes the code
trusted within the existing permission. Use appropriate deployment immutability
or revoke/reapprove around an upgrade. Signed off-chain permits and private
sender aliases are outside this example's tested surface.

Tests cover batch success/failure/overflow, cumulative limits, burn limits,
expiry, replay, full caller identity, ordinary-transfer bypass attempts,
revocation, replacement, late rollback and preservation of permission metadata.
They execute real guests in development mode and public declared actors. They
do not establish production proof validity or benchmark performance.
