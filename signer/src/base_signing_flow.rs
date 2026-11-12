use crate::base_abi_words::{address_text, parse_address_text};
use crate::base_nonce_record::{base_record_lock, base_size_spent_today, load_base_record, persist_base_record, reserve_base_nonce};
use crate::base_transaction::{raw_signed_transaction, transaction_hash, Eip1559Transaction};
use crate::base_transaction_request::{build_base_transaction, BaseIntent, BuiltBaseTransaction};
use crate::chain_view::{check_share_matches_chain, read_chain_view, ChainView};
use crate::key_share_store::StoredKeyShare;
use crate::order_intent::{canonical_intent_bytes, is_valid_identifier, parse_positive_decimal_string, sha256_hex, OrderIntent, VerifiedIntent};
use crate::peer_signing_handlers::{participants_in_keygen_order, sweep_abandoned_sessions};
use crate::peer_transport::call_peer;
use crate::policy_reader::ChainPolicy;
use crate::policy_rules::{is_chain_stale, ChainFreshness};
use crate::round_delivery::build_session_inbox;
use crate::signer_state::{lookup_key_share, now_unix_seconds, register_round_inbox, remove_round_inbox, SignerState, SigningSession};
use crate::signing_flow::{record_outcome, record_refusal_of_raw};
use crate::signing_round::{run_signing_round, SigningRound};
use ed25519_dalek::{Signature, VerifyingKey};
use futures::stream::{FuturesUnordered, StreamExt};
use rand_core::{OsRng, RngCore};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

const PROPOSAL_TIMEOUT_MILLISECONDS: u64 = 5000;
const BEGIN_TIMEOUT_MILLISECONDS: u64 = 3000;
const MAXIMUM_INTENT_LIFETIME_SECONDS: u64 = 120;
const CLOSE_OUT_SECONDS: u64 = 600;

pub struct EvaluatedBaseIntent {
    pub verified_intent: Arc<VerifiedIntent>,
    pub intent: BaseIntent,
    pub built: BuiltBaseTransaction,
    pub stored_share: Arc<StoredKeyShare>,
    pub chain_view: ChainView,
}

fn is_lowercase_hex_of_length(value: &str, length: usize) -> bool {
    value.len() == length && value.chars().all(|character| matches!(character, '0'..='9' | 'a'..='f'))
}

fn verify_base_delegate_signature(raw_intent: &Value, intent: &BaseIntent) -> Result<Vec<u8>, String> {
    let canonical_bytes = canonical_intent_bytes(raw_intent)?;
    let key_bytes: [u8; 32] = hex::decode(&intent.delegate_key).map_err(|_| "bad_delegate_key")?.try_into().map_err(|_| "bad_delegate_key")?;
    let signature_bytes: [u8; 64] =
        hex::decode(&intent.delegate_signature).map_err(|_| "bad_delegate_signature")?.try_into().map_err(|_| "bad_delegate_signature")?;
    let verifying_key = VerifyingKey::from_bytes(&key_bytes).map_err(|_| "bad_delegate_key")?;
    verifying_key.verify_strict(&canonical_bytes, &Signature::from_bytes(&signature_bytes)).map_err(|_| "bad_delegate_signature")?;
    Ok(canonical_bytes)
}

// The shared session table and the decision log take an OrderIntent; this one carries only what they read.
fn decision_record_view(intent: &BaseIntent) -> OrderIntent {
    OrderIntent {
        request_id: intent.request_id.clone(),
        key_ref: intent.key_ref.clone(),
        delegate_key: intent.delegate_key.clone(),
        policy_epoch: intent.policy_epoch,
        venue: intent.venue.clone(),
        vault_address: intent.vault_address.clone(),
        kind: intent.kind.clone(),
        venue_nonce: intent.chain_nonce,
        expires_at: intent.expires_at,
        delegate_signature: intent.delegate_signature.clone(),
        asset_index: None,
        asset: None,
        is_buy: None,
        limit_px: None,
        size: None,
        reduce_only: None,
        tif: None,
        oid: None,
        time_ms: None,
        symbol: None,
        side: None,
        order_type: None,
        price: None,
        quantity: None,
        time_in_force: None,
        order_id: None,
        countdown_time: None,
        path: None,
    }
}

pub fn parse_and_verify_base_intent(raw_intent: &Value) -> Result<(BaseIntent, Arc<VerifiedIntent>), String> {
    let intent: BaseIntent = serde_json::from_value(raw_intent.clone()).map_err(|error| format!("bad_intent_json: {error}"))?;
    if !is_valid_identifier(&intent.request_id) || !is_valid_identifier(&intent.key_ref) {
        return Err("bad_intent_json: request_id or key_ref format".to_string());
    }
    if !is_lowercase_hex_of_length(&intent.delegate_key, 64) || !is_lowercase_hex_of_length(&intent.delegate_signature, 128) {
        return Err("bad_intent_json: delegate_key or delegate_signature format".to_string());
    }
    parse_address_text(&intent.vault_address).map_err(|_| "bad_vault_address")?;
    let canonical_bytes = verify_base_delegate_signature(raw_intent, &intent)?;
    let verified_intent = Arc::new(VerifiedIntent { intent: decision_record_view(&intent), intent_hash: sha256_hex(&canonical_bytes) });
    Ok((intent, verified_intent))
}

fn close_out_deadline_seconds(policy: &ChainPolicy) -> u64 {
    let ended_at = if policy.revoked { policy.revoked_at.min(policy.expires_at) } else { policy.expires_at };
    ended_at + CLOSE_OUT_SECONDS
}

// Same shape as policy_rules for Hyperliquid: a call that takes no new risk (close, cancel) stays open until the
// close-out deadline and while the chain is stale; an open needs a live policy, the epoch, a fresh chain and the caps.
pub fn evaluate_base_policy(
    intent: &BaseIntent,
    built: &BuiltBaseTransaction,
    policy: &ChainPolicy,
    freshness: &ChainFreshness,
    size_spent_today: Decimal,
) -> Result<(), String> {
    if freshness.now_seconds >= intent.expires_at {
        return Err("intent_expired".to_string());
    }
    if intent.expires_at > freshness.now_seconds + MAXIMUM_INTENT_LIFETIME_SECONDS {
        return Err("intent_expiry_too_far".to_string());
    }
    if !policy.venues.iter().any(|venue| *venue == intent.venue) {
        return Err("venue_not_allowed".to_string());
    }
    if intent.vault_address != policy.vault_address {
        return Err("vault_mismatch".to_string());
    }
    if freshness.now_seconds >= close_out_deadline_seconds(policy) {
        return Err("policy_closed".to_string());
    }
    let Some(new_risk) = &built.new_risk else {
        return Ok(());
    };
    if policy.revoked {
        return Err("policy_revoked".to_string());
    }
    if freshness.now_seconds >= policy.expires_at {
        return Err("policy_expired".to_string());
    }
    if intent.policy_epoch != policy.epoch {
        return Err(format!("epoch_mismatch_policy_is_{}", policy.epoch));
    }
    let asset_cap = policy.asset_caps.iter().find(|asset_cap| asset_cap.asset == new_risk.asset).ok_or("asset_not_allowed")?;
    if is_chain_stale(freshness) {
        return Err("chain_stale_new_risk_stopped".to_string());
    }
    let max_order_size = parse_positive_decimal_string(&asset_cap.max_order_size).ok_or("bad_policy_cap")?;
    let max_daily_size = parse_positive_decimal_string(&asset_cap.max_daily_size).ok_or("bad_policy_cap")?;
    if new_risk.size > max_order_size {
        return Err("over_max_order_size".to_string());
    }
    if size_spent_today.checked_add(new_risk.size).ok_or("over_max_daily_size")? > max_daily_size {
        return Err("over_max_daily_size".to_string());
    }
    Ok(())
}

pub async fn check_base_rules_and_nonce(state: &SignerState, evaluated: &EvaluatedBaseIntent, chain_view: &ChainView, persist: bool) -> Result<(), String> {
    let _record_guard = base_record_lock().lock().await;
    let address = &evaluated.stored_share.venue_address;
    let record = load_base_record(&state.config.state_directory, address)?;
    let now_seconds = now_unix_seconds();
    let new_risk = evaluated.built.new_risk.as_ref();
    let spent = new_risk.map(|risk| base_size_spent_today(&record, &evaluated.intent.delegate_key, &risk.asset, now_seconds)).unwrap_or_default();
    evaluate_base_policy(&evaluated.intent, &evaluated.built, &chain_view.policy, &chain_view.freshness, spent)?;
    let reserved_record = reserve_base_nonce(&record, evaluated.intent.chain_nonce, &evaluated.intent.delegate_key, new_risk, now_seconds)?;
    if persist {
        persist_base_record(&state.config.state_directory, &reserved_record)?;
    }
    Ok(())
}

pub async fn evaluate_base_without_reserving(state: &SignerState, raw_intent: &Value) -> Result<EvaluatedBaseIntent, String> {
    let (intent, verified_intent) = parse_and_verify_base_intent(raw_intent)?;
    let limits = state.config.base_transaction_limits.as_ref().ok_or("base_not_configured")?;
    let stored_share = lookup_key_share(state, &intent.key_ref)?.ok_or("no_key_share_for_key_ref")?;
    let built = build_base_transaction(&intent, limits)?;
    let chain_view = read_chain_view(state, &intent.key_ref, &intent.delegate_key).await?;
    check_share_matches_chain(&stored_share, &chain_view.key_reference)?;
    let evaluated = EvaluatedBaseIntent { verified_intent, intent, built, stored_share, chain_view };
    check_base_rules_and_nonce(state, &evaluated, &evaluated.chain_view, false).await?;
    Ok(evaluated)
}

pub fn transaction_json(transaction: &Eip1559Transaction) -> Value {
    json!({
        "chain_id": transaction.chain_id,
        "nonce": transaction.nonce,
        "max_priority_fee_per_gas": transaction.max_priority_fee_per_gas,
        "max_fee_per_gas": transaction.max_fee_per_gas,
        "gas_limit": transaction.gas_limit,
        "to": address_text(&transaction.to),
        "value_wei": transaction.value_wei.to_string(),
        "data": format!("0x{}", hex::encode(&transaction.data)),
    })
}

fn refusal_response(request_id: &str, reason: &str) -> Value {
    json!({"request_id": request_id, "signed": false, "reason": reason})
}

async fn tell_peer(state: Arc<SignerState>, peer_signer_id: String, kind: &'static str, body: Value) {
    if let Err(detail) = call_peer(&state.directory, &peer_signer_id, kind, &body, Duration::from_millis(BEGIN_TIMEOUT_MILLISECONDS)).await {
        eprintln!("{kind} to {peer_signer_id} failed: {detail}");
    }
}

pub async fn handle_base_sign_request(state: &Arc<SignerState>, raw_intent: Value) -> Value {
    sweep_abandoned_sessions(state);
    let evaluated = match evaluate_base_without_reserving(state, &raw_intent).await {
        Ok(evaluated) => evaluated,
        Err(reason) => {
            record_refusal_of_raw(state, &raw_intent, &reason);
            return refusal_response(raw_intent["request_id"].as_str().unwrap_or(""), &reason);
        }
    };
    let request_id = evaluated.intent.request_id.clone();
    let digest_hex = hex::encode(evaluated.built.signing_hash);
    let mut random_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut random_bytes);
    let session_id = sha256_hex(&[random_bytes.as_slice(), evaluated.verified_intent.intent_hash.as_bytes()].concat());
    let (inbox_sender, inbox_receiver) = build_session_inbox();
    if let Err(reason) = register_round_inbox(state, &session_id, inbox_sender.clone()) {
        record_outcome(state, &evaluated.verified_intent, "refused", &reason);
        return refusal_response(&request_id, &reason);
    }
    let own_signer_id = state.config.signer_id.clone();
    let threshold = evaluated.stored_share.threshold as usize;
    let proposal_body = json!({"session_id": session_id, "intent": raw_intent, "height": state.highest_known_height.load(Ordering::SeqCst)});
    let mut pending_replies = FuturesUnordered::new();
    for peer_signer_id in evaluated.stored_share.committee.iter().filter(|signer| **signer != own_signer_id).cloned() {
        let reply_state = state.clone();
        let reply_body = proposal_body.clone();
        pending_replies.push(async move {
            let timeout = Duration::from_millis(PROPOSAL_TIMEOUT_MILLISECONDS);
            let outcome = call_peer(&reply_state.directory, &peer_signer_id, "sign_proposal", &reply_body, timeout).await;
            (peer_signer_id, outcome)
        });
    }
    let mut accepted_peers: Vec<String> = Vec::new();
    let mut refusal_notes: Vec<String> = Vec::new();
    while accepted_peers.len() < threshold - 1 {
        let Some((peer_signer_id, outcome)) = pending_replies.next().await else {
            break;
        };
        match outcome {
            Err(detail) => refusal_notes.push(detail),
            Ok(body) if body["accepted"].as_bool() != Some(true) => {
                refusal_notes.push(format!("{peer_signer_id} refused: {}", body["reason"].as_str().unwrap_or("no reason")))
            }
            Ok(body) if body["digest"].as_str() != Some(digest_hex.as_str()) => {
                refusal_notes.push(format!("action_mismatch peer={peer_signer_id} digest_mismatch_reported={}", body["digest"]))
            }
            Ok(_) => accepted_peers.push(peer_signer_id),
        }
    }
    let quorum_reason = if accepted_peers.len() < threshold - 1 {
        Some(format!("quorum_unavailable {}", refusal_notes.join("; ")))
    } else {
        check_base_rules_and_nonce(state, &evaluated, &evaluated.chain_view, true).await.err()
    };
    if let Some(reason) = quorum_reason {
        remove_round_inbox(state, &session_id);
        let abort_body = json!({"session_id": session_id, "reason": reason});
        for peer_signer_id in accepted_peers {
            tell_peer(state.clone(), peer_signer_id, "sign_abort", abort_body.clone()).await;
        }
        let late_state = state.clone();
        tokio::spawn(async move {
            while let Some((peer_signer_id, outcome)) = pending_replies.next().await {
                if matches!(outcome, Ok(ref body) if body["accepted"].as_bool() == Some(true)) {
                    tell_peer(late_state.clone(), peer_signer_id, "sign_abort", abort_body.clone()).await;
                }
            }
        });
        record_outcome(state, &evaluated.verified_intent, "refused", &reason);
        return refusal_response(&request_id, &reason);
    }
    let mut chosen = accepted_peers.clone();
    chosen.push(own_signer_id);
    let participants = participants_in_keygen_order(&evaluated.stored_share.committee, &chosen);
    let begin_body = json!({"session_id": session_id, "participants": participants});
    for peer_signer_id in &accepted_peers {
        let begin_reply = call_peer(&state.directory, peer_signer_id, "sign_begin", &begin_body, Duration::from_millis(BEGIN_TIMEOUT_MILLISECONDS)).await;
        if !matches!(&begin_reply, Ok(body) if body["started"].as_bool() == Some(true)) {
            let reason = format!("round_failed peer={peer_signer_id} detail=begin refused: {begin_reply:?}");
            remove_round_inbox(state, &session_id);
            record_outcome(state, &evaluated.verified_intent, "refused", &reason);
            return refusal_response(&request_id, &reason);
        }
    }
    let late_state = state.clone();
    let late_begin_body = begin_body.clone();
    tokio::spawn(async move {
        while let Some((peer_signer_id, outcome)) = pending_replies.next().await {
            if matches!(outcome, Ok(ref body) if body["accepted"].as_bool() == Some(true)) {
                tell_peer(late_state.clone(), peer_signer_id, "sign_begin", late_begin_body.clone()).await;
            }
        }
    });
    let round = SigningRound {
        session_id,
        participants,
        stored_share: evaluated.stored_share.clone(),
        digest: evaluated.built.signing_hash,
        inbox_sender,
        inbox_receiver,
    };
    let signed = run_signing_round(state.clone(), round).await.and_then(|signature| {
        let raw = raw_signed_transaction(&evaluated.built.transaction, &signature.r, &signature.s, signature.v)?;
        Ok((signature, raw))
    });
    match signed {
        Ok((signature, raw)) => {
            record_outcome(state, &evaluated.verified_intent, "signed", "");
            json!({
                "request_id": request_id,
                "signed": true,
                "transaction": transaction_json(&evaluated.built.transaction),
                "digest": digest_hex,
                "r": signature.r,
                "s": signature.s,
                "v": signature.v,
                "raw_transaction": format!("0x{}", hex::encode(&raw)),
                "transaction_hash": format!("0x{}", hex::encode(transaction_hash(&raw))),
            })
        }
        Err(reason) => {
            record_outcome(state, &evaluated.verified_intent, "refused", &reason);
            refusal_response(&request_id, &reason)
        }
    }
}

pub async fn handle_base_sign_proposal(state: &Arc<SignerState>, sender: &str, body: &Value) -> Value {
    sweep_abandoned_sessions(state);
    state.highest_known_height.fetch_max(body["height"].as_u64().unwrap_or(0), Ordering::SeqCst);
    let height = state.highest_known_height.load(Ordering::SeqCst);
    let session_id = body["session_id"].as_str().unwrap_or_default().to_string();
    let raw_intent = body["intent"].clone();
    let evaluated = match evaluate_base_without_reserving(state, &raw_intent).await {
        Ok(evaluated) => evaluated,
        Err(reason) => {
            record_refusal_of_raw(state, &raw_intent, &reason);
            return json!({"accepted": false, "reason": reason, "height": height});
        }
    };
    if let Err(reason) = check_base_rules_and_nonce(state, &evaluated, &evaluated.chain_view, true).await {
        record_outcome(state, &evaluated.verified_intent, "refused", &reason);
        return json!({"accepted": false, "reason": reason, "height": height});
    }
    let (inbox_sender, inbox_receiver) = build_session_inbox();
    if let Err(reason) = register_round_inbox(state, &session_id, inbox_sender) {
        record_outcome(state, &evaluated.verified_intent, "refused", &reason);
        return json!({"accepted": false, "reason": reason, "height": height});
    }
    let digest = evaluated.built.signing_hash;
    state.signing_sessions.lock().expect("session lock").insert(
        session_id,
        SigningSession {
            proposer: sender.to_string(),
            verified_intent: evaluated.verified_intent,
            digest,
            stored_share: evaluated.stored_share,
            inbox_receiver,
            created_at: Instant::now(),
        },
    );
    json!({"accepted": true, "digest": hex::encode(digest), "height": height})
}
