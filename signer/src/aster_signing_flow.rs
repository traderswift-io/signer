use crate::aster_action::build_aster_request;
use crate::aster_intent_fields::ASTER_VENUE;
use crate::aster_policy_rules::evaluate_aster_intent;
use crate::chain_view::{check_share_matches_chain, read_chain_view, read_or_load_record, ChainView};
use crate::hyperliquid_action::BuiltAction;
use crate::key_share_store::StoredKeyShare;
use crate::nonce_record::{check_and_reserve, check_venue_nonce_window, persist_record, size_spent_today, KeyNonceRecord};
use crate::order_intent::{OrderIntent, VerifiedIntent};
use crate::signer_state::{now_unix_seconds, SignerState};
use crate::signing_flow::{check_rules_and_nonce, EvaluatedIntent};
use crate::threshold_protocol::EcdsaSignatureWithRecovery;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

// Aster accepts a nonce within 60 s of its own clock. The proposer sets now or one above the highest it signed; a peer
// accepts it only if it is above its own highest and inside this window.
const ASTER_NONCE_MAX_AGE_MICROSECONDS: u64 = 30_000_000;
const ASTER_NONCE_MAX_AHEAD_MICROSECONDS: u64 = 60_000_000;

pub fn now_unix_microseconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_micros() as u64).unwrap_or(0)
}

pub fn is_aster_intent(intent: &OrderIntent) -> bool {
    intent.venue == ASTER_VENUE
}

pub fn merge_aster_nonce(own_nonce: u64, peer_nonce: u64) -> Result<u64, String> {
    if peer_nonce > now_unix_microseconds().saturating_add(ASTER_NONCE_MAX_AHEAD_MICROSECONDS) {
        return Err(format!("claimed aster nonce {peer_nonce} beyond the window"));
    }
    Ok(own_nonce.max(peer_nonce))
}

fn reserve_aster_nonce(record: &KeyNonceRecord, aster_nonce: u64, now_microseconds: u64) -> Result<KeyNonceRecord, String> {
    if aster_nonce <= record.highest_signed_aster_nonce_microseconds {
        return Err(format!("aster_nonce_not_above_highest_signed_{}", record.highest_signed_aster_nonce_microseconds));
    }
    if aster_nonce.saturating_add(ASTER_NONCE_MAX_AGE_MICROSECONDS) < now_microseconds
        || aster_nonce > now_microseconds.saturating_add(ASTER_NONCE_MAX_AHEAD_MICROSECONDS)
    {
        return Err("aster_nonce_outside_window".to_string());
    }
    let mut reserved_record = record.clone();
    reserved_record.highest_signed_aster_nonce_microseconds = aster_nonce;
    Ok(reserved_record)
}

// venue_nonce keeps its Hyperliquid meaning here: the delegate-signed, rising intent number, so a signed intent cannot
// be replayed. The Aster nonce on the wire is a second counter, in microseconds, that only signers set.
async fn check_aster_rules_and_nonce(
    state: &SignerState,
    intent: &OrderIntent,
    chain_view: &ChainView,
    aster_nonce: u64,
    persist: bool,
) -> Result<bool, String> {
    let mut nonce_records = state.nonce_records.lock().await;
    let record = nonce_records.get(&intent.key_ref).ok_or("nonce record missing")?;
    let now_seconds = now_unix_seconds();
    let symbol = intent.symbol.as_deref().unwrap_or_default();
    let allowed = evaluate_aster_intent(
        intent,
        &chain_view.policy,
        &chain_view.freshness,
        size_spent_today(record, &intent.delegate_key, symbol, now_seconds),
    )?;
    check_venue_nonce_window(intent.venue_nonce, now_seconds, state.config.venue_nonce_max_ahead_milliseconds)?;
    let reserved_record =
        check_and_reserve(record, intent.venue_nonce, &intent.delegate_key, allowed.new_risk_size.as_ref(), now_seconds)?;
    let reserved_record = reserve_aster_nonce(&reserved_record, aster_nonce, now_unix_microseconds())?;
    if persist {
        persist_record(&state.config.state_directory, &reserved_record)?;
        nonce_records.insert(intent.key_ref.clone(), reserved_record);
    }
    Ok(allowed.new_risk_size.is_some())
}

pub async fn evaluate_aster_without_reserving(
    state: &SignerState,
    verified_intent: Arc<VerifiedIntent>,
    stored_share: Arc<StoredKeyShare>,
    proposed_aster_nonce: Option<u64>,
) -> Result<EvaluatedIntent, String> {
    let intent = &verified_intent.intent;
    let chain_view = read_chain_view(state, &intent.key_ref, &intent.delegate_key).await?;
    check_share_matches_chain(&stored_share, &chain_view.key_reference)?;
    let record = read_or_load_record(state, &intent.key_ref).await?;
    let aster_nonce = proposed_aster_nonce
        .unwrap_or_else(|| now_unix_microseconds().max(record.highest_signed_aster_nonce_microseconds + 1));
    let request = build_aster_request(intent, &chain_view.key_reference.venue_address, aster_nonce)?;
    check_aster_rules_and_nonce(state, intent, &chain_view, aster_nonce, false).await?;
    let built_action = BuiltAction {
        action: json!({
            "method": request.method,
            "path": request.path,
            "signed_query": request.signed_query,
            "nonce": request.nonce,
        }),
        digest: request.digest,
    };
    Ok(EvaluatedIntent { verified_intent: verified_intent.clone(), built_action, stored_share, chain_view })
}

pub async fn check_rules_and_nonce_for_venue(
    state: &SignerState,
    evaluated: &EvaluatedIntent,
    chain_view: &ChainView,
    persist: bool,
) -> Result<bool, String> {
    let intent = &evaluated.verified_intent.intent;
    if !is_aster_intent(intent) {
        return check_rules_and_nonce(state, intent, chain_view, persist).await;
    }
    let aster_nonce = evaluated.built_action.action["nonce"].as_u64().ok_or("aster_nonce_missing")?;
    check_aster_rules_and_nonce(state, intent, chain_view, aster_nonce, persist).await
}

pub fn add_aster_nonce_to_proposal(proposal_body: &mut Value, evaluated: &EvaluatedIntent) {
    if is_aster_intent(&evaluated.verified_intent.intent) {
        proposal_body["aster_nonce"] = evaluated.built_action.action["nonce"].clone();
    }
}

pub fn add_aster_fields_to_response(response: &mut Value, evaluated: &EvaluatedIntent, signature: &EcdsaSignatureWithRecovery) {
    if !is_aster_intent(&evaluated.verified_intent.intent) {
        return;
    }
    let action = &evaluated.built_action.action;
    response["signed_query"] = action["signed_query"].clone();
    response["nonce"] = action["nonce"].clone();
    response["method"] = action["method"].clone();
    response["path"] = action["path"].clone();
    response["signature"] = json!(format!("0x{}{}{:02x}", signature.r, signature.s, signature.v));
}
