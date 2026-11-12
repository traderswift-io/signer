# Signer

TraderSwift Signer holds a trading key that no one holds. It is one of the subnets of TraderSwift, next to timeseries, execution and context.

Signer continues the Lockbox Milestone A test, archived at https://github.com/PythonForForex/lockbox-chain.

## TraderSwift

TraderSwift is a trading network. Agents and people use it to trade across exchanges. It is built from subnets, and each subnet runs one job on its own set of nodes.

## What Signer is

- A trading key is held as shares by a committee of three signers. Any 2 of the 3 sign together; one alone cannot. In the design, the signers generate the key jointly, so it never exists whole.
- An owner writes a policy to a small chain, `signer-1`: which venue account the key may trade, which order kinds, and the size caps.
- Each signer builds the venue message itself from the order request and checks it against the policy on chain. It never signs a hash it did not build. Transfers and withdrawals are always refused. Every decision, signed or refused, is recorded on the chain.
- Signing is interactive today.

| Folder | What | Language |
|---|---|---|
| `chain/` | The chain, `signerd`: Cosmos SDK with a `policy` module and a `validatorset` module | Go |
| `signer/` | The signer node, `signer-node`: threshold ECDSA on `cggmp21` | Rust |
| `coordinator/` | Takes an order request, collects the signature, sends it to the venue. Holds no key share | Python |
| `dealer/` | One-time import of an existing key as shares | Rust |
| `hubanchor/` | Daily state anchor onto the TraderSwift hub | Python |
| `networks/signer-1/` | Genesis, its hash, and the peers | |

Words: **Signer** is the product. A **signer node** is the Rust program. **s1, s2, s3** are the three signer machines.

## Status

`signer-1` started on 12 Nov 2026 at 04:19:52 UTC. It runs one key, on a test account, with test money. What was tested before launch is in the release `signer-1-gates` of this repository. The launch record follows as a release after the first live orders.

## Run a validator

Anyone can run a validator and join from their own node. Nobody has to be asked: [docs/VALIDATOR_JOIN.md](docs/VALIDATOR_JOIN.md). An agent can follow [SKILL.md](SKILL.md). How the pieces talk to each other: [docs/INTERFACES.md](docs/INTERFACES.md).

## Free and open source

Apache License 2.0. Run it as your own chain with your own signers if you like. TraderSwift runs it as one of its subnets.
