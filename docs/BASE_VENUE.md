# Base venues (chainId 8453)

On Base a trade is an ordinary EIP-1559 (type 2) transaction from the threshold key's own address, which pays the gas. The signer builds the whole transaction from intent fields, checks it, and signs `keccak256(0x02 || rlp([chainId, nonce, maxPriorityFeePerGas, maxFeePerGas, gasLimit, to, value, data, []]))`. It never accepts calldata from the coordinator. Hyperliquid code is untouched; Base is reached through one dispatch line in `http_api.rs` on `venue` starting with `base_`.

## Intent

```json
{"request_id":"uuid","key_ref":"kr-base","delegate_key":"...","policy_epoch":1,"venue":"base_avantis",
 "vault_address":"0x<trader wallet>","kind":"open_trade","chain_nonce":7,"max_priority_fee_per_gas":1000000,
 "max_fee_per_gas":20000000,"gas_limit":900000,"expires_at":1794000030,
 "arguments":{"pair_index":"0","trade_index":"0","collateral_usdc":"100000000","open_price":"35000000000000",
              "is_long":"true","leverage":"100000000000","take_profit":"0","stop_loss":"0","order_type":"0","slippage":"10000000000"},
 "delegate_signature":"..."}
```

The delegate signs the same canonical bytes as for Hyperliquid. `vault_address` is the trader, the account the venue trades for, and it must equal the policy's `vault_address`. `arguments` values are strings: plain unsigned decimals (no leading zero, at most u128) or `true`/`false`; an argument the adapter does not name is refused.

## Adapter interface (`signer/src/base_venue_adapters.rs`)

A venue is one entry in `BASE_VENUE_ADAPTERS`:

| Field | What |
|---|---|
| `venue` | Name, `base_` prefix, as listed in the policy's `venues` |
| `allowed_calls` | (contract, selector, exact `value` in wei) the transaction may make. Value is 0 unless an entry says otherwise |
| `venue_contracts` | The venue's own contracts: the only ERC-20 transfer recipients and approve spenders |
| `approve_caps` | (token, spender, maximum amount) for each approve the venue needs |
| `build_call(kind, trader, arguments)` | Builds `to`, `data`, `value` and the new risk (asset name and size in policy units), or refuses `unknown_kind` |
| `check_call` | Decodes the built calldata again on its own, before the generic guards |

The coordinator's mirror is `BASE_CALL_BUILDERS` in `coordinator/base_transaction_digest.py`; both must build the same bytes.

## Guards (`signer/src/base_policy_guards.rs`, in this order)

- `chain_id` 8453 only; `to` not zero (`contract_creation_refused`); empty data is a native send (`native_send_refused`).
- ERC-20: `transfer` only to a venue contract; `approve` only to a venue contract, up to the declared cap; `transferFrom`, `increaseAllowance`, `decreaseAllowance`, `permit` always refused.
- (to, selector) must be in `allowed_calls` (`call_not_allowlisted`), and `value` must equal the entry's (`value_not_allowed`).
- Gas: `max_fee_per_gas <= max_fee_per_gas_ceiling`, `gas_limit <= gas_limit_ceiling` (signer config `base_transaction_limits`; without it every Base intent is refused `base_not_configured`), priority fee not above max fee, gas limit at least 21000.
- Nonce: the coordinator supplies the chain nonce (`eth_getTransactionCount` pending). Each signer refuses a nonce not above the highest it has signed for that address (`chain_nonce_not_above_highest_signed_N`), recorded in `base_nonce_records/` with fsync before the round.
- Policy (`base_signing_flow.rs`): expiry at most 120 s, venue in the policy, trader equals the policy vault, close-out deadline. A call with new risk also needs a live policy, the epoch, a fresh chain, its asset in the caps, and order and daily size within them. Closes and cancels stay open until the close-out deadline, as on Hyperliquid.

## Avantis (`base_avantis`)

Read from the verified source (Sourcify exact match), 12 Nov:
- TradingRouter proxy `0x44914408af82bc9983bbb330e3578e1105e11d4e`; EIP-1967 slot points at implementation `0x4a0d70ec00b708879530eeeaef2bb161551fdadc`. `delegateModule()` is `0xe6d7df4006fe2f4bf220aa804d603c0c35b6c206`.
- The trader registers the delegate (our threshold address) once with `setDelegate(address _delegate, uint256 _expiry)` (`0xd6a510d9`, expiry in unix seconds) or `setDelegateWithSig`. That is the owner's step, not ours.
- The delegate sends `delegatedAction(address trader, bytes call_data)` (`0x9a10cc32`). DelegateModule checks the delegation and then `delegatecall`s **any** inner calldata as the trader: it has no selector filter. So the adapter decodes the inner call and allows only:
  - `openTrade((address,uint256,uint256,uint256,uint256,uint256,bool,uint256,uint256,uint256,uint256),uint8,uint256)` `0x19cde9a1`, with `Trade.trader` equal to the trader, `initialPosToken` 0, `timestamp` 0, `order_type` 0 market, 1 stop limit or 2 limit
  - `closeTradeMarket(uint256,uint256,uint256,uint256)` `0xe5a97c92`
  - `cancelOpenLimitOrder(uint256,uint256)` `0xb9b6573a`
- Refused, among others: `setDelegate`, `removeDelegate`, `updateMargin` (its withdraw type), a nested `delegatedAction`, any direct call to the router, any approve (the trader approves USDC himself).
- Units: price, leverage and slippage 1e10, USDC 1e6. The new risk is notional USDC, collateral times leverage, under asset `avantis:<pair_index>`.
- `openTrade` is payable. The tx-builder's `/v2/meta` reports `executionFeeWei` 0, so value stays 0. If the venue ever charges a fee in ETH, the entry's `required_value_wei` must change.
- Not covered: there is no dead-man on Avantis. `updateOpenLimitOrder`, TP/SL and builder codes are left out.

`base_example` is a placeholder used only by the checks. One contract and selector (`placeOrder(uint256,bool,uint256,uint256)`) plus one capped approve, at addresses where nothing is deployed.
