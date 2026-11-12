# The Signer chain

Cosmos SDK v0.55.0 app, CometBFT v0.40.0, zero fees, no staking, mint, distribution or gov. Modules: auth, bank, consensus, genutil, upgrade, validatorset, policy. Everything here is Apache-2.0. Contract: `../docs/INTERFACES.md`. Outside validators: `../docs/VALIDATOR_JOIN.md`.

## Files

| Path | What |
|---|---|
| `policylogic/` | Pure rules, no SDK imports: field formats, policy validation, owner message texts, EIP-191 and ed25519 checks. The signer is the one authority on intents, so nothing here evaluates them. Depends on `x/crypto/sha3` and `dcrec/secp256k1` only |
| `policy/` | The `policy` module: keeper, message servers, query server, REST routes, genesis. `policy.pb.go` is generated |
| `validatorset/` | The validator set: admin, validators, the one-third rule, updates to CometBFT. `validatorset.pb.go` is generated |
| `proto/` | `policy.proto`, `validatorset.proto`. Regenerate with `generate_proto.sh` (needs `buf` and `protoc-gen-gocosmos` on the PATH) |
| `main.go`, `app.go`, `export.go` | Node binary and wiring, written from the SDK's Apache-2.0 `simapp`. The ante handler is the SDK's stock one |
| `run_local_network.sh` | Local validators on `127.0.0.1` |

## Validator set

An admin address, set in genesis, adds, re-weights and removes validators, and schedules upgrades. It can be a multisig account: the chain only checks that the tx signer is that address. CometBFT gets the changes at the end of the block and applies them two blocks later.

| Message | Who | Refused when |
|---|---|---|
| `AddValidator` (`consensus_public_key`, `power`, `moniker`, `founding`) | admin | key is not 32-byte ed25519 in standard base64, key already in the set, power outside 1 to 10^12, moniker empty or over 70 bytes |
| `SetValidatorPower` | admin | validator unknown, power out of range |
| `RemoveValidator` | admin | validator unknown, set would be empty |
| `RequestJoin` | anyone, from their own node | open join not active, proof over chain id, requester, node id and consensus key does not verify, key, account or endpoint IP already has a validator, outsider count cap reached, outsider power would reach the liveness limit, rejoin cooldown not over |
| `SetOutsiderCountCap` | admin | |
| `ScheduleUpgrade`, `CancelUpgrade` | admin | upgrade info has no checksummed `linux/amd64` binary |
| `SetAdmin` (`new_admin`) | admin | before the open-join activation height, address invalid, already the admin |
| `AcceptAdmin` | the pending admin | no rotation pending, signer is not the pending address |

**Admin change, two steps.** The admin proposes a new address with `SetAdmin`; the new address takes over only when it signs `AcceptAdmin`. Until then the old admin keeps every power, upgrades included, so a mistyped address cannot lose the upgrade path. A new `SetAdmin` replaces a pending one.

**Open join.** From its activation height (block 1 on `signer-1`) anyone may join with `RequestJoin`. A joiner gets power 100,000, at most 100 outsiders, and outsider power stays under the liveness limit: the chain must keep committing with its largest founder down and every outsider down. A joiner that misses 300 blocks is removed and may rejoin after 24 hours.

**First contact.** On a zero-fee chain only existing accounts may send a tx. Once open join is active, an address with no account may send one tx that holds only `RequestJoin`, with sequence 0; the ante handler creates its account, at most 20 per block.

**Upgrades.** `ScheduleUpgrade` sets a name and a height. At that height the chain halts until the nodes run the binary named for it. The nodes on `signer-1` do not run a binary manager that downloads one; each operator installs it.

**One-third rule.** After every change, and in genesis, each validator with `founding` false must hold less than one third of the total power (`3 x power < total`). A change that breaks it is refused, including a re-weight up and the removal of a founder. Several outsiders may together pass a third; one may not. At most 150 validators.

Genesis: `app_state.validatorset = {admin, validators: [{consensus_public_key, power, moniker, founding}]}`; power is a string. `genesis validate` checks the admin address, each key, duplicates, and the rule. Export writes the same shape plus CometBFT's validator list.

Queries: `signerd query validatorset admin | validators | validator --consensus-public-key <key> | joiners | open-join`.

## Build and run

```
go build -o build/signerd .
./run_local_network.sh [--directory path] up [validator_count]
./run_local_network.sh status
./run_local_network.sh stop 2
./run_local_network.sh start 2
./run_local_network.sh down
./run_local_network.sh wipe
```

Node `n` listens on `46000 + 10n`: `+0` p2p, `+1` CometBFT RPC (`/status`), `+2` REST (`/signer/policy/v1/...`), `+3` gRPC, `+4` pprof. RPC and REST are different ports. The keyring is `<directory>/client`, backend `test`, with `admin` (validator-set admin), `relay`, `outsider`, `signer-1..3`, `operator<n>`. Keys are throwaway.

## Quorum

CometBFT commits only with **more than** two thirds of voting power. Three equal validators therefore need all three: stopping one halts the chain. No power split fixes this, since every pair would need over two thirds of the total. Four validators tolerate one down.

## Who may do what

| Message | Tx signer (`submitter`) | Proof inside the message |
|---|---|---|
| `RegisterSigner` (added) | validator-set admin | |
| `RegisterAccount` | any existing chain account | |
| `ClaimOwner` | any existing chain account | owner wallet EIP-191 and registration key ed25519, both over the claim text |
| `RegisterKeyRef` | any existing chain account | owner wallet EIP-191; committee members must be registered signers; threshold at least 2 |
| `SetPolicy`, `RevokePolicy` | any existing chain account | owner wallet EIP-191 with the next epoch, so a captured signature cannot be replayed |
| `AllowRelease` | validator-set admin | activates at tx height plus `release_activation_delay_blocks` (genesis, default 100) |
| `RecordDecision` | the registered signer's own address, matching `signer_id` | |
| `AnchorTape` | a registered signer or the validator-set admin | |

Only accounts that exist on chain can send a tx, which is the spam limit on a zero-fee chain. New ones are created in genesis or by a bank send of any amount.

## Epochs

One counter per `key_ref`. Every `SetPolicy` and `RevokePolicy` must carry counter plus one. A `Policy` stores the counter value at its own last change, so with two delegates on a key_ref one policy's epoch can jump by more than one. The next value is `policy_epoch` on the keyref REST response.

## Texts the owner wallet signs

EIP-191 `personal_sign`, 65-byte signature as 130 lowercase hex (`v` 27, 28, 0 or 1). Line breaks are `\n`, no trailing newline.

Claim, SIWE form. The address is EIP-55 here even though it is lowercase in the message field. Times are UTC RFC 3339 from the `issued_at` and `expires_at` fields. The registration key signs the same text with ed25519.

```
signer wants you to sign in with your Ethereum account:
<EIP-55 owner address>

Claim signer account <account_id> with registration key <registration_key>

URI: signer://<chain id>/claim
Version: 1
Chain ID: 1
Nonce: <8 to 64 letters or digits>
Issued At: <issued_at>
Expiration Time: <expires_at>
```

```
signer register key ref
chain id: <chain id>
key ref: <key_ref>
account id: <account_id>
venue address: <0x lowercase>
committee: <id,id,id>
threshold: <n>
```

```
signer set policy
chain id: <chain id>
key ref: <key_ref>
delegate key: <hex>
epoch: <n>
venues: <a,b>
vault address: <0x lowercase or the word none>
asset <name>: max order size <decimal>, max daily size <decimal>
daily size is per delegate and counts signed size, not fills, including failed rounds
expires at: <unix seconds>
```

```
signer revoke policy
chain id: <chain id>
key ref: <key_ref>
delegate key: <hex>
epoch: <n>
```

One `asset` line per entry of `asset_caps`, in the order given. At least one is required; `SetPolicy` refuses an empty list, a repeated asset, and `max_order_size` above `max_daily_size`. It also refuses a vault that differs from another policy on the same key_ref, and any cap with a leading zero or more than 16 digits before the point or 12 after. `RevokePolicy` stores the block time as `revoked_at`; a second revoke does not move it, and `SetPolicy` clears it. On the autocli, `--asset-caps` takes one JSON object per flag.

## REST

`GET /signer/policy/v1/policy/{key_ref}/{delegate_key}` returns the Policy fields flat, with `asset_caps` a list of objects. `GET /signer/policy/v1/keyref/{key_ref}` returns the KeyRef fields flat plus `policy_epoch`. `epoch`, `expires_at`, `revoked_at`, `threshold` and `policy_epoch` are JSON numbers; empty lists are `[]`; missing returns 404 with `{"error": ...}`. Other queries (`account`, `signer`, `release`, `log-entries`, `anchor`) are gRPC and `signerd query policy`.

## Not on the chain yet

Unclaimed-account expiry, owner rotation, committee change and signer removal are not built.

## Measured, 8 Nov 2026

Three validators on one machine, with API and gRPC on, 1 s commit timeout, about 1.2 s per block. Memory per node was 105 to 108 MB after start and 149 to 155 MB after 10 minutes and about 380 `RecordDecision` transactions, still rising by about 1 MB per minute at the end. A 10-minute run does not establish a steady state. Disk was 6.5 MB per node at height 499. The binary is 115 MB.
