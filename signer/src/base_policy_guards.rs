use crate::base_abi_words::{argument_word, selector_of_calldata, word_as_address, word_as_unsigned, SELECTOR_BYTES, WORD_BYTES};
use crate::base_transaction::{Eip1559Transaction, BASE_CHAIN_ID};
use crate::base_venue_adapters::{BaseVenueAdapter, ERC20_APPROVE_SELECTOR};
use serde::Deserialize;

const ERC20_TRANSFER_SELECTOR: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb];
const ERC20_TRANSFER_FROM_SELECTOR: [u8; 4] = [0x23, 0xb8, 0x72, 0xdd];
const ERC20_INCREASE_ALLOWANCE_SELECTOR: [u8; 4] = [0x39, 0x50, 0x93, 0x51];
const ERC20_DECREASE_ALLOWANCE_SELECTOR: [u8; 4] = [0xa4, 0x57, 0xc2, 0xd7];
const ERC20_PERMIT_SELECTOR: [u8; 4] = [0xd5, 0x05, 0xac, 0xcf];
const ERC20_TWO_WORD_CALL_BYTES: usize = SELECTOR_BYTES + 2 * WORD_BYTES;
const INTRINSIC_TRANSACTION_GAS: u64 = 21_000;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseTransactionLimits {
    pub max_fee_per_gas_ceiling: u64,
    pub gas_limit_ceiling: u64,
}

fn check_erc20_rules(adapter: &BaseVenueAdapter, to: &[u8; 20], data: &[u8], selector: [u8; 4]) -> Result<(), String> {
    match selector {
        ERC20_TRANSFER_SELECTOR => {
            if data.len() != ERC20_TWO_WORD_CALL_BYTES {
                return Err("erc20_transfer_refused_malformed".to_string());
            }
            let recipient = word_as_address(&argument_word(data, 0)?)?;
            if !adapter.venue_contracts.contains(&recipient) {
                return Err("erc20_transfer_refused".to_string());
            }
            Ok(())
        }
        ERC20_APPROVE_SELECTOR => {
            if data.len() != ERC20_TWO_WORD_CALL_BYTES {
                return Err("erc20_approve_refused_malformed".to_string());
            }
            let spender = word_as_address(&argument_word(data, 0)?)?;
            let amount = word_as_unsigned(&argument_word(data, 1)?).map_err(|_| "erc20_approve_over_cap".to_string())?;
            if !adapter.venue_contracts.contains(&spender) {
                return Err("erc20_approve_refused_spender".to_string());
            }
            let cap = adapter
                .approve_caps
                .iter()
                .find(|cap| cap.token == *to && cap.spender == spender)
                .ok_or("erc20_approve_refused_no_cap")?;
            if amount > cap.maximum_amount {
                return Err("erc20_approve_over_cap".to_string());
            }
            Ok(())
        }
        ERC20_TRANSFER_FROM_SELECTOR | ERC20_INCREASE_ALLOWANCE_SELECTOR | ERC20_DECREASE_ALLOWANCE_SELECTOR | ERC20_PERMIT_SELECTOR => {
            Err("erc20_call_refused".to_string())
        }
        _ => Ok(()),
    }
}

pub fn check_contract_call(adapter: &BaseVenueAdapter, to: &[u8; 20], data: &[u8], value_wei: u128) -> Result<(), String> {
    if *to == [0u8; 20] {
        return Err("contract_creation_refused".to_string());
    }
    let Some(selector) = selector_of_calldata(data) else {
        return Err("native_send_refused".to_string());
    };
    check_erc20_rules(adapter, to, data, selector)?;
    let allowed_call = adapter
        .allowed_calls
        .iter()
        .find(|allowed| allowed.contract == *to && allowed.selector == selector)
        .ok_or("call_not_allowlisted")?;
    if value_wei != allowed_call.required_value_wei {
        return Err("value_not_allowed".to_string());
    }
    Ok(())
}

pub fn check_gas(transaction: &Eip1559Transaction, limits: &BaseTransactionLimits) -> Result<(), String> {
    if transaction.max_fee_per_gas > limits.max_fee_per_gas_ceiling {
        return Err("max_fee_per_gas_over_cap".to_string());
    }
    if transaction.max_priority_fee_per_gas > transaction.max_fee_per_gas {
        return Err("priority_fee_above_max_fee".to_string());
    }
    if transaction.gas_limit > limits.gas_limit_ceiling {
        return Err("gas_limit_over_cap".to_string());
    }
    if transaction.gas_limit < INTRINSIC_TRANSACTION_GAS {
        return Err("gas_limit_below_intrinsic".to_string());
    }
    Ok(())
}

pub fn check_transaction(adapter: &BaseVenueAdapter, transaction: &Eip1559Transaction, limits: &BaseTransactionLimits) -> Result<(), String> {
    if transaction.chain_id != BASE_CHAIN_ID {
        return Err("chain_id_not_base".to_string());
    }
    check_contract_call(adapter, &transaction.to, &transaction.data, transaction.value_wei)?;
    check_gas(transaction, limits)
}

pub fn check_chain_nonce(highest_signed_chain_nonce: Option<u64>, chain_nonce: u64) -> Result<(), String> {
    match highest_signed_chain_nonce {
        Some(highest) if chain_nonce <= highest => Err(format!("chain_nonce_not_above_highest_signed_{highest}")),
        _ => Ok(()),
    }
}
