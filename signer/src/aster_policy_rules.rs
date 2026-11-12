use crate::aster_intent_fields::ASTER_VENUE;
use crate::order_intent::{parse_positive_decimal_string, OrderIntent};
use crate::policy_reader::ChainPolicy;
use crate::policy_rules::{is_chain_stale, AllowedIntent, ChainFreshness, NewRiskSize};
use rust_decimal::Decimal;

const MAXIMUM_INTENT_LIFETIME_SECONDS: u64 = 120;
const MINIMUM_COUNTDOWN_MILLISECONDS: u64 = 15_000;
const MAXIMUM_COUNTDOWN_MILLISECONDS: u64 = 600_000;
const CLOSE_OUT_SECONDS: u64 = 600;
const MILLISECONDS_PER_SECOND: u64 = 1000;

fn close_out_deadline_seconds(policy: &ChainPolicy) -> u64 {
    let ended_at = if policy.revoked { policy.revoked_at.min(policy.expires_at) } else { policy.expires_at };
    ended_at + CLOSE_OUT_SECONDS
}

// countdownTime 0 stops Aster's dead-man and a long one defeats it, so it must be 15 s to 600 s and end by the deadline.
fn check_countdown_time(intent: &OrderIntent, now_seconds: u64, deadline_seconds: u64) -> Result<(), String> {
    let countdown_time = intent.countdown_time.ok_or("countdown_time_missing")?;
    if !(MINIMUM_COUNTDOWN_MILLISECONDS..=MAXIMUM_COUNTDOWN_MILLISECONDS).contains(&countdown_time) {
        return Err("countdown_time_out_of_range".to_string());
    }
    if now_seconds * MILLISECONDS_PER_SECOND + countdown_time > deadline_seconds * MILLISECONDS_PER_SECOND {
        return Err("countdown_past_close_out_deadline".to_string());
    }
    Ok(())
}

// Aster's agent key cannot be limited to symbols or sizes, so the policy is the only limit: the symbol must be one of
// the policy's assets with its caps, as for Hyperliquid. Same order of rules as policy_rules::evaluate_intent; cancel,
// countdown_cancel_all and read take no risk and stay open through the close-out window.
pub fn evaluate_aster_intent(
    intent: &OrderIntent,
    policy: &ChainPolicy,
    freshness: &ChainFreshness,
    size_spent_today: Decimal,
) -> Result<AllowedIntent, String> {
    if freshness.now_seconds >= intent.expires_at {
        return Err("intent_expired".to_string());
    }
    if intent.expires_at > freshness.now_seconds + MAXIMUM_INTENT_LIFETIME_SECONDS {
        return Err("intent_expiry_too_far".to_string());
    }
    if intent.venue != ASTER_VENUE || !policy.venues.iter().any(|venue| venue == ASTER_VENUE) {
        return Err("venue_not_allowed".to_string());
    }
    if intent.vault_address != policy.vault_address {
        return Err("vault_mismatch".to_string());
    }
    let deadline_seconds = close_out_deadline_seconds(policy);
    if freshness.now_seconds >= deadline_seconds {
        return Err("policy_closed".to_string());
    }
    match intent.kind.as_str() {
        "cancel" | "read" => return Ok(AllowedIntent { new_risk_size: None }),
        "countdown_cancel_all" => {
            check_countdown_time(intent, freshness.now_seconds, deadline_seconds)?;
            return Ok(AllowedIntent { new_risk_size: None });
        }
        "order" => {}
        _ => return Err("refused_non_trading_kind".to_string()),
    }
    if policy.revoked {
        return Err("policy_revoked".to_string());
    }
    if freshness.now_seconds >= policy.expires_at {
        return Err("policy_expired".to_string());
    }
    if intent.policy_epoch != policy.epoch {
        return Err(format!("epoch_mismatch_policy_is_{}", policy.epoch));
    }
    let symbol = intent.symbol.as_deref().unwrap_or_default();
    let asset_cap = policy.asset_caps.iter().find(|asset_cap| asset_cap.asset == symbol).ok_or("asset_not_allowed")?;
    if intent.reduce_only == Some(true) {
        return Ok(AllowedIntent { new_risk_size: None });
    }
    if is_chain_stale(freshness) {
        return Err("chain_stale_new_risk_stopped".to_string());
    }
    let quantity = parse_positive_decimal_string(intent.quantity.as_deref().unwrap_or_default()).ok_or("bad_quantity")?;
    let max_order_size = parse_positive_decimal_string(&asset_cap.max_order_size).ok_or("bad_policy_cap")?;
    let max_daily_size = parse_positive_decimal_string(&asset_cap.max_daily_size).ok_or("bad_policy_cap")?;
    if quantity > max_order_size {
        return Err("over_max_order_size".to_string());
    }
    let size_after_order = size_spent_today.checked_add(quantity).ok_or("over_max_daily_size")?;
    if size_after_order > max_daily_size {
        return Err("over_max_daily_size".to_string());
    }
    Ok(AllowedIntent { new_risk_size: Some(NewRiskSize { asset: symbol.to_string(), size: quantity }) })
}
