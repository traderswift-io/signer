use crate::aster_intent_fields::{allowed_read_path, ASTER_COUNTDOWN_CANCEL_ALL_PATH, ASTER_ORDER_PATH};
use crate::hyperliquid_action::keccak256;
use crate::order_intent::{parse_positive_decimal_string, OrderIntent};

const ASTER_DOMAIN_CHAIN_ID: u64 = 1666;
const DOMAIN_TYPE: &[u8] = b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";
const MESSAGE_TYPE: &[u8] = b"Message(string msg)";
const ASTER_DOMAIN_NAME: &[u8] = b"AsterSignTransaction";
const ASTER_DOMAIN_VERSION: &[u8] = b"1";
const MAXIMUM_SYMBOL_LENGTH: usize = 32;

pub struct AsterRequest {
    pub method: &'static str,
    pub path: &'static str,
    pub signed_query: String,
    pub nonce: u64,
    pub digest: [u8; 32],
}

fn aster_domain_separator() -> [u8; 32] {
    let mut chain_id_word = [0u8; 32];
    chain_id_word[24..].copy_from_slice(&ASTER_DOMAIN_CHAIN_ID.to_be_bytes());
    let mut encoded = Vec::with_capacity(32 * 5);
    encoded.extend_from_slice(&keccak256(DOMAIN_TYPE));
    encoded.extend_from_slice(&keccak256(ASTER_DOMAIN_NAME));
    encoded.extend_from_slice(&keccak256(ASTER_DOMAIN_VERSION));
    encoded.extend_from_slice(&chain_id_word);
    encoded.extend_from_slice(&[0u8; 32]);
    keccak256(&encoded)
}

// Same bytes as the Aster signing code that has placed orders on the venue since 16 Oct.
pub fn aster_message_digest(signed_query: &str) -> [u8; 32] {
    let mut message_preimage = Vec::with_capacity(64);
    message_preimage.extend_from_slice(&keccak256(MESSAGE_TYPE));
    message_preimage.extend_from_slice(&keccak256(signed_query.as_bytes()));
    let message_struct_hash = keccak256(&message_preimage);
    let mut typed_data = Vec::with_capacity(66);
    typed_data.extend_from_slice(b"\x19\x01");
    typed_data.extend_from_slice(&aster_domain_separator());
    typed_data.extend_from_slice(&message_struct_hash);
    keccak256(&typed_data)
}

fn checked_symbol(intent: &OrderIntent) -> Result<String, String> {
    let symbol = intent.symbol.as_deref().unwrap_or_default();
    let well_formed = !symbol.is_empty()
        && symbol.len() <= MAXIMUM_SYMBOL_LENGTH
        && symbol.chars().all(|character| character.is_ascii_uppercase() || character.is_ascii_digit());
    if !well_formed {
        return Err("bad_symbol".to_string());
    }
    Ok(symbol.to_string())
}

fn checked_decimal(field_name: &str, value: Option<&str>) -> Result<String, String> {
    let text = value.unwrap_or_default();
    parse_positive_decimal_string(text).ok_or(format!("bad_{field_name}"))?;
    Ok(text.to_string())
}

fn checked_address(field_name: &str, address: &str) -> Result<String, String> {
    let well_formed = address.len() == 42
        && address.starts_with("0x")
        && address[2..].chars().all(|character| matches!(character, '0'..='9' | 'a'..='f'));
    if !well_formed {
        return Err(format!("bad_{field_name}"));
    }
    Ok(address.to_string())
}

// The pinned order of each kind's parameters, before nonce, user and signer. Every value is checked to be letters,
// digits, dot or 0x hex, so Python's urlencode leaves it as it is and the string below is what urlencode would build.
fn kind_parameters(intent: &OrderIntent) -> Result<(&'static str, &'static str, Vec<(&'static str, String)>), String> {
    match intent.kind.as_str() {
        "order" => {
            let mut parameters = vec![("symbol", checked_symbol(intent)?)];
            let side = match intent.side.as_deref() {
                Some(side @ ("BUY" | "SELL")) => side.to_string(),
                _ => return Err("bad_side".to_string()),
            };
            parameters.push(("side", side));
            match intent.order_type.as_deref() {
                Some("LIMIT") => {
                    let time_in_force = match intent.time_in_force.as_deref() {
                        Some(time_in_force @ ("GTC" | "IOC" | "FOK" | "GTX")) => time_in_force.to_string(),
                        _ => return Err("bad_time_in_force".to_string()),
                    };
                    parameters.push(("type", "LIMIT".to_string()));
                    parameters.push(("timeInForce", time_in_force));
                    parameters.push(("quantity", checked_decimal("quantity", intent.quantity.as_deref())?));
                    parameters.push(("price", checked_decimal("price", intent.price.as_deref())?));
                }
                Some("MARKET") => {
                    parameters.push(("type", "MARKET".to_string()));
                    parameters.push(("quantity", checked_decimal("quantity", intent.quantity.as_deref())?));
                }
                _ => return Err("bad_type".to_string()),
            }
            if intent.reduce_only == Some(true) {
                parameters.push(("reduceOnly", "true".to_string()));
            }
            Ok(("POST", ASTER_ORDER_PATH, parameters))
        }
        "cancel" => {
            let order_id = intent.order_id.ok_or("cancel_missing_order_id")?;
            Ok(("DELETE", ASTER_ORDER_PATH, vec![("symbol", checked_symbol(intent)?), ("orderId", order_id.to_string())]))
        }
        "countdown_cancel_all" => {
            let countdown_time = intent.countdown_time.ok_or("countdown_cancel_all_missing_countdown_time")?;
            Ok(("POST", ASTER_COUNTDOWN_CANCEL_ALL_PATH, vec![("symbol", checked_symbol(intent)?), ("countdownTime", countdown_time.to_string())]))
        }
        "read" => {
            let path = allowed_read_path(intent.path.as_deref().unwrap_or_default()).ok_or("read_path_not_allowed")?;
            let mut parameters = Vec::new();
            if intent.symbol.is_some() {
                parameters.push(("symbol", checked_symbol(intent)?));
            }
            if let Some(order_id) = intent.order_id {
                parameters.push(("orderId", order_id.to_string()));
            }
            Ok(("GET", path, parameters))
        }
        _ => Err("refused_non_trading_kind".to_string()),
    }
}

// user is the Aster account (the owner wallet, the policy's vault_address); signer is the agent, the key ref's
// venue_address. The signer node sets the nonce. The sender transmits signed_query byte for byte plus
// "&signature=0x" r s v, never a re-encoding.
pub fn build_aster_request(intent: &OrderIntent, agent_address: &str, aster_nonce: u64) -> Result<AsterRequest, String> {
    let (method, path, mut parameters) = kind_parameters(intent)?;
    parameters.push(("nonce", aster_nonce.to_string()));
    parameters.push(("user", checked_address("aster_user", &intent.vault_address)?));
    parameters.push(("signer", checked_address("aster_signer", agent_address)?));
    let signed_query = parameters.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join("&");
    let digest = aster_message_digest(&signed_query);
    Ok(AsterRequest { method, path, signed_query, nonce: aster_nonce, digest })
}
