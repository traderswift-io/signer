use crate::order_intent::OrderIntent;

pub const ASTER_VENUE: &str = "aster";
const ASTER_TRADING_KINDS: [&str; 4] = ["order", "cancel", "countdown_cancel_all", "read"];
pub const ASTER_OPEN_ORDERS_PATH: &str = "/fapi/v3/openOrders";
pub const ASTER_ORDER_PATH: &str = "/fapi/v3/order";
pub const ASTER_POSITION_RISK_PATH: &str = "/fapi/v3/positionRisk";
pub const ASTER_BALANCE_PATH: &str = "/fapi/v3/balance";
pub const ASTER_COUNTDOWN_CANCEL_ALL_PATH: &str = "/fapi/v3/countdownCancelAll";

pub fn allowed_read_path(path: &str) -> Option<&'static str> {
    [ASTER_OPEN_ORDERS_PATH, ASTER_ORDER_PATH, ASTER_POSITION_RISK_PATH, ASTER_BALANCE_PATH]
        .into_iter()
        .find(|allowed_path| *allowed_path == path)
}

fn names_of_present_aster_fields(intent: &OrderIntent) -> Vec<&'static str> {
    [
        ("symbol", intent.symbol.is_some()),
        ("side", intent.side.is_some()),
        ("type", intent.order_type.is_some()),
        ("price", intent.price.is_some()),
        ("quantity", intent.quantity.is_some()),
        ("time_in_force", intent.time_in_force.is_some()),
        ("order_id", intent.order_id.is_some()),
        ("countdown_time", intent.countdown_time.is_some()),
        ("path", intent.path.is_some()),
    ]
    .into_iter()
    .filter(|(_, present)| *present)
    .map(|(field_name, _)| field_name)
    .collect()
}

// Before Aster these fields did not exist and serde refused them as unknown; a Hyperliquid intent still refuses them.
pub fn refuse_aster_only_fields(intent: &OrderIntent) -> Result<(), String> {
    match names_of_present_aster_fields(intent).first() {
        Some(field_name) => Err(format!("bad_intent_json: unknown field `{field_name}`")),
        None => Ok(()),
    }
}

fn carries_hyperliquid_fields(intent: &OrderIntent) -> bool {
    intent.asset_index.is_some()
        || intent.asset.is_some()
        || intent.is_buy.is_some()
        || intent.limit_px.is_some()
        || intent.size.is_some()
        || intent.tif.is_some()
        || intent.oid.is_some()
        || intent.time_ms.is_some()
}

fn require_exactly(intent: &OrderIntent, kind_name: &str, required: &[&str], optional: &[&str]) -> Result<(), String> {
    let present = names_of_present_aster_fields(intent);
    if let Some(missing) = required.iter().find(|field_name| !present.contains(field_name)) {
        return Err(format!("{kind_name}_missing_{missing}"));
    }
    if let Some(extra) = present.iter().find(|field_name| !required.contains(field_name) && !optional.contains(field_name)) {
        return Err(format!("{kind_name}_carries_{extra}"));
    }
    Ok(())
}

// The agent key can do anything the account allows short of withdrawal, so every kind outside the four trading kinds is
// refused here, by name, before any rule or digest: withdraw, transfer, approve, sub-account and anything else.
// vault_address is the Aster user (the owner wallet); the policy pins it, so it cannot be empty.
pub fn refuse_aster_fields_of_other_kinds(intent: &OrderIntent) -> Result<(), String> {
    if !ASTER_TRADING_KINDS.contains(&intent.kind.as_str()) {
        return Err("refused_non_trading_kind".to_string());
    }
    if intent.vault_address.is_empty() {
        return Err("aster_user_missing".to_string());
    }
    if carries_hyperliquid_fields(intent) {
        return Err("aster_intent_carries_hyperliquid_fields".to_string());
    }
    match intent.kind.as_str() {
        "order" => {
            if intent.reduce_only.is_none() {
                return Err("order_missing_reduce_only".to_string());
            }
            match intent.order_type.as_deref() {
                Some("LIMIT") => require_exactly(intent, "order", &["symbol", "side", "type", "quantity", "price", "time_in_force"], &[]),
                Some("MARKET") => require_exactly(intent, "order", &["symbol", "side", "type", "quantity"], &[]),
                _ => Err("bad_type".to_string()),
            }
        }
        "cancel" | "countdown_cancel_all" | "read" if intent.reduce_only.is_some() => Err(format!("{}_carries_reduce_only", intent.kind)),
        "cancel" => require_exactly(intent, "cancel", &["symbol", "order_id"], &[]),
        "countdown_cancel_all" => require_exactly(intent, "countdown_cancel_all", &["symbol", "countdown_time"], &[]),
        _ => match intent.path.as_deref() {
            Some(ASTER_ORDER_PATH) => require_exactly(intent, "read", &["path", "symbol", "order_id"], &[]),
            Some(ASTER_OPEN_ORDERS_PATH) | Some(ASTER_POSITION_RISK_PATH) => require_exactly(intent, "read", &["path"], &["symbol"]),
            Some(ASTER_BALANCE_PATH) => require_exactly(intent, "read", &["path"], &[]),
            _ => Err("read_path_not_allowed".to_string()),
        },
    }
}
