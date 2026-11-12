# Run a Signer validator: https://github.com/traderswift-io/signer

Instructions for an agent (a Grok bot, or any agent with a Linux shell) to run a validator on the Signer chain for its user. Follow them in order. Every step runs as an ordinary user: no sudo and no systemd needed.

**Tell your user this first, in these words:** "I will run a Signer node on this machine. Once it has caught up with the chain, it will ask to join the validator set automatically, unless you tell me not to. As a validator it signs blocks. It holds its own signing key on this machine and nothing else: no money, and no key of anyone else's." If the user says no, or says "full node only", do every step except step 7.

## What this is, and what it is not

- The node keeps a full copy of the Signer chain and, once joined, signs blocks with its own consensus key.
- It never holds money, a wallet seed, or any key except the two its own `init` makes.
- Joining is decided by the chain's rules, not by a person. Every joiner gets the same small power. Outside validators together stay small enough that the chain keeps making blocks even with every one of them down and its largest founder down too, so no group of them can halt or rewrite the chain.
- A joiner that misses 300 blocks in a row is removed automatically. It can ask again after one day. So run the node only where it can stay up.

## Rules for you, the agent

- Never print, copy, upload or post `config/priv_validator_key.json` or `config/node_key.json`. They stay on this disk. Your user's backups are their own business.
- Run exactly one copy of the node with these keys. Two copies with the same `priv_validator_key.json` sign twice, and the chain treats that as misbehaviour.
- On a machine shared with other agents, keep everything under one folder that is yours, and tell your user that other agents on the machine can read the keys.
- If a step fails, stop, show your user the last 30 lines of the log, and do not improvise a workaround on the key files.

## 1. Check the machine

```
nproc; free -h; df -h .; go version
```

You need Linux x86_64, 1 CPU, 2 GB RAM and 20 GB free disk, and any Go 1.21 or newer: the build pins Go 1.26.5 with `GOTOOLCHAIN=go1.26.5` and fetches it. Build with exactly that version. A newer Go makes the binary print a warning on every command, and the warning breaks cosmovisor's upgrade check. The node needs outgoing connections to port 26656; an open incoming port is better but not required.

## 2. Build

```
export SIGNER="$PWD/signer-validator"
mkdir -p "$SIGNER/bin"
git clone https://github.com/traderswift-io/signer.git "$SIGNER/src"
cd "$SIGNER/src/chain" && GOTOOLCHAIN=go1.26.5 go build -trimpath -o "$SIGNER/bin/signerd" . && cd -
GOTOOLCHAIN=go1.26.5 GOBIN="$SIGNER/bin" go install cosmossdk.io/tools/cosmovisor/cmd/cosmovisor@v1.7.3
sha256sum "$SIGNER/bin/signerd"
```

## 3. Make the node's keys

```
"$SIGNER/bin/signerd" init <moniker> --chain-id signer-1 --home "$SIGNER/node-home"
```

Pick a short public `<moniker>`, for example your user's handle plus `-grok`. This writes the two key files. Do not show them.

## 4. Genesis and peers

```
cd "$SIGNER/src/networks/signer-1"
sha256sum -c genesis.sha256                      # must print: genesis.json: OK
cp genesis.json "$SIGNER/node-home/config/genesis.json"
PEERS="$(paste -sd, peers.txt)"
cd -
sed -i "s#^persistent_peers = .*#persistent_peers = \"$PEERS\"#" "$SIGNER/node-home/config/config.toml"
sed -i 's#^minimum-gas-prices = .*#minimum-gas-prices = "0usigner"#' "$SIGNER/node-home/config/app.toml"
```

Leave RPC on `127.0.0.1:26657`, as `init` wrote it. If the machine has a public IP with port 26656 open, also set `external_address = "tcp://<public ip>:26656"` under `[p2p]`.

## 5. Start under cosmovisor

cosmovisor runs `signerd` and switches it by itself when the chain schedules an upgrade. It manages the chain binary only.

```
export DAEMON_HOME="$SIGNER/node-home" DAEMON_NAME=signerd
export DAEMON_ALLOW_DOWNLOAD_BINARIES=true DAEMON_DOWNLOAD_MUST_HAVE_CHECKSUM=true DAEMON_RESTART_AFTER_UPGRADE=true
"$SIGNER/bin/cosmovisor" init "$SIGNER/bin/signerd"
nohup "$SIGNER/bin/cosmovisor" run start --home "$SIGNER/node-home" > "$SIGNER/node.log" 2>&1 &
echo $! > "$SIGNER/node.pid"
```

If the machine has systemd and you may use it, a unit is better than `nohup`; `docs/VALIDATOR_JOIN.md` section 6 has one. If your process does not outlive your turn, tell your user: a validator must stay up.

## 6. Wait until caught up

```
curl -s 127.0.0.1:26657/status | jq '.result.sync_info | {latest_block_height, catching_up}'
curl -s 127.0.0.1:26657/net_info | jq .result.n_peers
```

Wait until `catching_up` is `false` and `n_peers` is at least 1. Check again a minute later that the height is still moving.

## 7. Ask to join

Skip this step if your user said no.

```
"$SIGNER/bin/signerd" keys add joiner --keyring-backend test --home "$SIGNER/node-home"
"$SIGNER/bin/signerd" tx validatorset request-join \
  --moniker <moniker> --endpoint-ip <this machine's public IP> \
  --from joiner --keyring-backend test --home "$SIGNER/node-home" \
  --chain-id signer-1 --node tcp://127.0.0.1:26657 --yes -o json
```

- `<this machine's public IP>` is the address the internet sees, for example from `curl -s https://ifconfig.me`. The chain takes one validator per IP and one per account.
- The command signs the request with the node's consensus key to prove it is yours. The key never leaves the machine. The account key `joiner` holds no coins and needs none.

Read the result a few seconds later:

```
"$SIGNER/bin/signerd" query tx <txhash> --node tcp://127.0.0.1:26657 -o json | jq '.events[] | select(.type | startswith("validatorset."))'
```

- `validatorset.join_request_accepted`: you are in from the next block. Confirm with `signerd query validatorset joiners --node tcp://127.0.0.1:26657`.
- `validatorset.join_request_refused` with a `reason`: nothing was queued. Tell your user the reason. `open_join_not_active_on_this_chain` means the chain route has not started yet; `endpoint_ip_already_has_a_validator` means another node on this IP already joined; `outsider_count_cap_*` or `outsider_power_*_would_reach_liveness_limit_*` means the outside seats are full for now; `rejoin_cooldown_until_<unix time>` means wait until then.

## 8. Keep it healthy

Report to your user, now and whenever they ask:

| Check | Command | Healthy |
|---|---|---|
| Height moving | `curl -s 127.0.0.1:26657/status \| jq -r .result.sync_info.latest_block_height` | Higher each time |
| Peers | `curl -s 127.0.0.1:26657/net_info \| jq .result.n_peers` | 1 or more |
| Still a validator | `signerd query validatorset joiners --node tcp://127.0.0.1:26657` | Your moniker listed |
| Process | `kill -0 "$(cat "$SIGNER/node.pid")"` | No error |

To stop: `kill "$(cat "$SIGNER/node.pid")"`. After 300 missed blocks the chain removes the validator by itself.

The full operator guide is `docs/VALIDATOR_JOIN.md`. Questions go to https://github.com/traderswift-io/signer/issues.
