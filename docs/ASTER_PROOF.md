# Signer enforces policy on Aster

On 21 Nov 2026 (04:42–04:46 UTC), Signer placed, closed, rested, cancelled and refused orders on Aster ETHUSDT perpetuals. Every order was signed by Signer's threshold key under a policy written on the signer-1 chain. The test account holds a small amount of throwaway test money.

- Owner (test wallet): `0xfFd281cCaE0CCB05803f154a829FA44FcE882A35`
- Signer's Aster agent (threshold key, perpetuals only, no withdrawal): `0x31de89ac822710e8804f051b811495b1f21e5cdf`
- Policy on signer-1 (key ref `kr-signer-1-aster-test`, epoch 1, set at height 604660): venue `aster`, symbol `ETHUSDT`, at most 0.009 per order and 0.018 a day.

## What happened

| Request | Aster order id | Result |
|---|---|---|
| Buy 0.018, twice the per-order cap | none | Refused by the signers (`over_max_order_size`). Nothing was signed, nothing reached Aster. |
| Buy 0.004, immediate-or-cancel | 18883742171 | Filled |
| Sell 0.004, reduce-only | 18883745784 | Filled, account flat again |
| Buy 0.005 far from the price, post-only | 18883758374 | Rested on the book |
| Cancel 18883758374 | 18883758374 | Cancelled |

Aster records each order on its chain; the hashes it returned are `0x164f41e76d867947df2087267890f226d3c3f05e43922ddf636065fe1733195e` (buy), `0x717fceb33bc37902a844089a4107f159c7757d083581096c34c3eb7e415e189b` (close) and `0xc447931eb58fa827060450e4cd08595fa57cca0795902072f65752bb54d7805f` (cancel).

The test margin went into Aster on BNB Chain in transaction `0x6393e1e1bafe7f7fb368979661f3bde616fe99f78bb739bf2f11d5ee614d2375`.

## How the refusal works

Each signer reads the policy from the chain and builds the Aster request itself before it signs (`signer/src/aster_policy_rules.rs`, `signer/src/aster_signing_flow.rs`). An order outside the policy gets no signature share, so no valid signature exists to send.

The owner can always trade, close, withdraw or remove Signer's agent on Aster directly, without Signer.
