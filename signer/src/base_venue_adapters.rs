use crate::base_abi_words::{
    address_from_hex, argument_word, bool_argument, calldata_from_words, refuse_unexpected_arguments, selector_of_calldata,
    unsigned_argument, word_as_address, word_as_unsigned, word_from_address, word_from_bool, word_from_unsigned, SELECTOR_BYTES,
    WORD_BYTES,
};
use rust_decimal::Decimal;
use std::collections::BTreeMap;

pub struct BaseAllowedCall {
    pub contract: [u8; 20],
    pub selector: [u8; 4],
    pub required_value_wei: u128,
}

pub struct BaseApproveCap {
    pub token: [u8; 20],
    pub spender: [u8; 20],
    pub maximum_amount: u128,
}

pub struct BaseCallRequest<'a> {
    pub kind: &'a str,
    pub trader: [u8; 20],
    pub arguments: &'a BTreeMap<String, String>,
}

pub struct BaseNewRisk {
    pub asset: String,
    pub size: Decimal,
}

pub struct BaseVenueCall {
    pub to: [u8; 20],
    pub data: Vec<u8>,
    pub value_wei: u128,
    pub new_risk: Option<BaseNewRisk>,
}

// The adapter builds the calldata from intent fields, and check_call then decodes the bytes again on its own, so a
// builder bug cannot reach the round. The generic guards in base_policy_guards run after both.
pub struct BaseVenueAdapter {
    pub venue: &'static str,
    pub allowed_calls: &'static [BaseAllowedCall],
    pub venue_contracts: &'static [[u8; 20]],
    pub approve_caps: &'static [BaseApproveCap],
    pub build_call: fn(&BaseCallRequest) -> Result<BaseVenueCall, String>,
    pub check_call: fn(&BaseCallRequest, &BaseVenueCall) -> Result<(), String>,
}

pub const ERC20_APPROVE_SELECTOR: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];

const EXAMPLE_VENUE_CONTRACT: [u8; 20] = address_from_hex("0x00000000000000000000000000000000000e0001");
const EXAMPLE_COLLATERAL_TOKEN: [u8; 20] = address_from_hex("0x00000000000000000000000000000000000e0002");
const EXAMPLE_PLACE_ORDER_SELECTOR: [u8; 4] = [0xcc, 0x88, 0x13, 0x5d];
const EXAMPLE_APPROVE_CAP: u128 = 1_000_000_000;
const EXAMPLE_PLACE_ORDER_ARGUMENTS: [&str; 4] = ["market_id", "is_buy", "size", "limit_price"];
const EXAMPLE_SIZE_SCALE: u32 = 6;

const AVANTIS_TRADING_ROUTER: [u8; 20] = address_from_hex("0x44914408af82bc9983bbb330e3578e1105e11d4e");
const AVANTIS_DELEGATED_ACTION_SELECTOR: [u8; 4] = [0x9a, 0x10, 0xcc, 0x32];
const AVANTIS_OPEN_TRADE_SELECTOR: [u8; 4] = [0x19, 0xcd, 0xe9, 0xa1];
const AVANTIS_CLOSE_TRADE_MARKET_SELECTOR: [u8; 4] = [0xe5, 0xa9, 0x7c, 0x92];
const AVANTIS_CANCEL_OPEN_LIMIT_ORDER_SELECTOR: [u8; 4] = [0xb9, 0xb6, 0x57, 0x3a];
const AVANTIS_OPEN_TRADE_WORDS: usize = 13;
const AVANTIS_CLOSE_TRADE_MARKET_WORDS: usize = 4;
const AVANTIS_CANCEL_OPEN_LIMIT_ORDER_WORDS: usize = 2;
const AVANTIS_OPEN_ORDER_TYPES: [u128; 3] = [0, 1, 2];
const AVANTIS_USDC_SCALE: u32 = 6;
const AVANTIS_LEVERAGE_SCALE: u32 = 10;
const AVANTIS_OPEN_TRADE_ARGUMENTS: [&str; 9] =
    ["pair_index", "trade_index", "collateral_usdc", "open_price", "is_long", "leverage", "take_profit", "stop_loss", "order_type"];
const AVANTIS_OPEN_TRADE_SLIPPAGE_ARGUMENT: &str = "slippage";
const AVANTIS_CLOSE_TRADE_MARKET_ARGUMENTS: [&str; 4] = ["pair_index", "trade_index", "collateral_to_close_usdc", "wanted_price"];
const AVANTIS_CANCEL_OPEN_LIMIT_ORDER_ARGUMENTS: [&str; 2] = ["pair_index", "trade_index"];

fn build_example_call(request: &BaseCallRequest) -> Result<BaseVenueCall, String> {
    match request.kind {
        "place_order" => {
            refuse_unexpected_arguments(request.arguments, &EXAMPLE_PLACE_ORDER_ARGUMENTS)?;
            let market_id = unsigned_argument(request.arguments, "market_id")?;
            let size = unsigned_argument(request.arguments, "size")?;
            if size == 0 {
                return Err("bad_size".to_string());
            }
            let words = [
                word_from_unsigned(market_id),
                word_from_bool(bool_argument(request.arguments, "is_buy")?),
                word_from_unsigned(size),
                word_from_unsigned(unsigned_argument(request.arguments, "limit_price")?),
            ];
            Ok(BaseVenueCall {
                to: EXAMPLE_VENUE_CONTRACT,
                data: calldata_from_words(EXAMPLE_PLACE_ORDER_SELECTOR, &words),
                value_wei: 0,
                new_risk: Some(BaseNewRisk { asset: format!("example:{market_id}"), size: Decimal::from_i128_with_scale(size as i128, EXAMPLE_SIZE_SCALE) }),
            })
        }
        "approve_collateral" => {
            refuse_unexpected_arguments(request.arguments, &["amount"])?;
            let words = [word_from_address(&EXAMPLE_VENUE_CONTRACT), word_from_unsigned(unsigned_argument(request.arguments, "amount")?)];
            Ok(BaseVenueCall {
                to: EXAMPLE_COLLATERAL_TOKEN,
                data: calldata_from_words(ERC20_APPROVE_SELECTOR, &words),
                value_wei: 0,
                new_risk: None,
            })
        }
        _ => Err("unknown_kind".to_string()),
    }
}

fn check_example_call(request: &BaseCallRequest, call: &BaseVenueCall) -> Result<(), String> {
    let expected_words = match request.kind {
        "place_order" => 4,
        "approve_collateral" => 2,
        _ => return Err("unknown_kind".to_string()),
    };
    if call.data.len() != SELECTOR_BYTES + expected_words * WORD_BYTES {
        return Err("calldata_length_differs".to_string());
    }
    Ok(())
}

fn avantis_open_trade_inner(request: &BaseCallRequest) -> Result<(Vec<u8>, BaseNewRisk), String> {
    let mut expected_names = AVANTIS_OPEN_TRADE_ARGUMENTS.to_vec();
    expected_names.push(AVANTIS_OPEN_TRADE_SLIPPAGE_ARGUMENT);
    refuse_unexpected_arguments(request.arguments, &expected_names)?;
    let arguments = request.arguments;
    let pair_index = unsigned_argument(arguments, "pair_index")?;
    let collateral_usdc = unsigned_argument(arguments, "collateral_usdc")?;
    let leverage = unsigned_argument(arguments, "leverage")?;
    let order_type = unsigned_argument(arguments, "order_type")?;
    if collateral_usdc == 0 || leverage == 0 {
        return Err("bad_collateral_or_leverage".to_string());
    }
    if !AVANTIS_OPEN_ORDER_TYPES.contains(&order_type) {
        return Err("bad_order_type".to_string());
    }
    let words = [
        word_from_address(&request.trader),
        word_from_unsigned(pair_index),
        word_from_unsigned(unsigned_argument(arguments, "trade_index")?),
        word_from_unsigned(0),
        word_from_unsigned(collateral_usdc),
        word_from_unsigned(unsigned_argument(arguments, "open_price")?),
        word_from_bool(bool_argument(arguments, "is_long")?),
        word_from_unsigned(leverage),
        word_from_unsigned(unsigned_argument(arguments, "take_profit")?),
        word_from_unsigned(unsigned_argument(arguments, "stop_loss")?),
        word_from_unsigned(0),
        word_from_unsigned(order_type),
        word_from_unsigned(unsigned_argument(arguments, AVANTIS_OPEN_TRADE_SLIPPAGE_ARGUMENT)?),
    ];
    let collateral = Decimal::from_i128_with_scale(collateral_usdc as i128, AVANTIS_USDC_SCALE);
    let leverage_multiple = Decimal::from_i128_with_scale(leverage as i128, AVANTIS_LEVERAGE_SCALE);
    let notional = collateral.checked_mul(leverage_multiple).ok_or("bad_collateral_or_leverage")?;
    let new_risk = BaseNewRisk { asset: format!("avantis:{pair_index}"), size: notional.normalize() };
    Ok((calldata_from_words(AVANTIS_OPEN_TRADE_SELECTOR, &words), new_risk))
}

fn avantis_plain_inner(request: &BaseCallRequest, selector: [u8; 4], names: &[&str]) -> Result<Vec<u8>, String> {
    refuse_unexpected_arguments(request.arguments, names)?;
    let words = names.iter().map(|name| unsigned_argument(request.arguments, name).map(word_from_unsigned)).collect::<Result<Vec<_>, _>>()?;
    Ok(calldata_from_words(selector, &words))
}

fn wrap_in_delegated_action(trader: &[u8; 20], inner_calldata: &[u8]) -> Vec<u8> {
    let padded_length = inner_calldata.len().div_ceil(WORD_BYTES) * WORD_BYTES;
    let mut calldata = calldata_from_words(
        AVANTIS_DELEGATED_ACTION_SELECTOR,
        &[word_from_address(trader), word_from_unsigned(2 * WORD_BYTES as u128), word_from_unsigned(inner_calldata.len() as u128)],
    );
    calldata.extend_from_slice(inner_calldata);
    calldata.resize(SELECTOR_BYTES + 3 * WORD_BYTES + padded_length, 0);
    calldata
}

fn build_avantis_call(request: &BaseCallRequest) -> Result<BaseVenueCall, String> {
    let (inner_calldata, new_risk) = match request.kind {
        "open_trade" => {
            let (inner_calldata, new_risk) = avantis_open_trade_inner(request)?;
            (inner_calldata, Some(new_risk))
        }
        "close_trade_market" => {
            (avantis_plain_inner(request, AVANTIS_CLOSE_TRADE_MARKET_SELECTOR, &AVANTIS_CLOSE_TRADE_MARKET_ARGUMENTS)?, None)
        }
        "cancel_open_limit_order" => {
            (avantis_plain_inner(request, AVANTIS_CANCEL_OPEN_LIMIT_ORDER_SELECTOR, &AVANTIS_CANCEL_OPEN_LIMIT_ORDER_ARGUMENTS)?, None)
        }
        _ => return Err("unknown_kind".to_string()),
    };
    Ok(BaseVenueCall { to: AVANTIS_TRADING_ROUTER, data: wrap_in_delegated_action(&request.trader, &inner_calldata), value_wei: 0, new_risk })
}

// delegatedAction runs any inner calldata as the trader (DelegateModule has no selector filter): setDelegate,
// removeDelegate, updateMargin withdraw and a nested delegatedAction would all go through. Only the three inner
// calls below pass, each with its exact length, and the trader words must be the policy's trader.
fn check_avantis_call(request: &BaseCallRequest, call: &BaseVenueCall) -> Result<(), String> {
    if selector_of_calldata(&call.data) != Some(AVANTIS_DELEGATED_ACTION_SELECTOR) {
        return Err("outer_call_not_delegated_action".to_string());
    }
    if word_as_address(&argument_word(&call.data, 0)?)? != request.trader {
        return Err("delegated_action_trader_differs".to_string());
    }
    if word_as_unsigned(&argument_word(&call.data, 1)?)? != 2 * WORD_BYTES as u128 {
        return Err("delegated_action_offset_differs".to_string());
    }
    let inner_length = word_as_unsigned(&argument_word(&call.data, 2)?)? as usize;
    let inner_start = SELECTOR_BYTES + 3 * WORD_BYTES;
    let inner_calldata = call.data.get(inner_start..inner_start + inner_length).ok_or("calldata_too_short")?;
    if call.data.len() != inner_start + inner_length.div_ceil(WORD_BYTES) * WORD_BYTES
        || call.data[inner_start + inner_length..].iter().any(|byte| *byte != 0)
    {
        return Err("delegated_action_padding_differs".to_string());
    }
    let inner_selector = selector_of_calldata(inner_calldata).ok_or("inner_call_empty")?;
    let expected_words = match (request.kind, inner_selector) {
        ("open_trade", AVANTIS_OPEN_TRADE_SELECTOR) => AVANTIS_OPEN_TRADE_WORDS,
        ("close_trade_market", AVANTIS_CLOSE_TRADE_MARKET_SELECTOR) => AVANTIS_CLOSE_TRADE_MARKET_WORDS,
        ("cancel_open_limit_order", AVANTIS_CANCEL_OPEN_LIMIT_ORDER_SELECTOR) => AVANTIS_CANCEL_OPEN_LIMIT_ORDER_WORDS,
        _ => return Err("inner_selector_not_allowlisted".to_string()),
    };
    if inner_length != SELECTOR_BYTES + expected_words * WORD_BYTES {
        return Err("inner_calldata_length_differs".to_string());
    }
    if inner_selector == AVANTIS_OPEN_TRADE_SELECTOR && word_as_address(&argument_word(inner_calldata, 0)?)? != request.trader {
        return Err("open_trade_trader_differs".to_string());
    }
    Ok(())
}

static BASE_VENUE_ADAPTERS: [BaseVenueAdapter; 2] = [
    BaseVenueAdapter {
        venue: "base_example",
        allowed_calls: &[
            BaseAllowedCall { contract: EXAMPLE_VENUE_CONTRACT, selector: EXAMPLE_PLACE_ORDER_SELECTOR, required_value_wei: 0 },
            BaseAllowedCall { contract: EXAMPLE_COLLATERAL_TOKEN, selector: ERC20_APPROVE_SELECTOR, required_value_wei: 0 },
        ],
        venue_contracts: &[EXAMPLE_VENUE_CONTRACT],
        approve_caps: &[BaseApproveCap { token: EXAMPLE_COLLATERAL_TOKEN, spender: EXAMPLE_VENUE_CONTRACT, maximum_amount: EXAMPLE_APPROVE_CAP }],
        build_call: build_example_call,
        check_call: check_example_call,
    },
    BaseVenueAdapter {
        venue: "base_avantis",
        allowed_calls: &[BaseAllowedCall { contract: AVANTIS_TRADING_ROUTER, selector: AVANTIS_DELEGATED_ACTION_SELECTOR, required_value_wei: 0 }],
        venue_contracts: &[AVANTIS_TRADING_ROUTER],
        approve_caps: &[],
        build_call: build_avantis_call,
        check_call: check_avantis_call,
    },
];

pub fn lookup_base_venue_adapter(venue: &str) -> Option<&'static BaseVenueAdapter> {
    BASE_VENUE_ADAPTERS.iter().find(|adapter| adapter.venue == venue)
}
