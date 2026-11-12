# Running a Signer validator

For an outside operator. You run one node that produces blocks on the Signer chain. Everything you need is in this repository. You can clone, configure, start, sync and join without contacting us: your node asks the chain to add it, and the chain adds it by rule (section 7). An agent can do all of this from [`SKILL.md`](../SKILL.md).

The network files are in [`networks/signer-1/`](../networks/signer-1/): chain ID, `genesis.json`, its SHA-256 and the peer list.

## What you get, and what you never get

| You get | You never get |
|---|---|
| A vote in block production, at the fixed power the chain gives every joiner | A key share, a signer role, or any part of a venue key |
| A full copy of the chain: accounts, policies, signer decisions, release hashes | The power to change policy, signers, releases or the validator set |
| Your node's own keys, made and kept on your machine | Any secret of ours, or any of our machines' keys |

Your node checks and orders transactions. It cannot sign orders; signers do that, under policies only an owner wallet can change. A validator alone cannot change any of that.

**The liveness limit.** The chain must keep making blocks even with its largest founding validator down and every outside validator down at once. Blocks need more than two thirds of the power, so the founders still up must hold more than two thirds of the total:

```
Fsum − fmax > 2/3 · (Fsum + O)    which is    O < (Fsum − 3·fmax) / 2
```

Here `Fsum` is the founders' total power, `fmax` the largest founder's power and `O` the outside validators' total power after your join. The chain computes this from the live set on every request and refuses a join that would reach the limit.

- Four founders at 10M each: `(40M − 30M) / 2 = 5M`. Outside power stays under 5M: 49 joiners at 100000, fewer while other outside validators hold power (an outside validator added by the admin at 1M leaves room for 39).
- A fifth founder at 10M: `(50M − 30M) / 2 = 10M`, so 99 joiners at 100000, one under the count cap of 100.

`signerd query validatorset open-join` shows `outsider_power` and `outsider_power_limit`. So outside validators, whatever they do together, cannot halt the chain while at most one founder is down, and can never rewrite it. The chain also refuses any set where a single non-founding validator holds one third or more.

## 1. Machine

- Linux x86_64, 1 vCPU, 2 GB RAM, 20 GB disk is enough today. The binary is about 115 MB; a node used about 150 MB of memory after 10 minutes; the chain was 6.5 MB at height 499.
- A public IP, fixed if you can. Our nodes accept p2p on 26656 from anywhere.
- A clock kept by NTP.
- No other service on the machine needs our trust. Keep it to yourself.

## 2. The binary

Either build it from source:

```
git clone https://github.com/traderswift-io/signer.git signer
cd signer/chain
GOTOOLCHAIN=go1.26.5 go build -trimpath -o signerd .     # exactly Go 1.26.5: a newer Go makes every command print a warning that breaks cosmovisor
sha256sum signerd
```

There is no published binary yet, so build from source.

Install it as `/opt/signer/bin/signerd` and run the node as its own user:

```
sudo useradd --system --home /var/lib/signer --create-home signer
sudo install -d -m 0755 /opt/signer/bin
sudo install -m 0755 signerd /opt/signer/bin/signerd
```

## 3. Your keys, made on your machine

```
sudo -u signer /opt/signer/bin/signerd init <your moniker> --chain-id signer-1 --home /var/lib/signer/chain
```

This writes two keys you keep:

| File | What | If lost or copied |
|---|---|---|
| `config/node_key.json` | p2p identity (your node ID) | Make a new one |
| `config/priv_validator_key.json` | consensus key that signs blocks | Copied: open an issue at once and we remove it. Never run two nodes with it: both would sign and the chain sees double signing |

Back up `priv_validator_key.json` offline. Never send it to anyone, us included.

You send us nothing yet. Section 7 says what to send once you are synced.

## 4. Genesis and peers

From the clone, check the hash, then copy genesis in:

```
cd signer/networks/signer-1
sha256sum -c genesis.sha256                     # must print: genesis.json: OK
sudo -u signer cp genesis.json /var/lib/signer/chain/config/genesis.json
paste -sd, peers.txt                            # your persistent_peers value
```

In `config/config.toml`:

```
[p2p]
laddr = "tcp://0.0.0.0:26656"
external_address = "tcp://<your public ip>:26656"
persistent_peers = "<the output of paste -sd, peers.txt>"
pex = false

[rpc]
laddr = "tcp://127.0.0.1:26657"
```

In `config/app.toml`: `minimum-gas-prices = "0usigner"`. Keep `[api]` and `[grpc]` on `127.0.0.1`.

## 5. Sync

**Block sync (default).** Start the node; it replays every block from genesis. The chain is small, so this is minutes.

**State sync (optional, not offered yet).** Only once we publish a read-only RPC address in `networks/signer-1/`. In `config.toml`:

```
[statesync]
enable = true
rpc_servers = "<rpc 1>,<rpc 2>"
trust_height = <height we publish>
trust_hash = "<block hash at that height>"
trust_period = "168h0m0s"
```

Check `trust_hash` yourself against a second source before you start. State sync needs our nodes to take snapshots (`state-sync.snapshot-interval` in their `app.toml`), which we turn on before we offer it.

## 6. Run under cosmovisor

cosmovisor runs the chain binary and switches it by itself when the chain schedules an upgrade (section 10). It manages `signerd` only, nothing else on the machine. Install it and give it the binary you built:

```
go install cosmossdk.io/tools/cosmovisor/cmd/cosmovisor@v1.7.3
sudo install -m 0755 ~/go/bin/cosmovisor /opt/signer/bin/cosmovisor
sudo -u signer env DAEMON_HOME=/var/lib/signer/chain DAEMON_NAME=signerd /opt/signer/bin/cosmovisor init /opt/signer/bin/signerd
```

`/etc/systemd/system/signer-chain.service`:

```
[Unit]
Description=Signer node
After=network-online.target
Wants=network-online.target

[Service]
User=signer
Environment=DAEMON_HOME=/var/lib/signer/chain
Environment=DAEMON_NAME=signerd
Environment=DAEMON_ALLOW_DOWNLOAD_BINARIES=true
Environment=DAEMON_DOWNLOAD_MUST_HAVE_CHECKSUM=true
Environment=DAEMON_RESTART_AFTER_UPGRADE=true
ExecStart=/opt/signer/bin/cosmovisor run start --home /var/lib/signer/chain
Restart=on-failure
RestartSec=3
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

```
sudo systemctl daemon-reload && sudo systemctl enable --now signer-chain.service
curl -s http://127.0.0.1:26657/status | jq .result.sync_info
```

Wait until `catching_up` is `false`, and compare your height with a founding node's if you can. Until you join, you are a full node: you follow the chain and sign nothing.

## 7. Join

### The chain route (first choice)

Make an account key on the machine. It holds no coins and needs none; transactions are free. Then ask to join:

```
sudo -u signer /opt/signer/bin/signerd keys add joiner --keyring-backend test --home /var/lib/signer/chain
sudo -u signer /opt/signer/bin/signerd tx validatorset request-join \
  --moniker <your moniker> --endpoint-ip <your public ip> [--contact <optional, shown publicly>] \
  --from joiner --keyring-backend test --home /var/lib/signer/chain \
  --chain-id signer-1 --node tcp://127.0.0.1:26657 --yes
```

The command reads your node's consensus public key and node ID from `config/`, signs the request with the consensus key to prove the key is yours, and sends it. The key never leaves the machine. Your account is created by this first transaction.

The chain then decides by rule, in the same block, with no person in the loop:

| Check | If it fails |
|---|---|
| The proof is a valid signature by that consensus key | The transaction fails; nothing is stored |
| Your account has no validator yet, and your IP has none | Refused: `account_already_has_a_validator`, `endpoint_ip_already_has_a_validator` |
| You were not removed in the last day | Refused: `rejoin_cooldown_until_<unix time>` |
| Fewer than 100 outside validators (the admin can raise this) | Refused: `outsider_count_cap_<n>_reached` |
| Outside power, with yours, stays under the liveness limit above | Refused: `outsider_power_<O>_would_reach_liveness_limit_<limit>` |

Accepted, you join at the next block with power 100000, the same for every joiner. A refusal is a successful transaction that carries the event `validatorset.join_request_refused` with the reason; nothing is queued, so send again when the reason has gone. Check:

```
signerd query validatorset joiners --node tcp://127.0.0.1:26657
signerd query validatorset open-join --node tcp://127.0.0.1:26657
curl -s http://127.0.0.1:26657/validators | jq '.result.validators[] | {address, voting_power}'
```

**Stay online.** A joiner that misses 300 blocks in a row is removed by the chain at once, and can ask again after one day. Founding validators are never removed this way. Your node keeps running as a full node with its copy of the chain.

The record shows which account owns each joiner, so readers can see when many validators belong to one operator.

The chain route is open on `signer-1` from block 1.

### Fallback: ask a person

Open an issue on https://github.com/traderswift-io/signer/issues titled `validator: <your moniker>`, with the output of:

```
sudo -u signer /opt/signer/bin/signerd comet show-validator --home /var/lib/signer/chain   # consensus public key
sudo -u signer /opt/signer/bin/signerd comet show-node-id --home /var/lib/signer/chain     # node ID
```

plus your public IP. These are public keys and IDs only. Never paste `priv_validator_key.json` or `node_key.json`. The admin then adds you with `add-validator`. The admin is one address fixed in genesis, held by the founders; today a single key, with a founders' multisig planned. The admin can also change a validator's power or remove it; removal takes effect two blocks later.

## 8. Firewall

Open only p2p:

```
table inet filter {
  chain input {
    type filter hook input priority 0; policy drop;
    iif lo accept
    ct state established,related accept
    tcp dport 22 ip saddr <your admin ip> accept
    tcp dport 26656 accept
  }
}
```

RPC (26657), REST (1317), gRPC (9090) and pprof stay on loopback. Use an SSH tunnel to read them from elsewhere.

## 9. Monitoring

| Watch | How | Alert when |
|---|---|---|
| Height moving | `curl -s 127.0.0.1:26657/status \| jq -r .result.sync_info.latest_block_height` | Unchanged for 30 s |
| Catching up | `.result.sync_info.catching_up` | `true` for more than a few minutes |
| Peers | `curl -s 127.0.0.1:26657/net_info \| jq .result.n_peers` | Fewer than 2 |
| Signing | your `address` in `/commit?height=<h>` signatures, `block_id_flag` 2 | Missing for 10 blocks in a row |
| Disk, memory | the usual | Disk above 80% |
| Process | `systemctl is-active signer-chain` | Not active |

CometBFT also serves Prometheus metrics if you set `prometheus = true` under `[instrumentation]` in `config.toml`; keep that port on loopback too.

## 10. Upgrades and leaving

The admin schedules an upgrade on chain with `signerd tx validatorset schedule-upgrade --name <n> --height <h> --info <json>`. The plan carries a download URL with a `sha256` checksum for every platform this guide supports (today `linux/amd64`); the chain refuses a plan without one. At that height every node stops; cosmovisor downloads the new binary, checks the checksum, switches and restarts. You do nothing. Watch the events `validatorset.upgrade_scheduled` and `validatorset.upgrade_cancelled`, or:

```
signerd query upgrade plan --node tcp://127.0.0.1:26657
```

`signer-1` has had its upgrade module from genesis, so a new node syncs from block 1 with the current release and later upgrades switch by themselves.

To leave, stop the node. If you joined by the chain route you are removed after 300 missed blocks; or open an issue and we remove you at once. Keep or delete your copy of the chain as you like; it holds no secret.
