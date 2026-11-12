use crate::base_abi_words::parse_address_text;
use crate::base_policy_guards::{check_transaction, BaseTransactionLimits};
use crate::base_transaction::{signing_hash, Eip1559Transaction, BASE_CHAIN_ID};
use crate::base_venue_adapters::{lookup_base_venue_adapter, BaseCallRequest, BaseNewRisk};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const BASE_VENUE_PREFIX: &str = "base_";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseIntent {
    pub request_id: String,
    pub key_ref: String,
    pub delegate_key: String,
    pub policy_epoch: u64,
    pub venue: String,
    pub vault_address: String,
    pub kind: String,
    pub chain_nonce: u64,
    pub max_priority_fee_per_gas: u64,
    pub max_fee_per_gas: u64,
    pub gas_limit: u64,
    pub expires_at: u64,
    pub delegate_signature: String,
    pub arguments: BTreeMap<String, String>,
}

pub struct BuiltBaseTransaction {
    pub transaction: Eip1559Transaction,
    pub signing_hash: [u8; 32],
    pub new_risk: Option<BaseNewRisk>,
}

pub fn is_base_venue(venue: &str) -> bool {
    venue.starts_with(BASE_VENUE_PREFIX)
}

pub fn build_base_transaction(intent: &BaseIntent, limits: &BaseTransactionLimits) -> Result<BuiltBaseTransaction, String> {
    let adapter = lookup_base_venue_adapter(&intent.venue).ok_or("venue_not_allowed")?;
    let trader = parse_address_text(&intent.vault_address).map_err(|_| "bad_vault_address")?;
    let request = BaseCallRequest { kind: &intent.kind, trader, arguments: &intent.arguments };
    let call = (adapter.build_call)(&request)?;
    (adapter.check_call)(&request, &call)?;
    let transaction = Eip1559Transaction {
        chain_id: BASE_CHAIN_ID,
        nonce: intent.chain_nonce,
        max_priority_fee_per_gas: intent.max_priority_fee_per_gas,
        max_fee_per_gas: intent.max_fee_per_gas,
        gas_limit: intent.gas_limit,
        to: call.to,
        value_wei: call.value_wei,
        data: call.data,
    };
    check_transaction(adapter, &transaction, limits)?;
    Ok(BuiltBaseTransaction { signing_hash: signing_hash(&transaction), transaction, new_risk: call.new_risk })
}
