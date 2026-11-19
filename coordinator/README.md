# signer coordinator

One per key. Takes a delegate-signed intent, asks a signer, checks the answer, posts to the venue. Contract: `../docs/INTERFACES.md`.

**Live venue: off by default; the operator turns it on.** `venue_poster.py` refuses any URL but plain HTTP on loopback. The one exception is exactly `https://api.hyperliquid.xyz/exchange`, and only with `allow_live_hyperliquid_exchange: true` in the config. Only the operator sets it. The optional websocket route is in `venue_websocket_poster.py`.

```
cd signer
python3 -m coordinator.coordinator_service coordinator-config.json
```

Config is the fields of `CoordinatorConfig`: `listen_host`, `listen_port`, `key_ref`, `signer_urls`, `chain_rest_urls`, `venue_exchange_url`, `dead_man_window_seconds` (30 to 600, default 300), `intent_lifetime_seconds` (at most 120, the signers refuse beyond that), `allow_live_hyperliquid_exchange` (default false), for Base `base_rpc_url` and `allow_live_base_broadcast` (default false: the signed transaction is returned, not sent), for the hub owner check `hub_rest_urls`, `hub_websocket_url` and `hub_account_id` (all empty by default, which turns the check off), `venue_websocket_url` (empty by default, which turns the websocket route off) and `hold_state_file` (when unset, `coordinator_holds.json` next to the config file). The check is in `hub_owner_check.py`, and the rule is in `docs/INTERFACES.md`.

| File | What |
|---|---|
| `coordinator_service.py` | HTTP routes, config, start |
| `coordinator_rounds.py` | `CoordinatorConfig`, round state, prepare, nonce, dead-man, one signing round and its checks |
| `signer_client.py` | `ask_signers`: asks signers in order, moves on only for a dead connection or a local refusal |
| `chain_reader.py` | `read_venue_address` and `read_policy` from REST |
| `venue_digest.py` | Own build of the action and digest, signature recovery. Follows Hyperliquid's published signing scheme |
| `venue_poster.py` | The URL check and the HTTP call to the venue |
| `aster_rounds.py` | Aster: prepare with a `countdown_cancel_all` re-arm, one round, its checks. Returns the signed query; nothing here posts it |
| `base_transaction_digest.py` | Base: own build of the EIP-1559 transaction and its signing hash from the intent, raw signed bytes, sender recovery. Mirrors the signer's adapters; see `../docs/BASE_VENUE.md` |
| `base_rounds.py` | Base: `prepare_base_round` with the chain nonce from `base_rpc_url`, `run_base_signing_round` and its checks, broadcast only with `allow_live_base_broadcast`, and only to a `base_rpc_url` that starts `https://`; otherwise nothing is sent |
| `delegate_client.py` | What a bot imports: `run_client_round` checks `rearm_intent` before signing, signs through a function it is given, re-prepares once on a stale nonce |
| `hub_owner_check.py` | The hub owner check: reads the signer's owner address and the hub's answer, caches the hub answer, follows the hub websocket and its `network.claimed` events, gives the refusal reason |
| `account_holds.py` | Holds a vault after a post whose answer was lost: load and save the hold state file, mark and settle a lost nonce, `release_account_hold` removes a hold when the delegate calls `/account-state-known`, on the delegate's word |
| `venue_websocket_poster.py` | The websocket route to the venue: its URL check, the shared socket, posting with a deadline, and the fallback to `venue_poster.py` |

It holds no key. The delegate signs `venue_nonce` too, so a client calls `/prepare` for the nonce and any re-arm intent, signs, then calls `/intent`.
