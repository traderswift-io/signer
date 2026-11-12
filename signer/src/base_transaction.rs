use sha3::{Digest, Keccak256};

pub const BASE_CHAIN_ID: u64 = 8453;
const EIP1559_TRANSACTION_TYPE: u8 = 0x02;
const RLP_SHORT_STRING_OFFSET: u8 = 0x80;
const RLP_LONG_STRING_OFFSET: u8 = 0xb7;
const RLP_SHORT_LIST_OFFSET: u8 = 0xc0;
const RLP_LONG_LIST_OFFSET: u8 = 0xf7;
const RLP_SHORT_PAYLOAD_LIMIT: usize = 55;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eip1559Transaction {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee_per_gas: u64,
    pub max_fee_per_gas: u64,
    pub gas_limit: u64,
    pub to: [u8; 20],
    pub value_wei: u128,
    pub data: Vec<u8>,
}

pub fn keccak256_of(data: &[u8]) -> [u8; 32] {
    Keccak256::digest(data).into()
}

fn minimal_big_endian(bytes: &[u8]) -> &[u8] {
    let first_nonzero = bytes.iter().position(|byte| *byte != 0).unwrap_or(bytes.len());
    &bytes[first_nonzero..]
}

fn rlp_length_prefix(payload_length: usize, short_offset: u8, long_offset: u8, output: &mut Vec<u8>) {
    if payload_length <= RLP_SHORT_PAYLOAD_LIMIT {
        output.push(short_offset + payload_length as u8);
        return;
    }
    let length_bytes = (payload_length as u64).to_be_bytes();
    let minimal_length = minimal_big_endian(&length_bytes);
    output.push(long_offset + minimal_length.len() as u8);
    output.extend_from_slice(minimal_length);
}

pub fn rlp_encode_bytes(bytes: &[u8], output: &mut Vec<u8>) {
    if bytes.len() == 1 && bytes[0] < RLP_SHORT_STRING_OFFSET {
        output.push(bytes[0]);
        return;
    }
    rlp_length_prefix(bytes.len(), RLP_SHORT_STRING_OFFSET, RLP_LONG_STRING_OFFSET, output);
    output.extend_from_slice(bytes);
}

pub fn rlp_encode_unsigned(big_endian_bytes: &[u8], output: &mut Vec<u8>) {
    rlp_encode_bytes(minimal_big_endian(big_endian_bytes), output);
}

pub fn rlp_wrap_list(payload: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(payload.len() + 9);
    rlp_length_prefix(payload.len(), RLP_SHORT_LIST_OFFSET, RLP_LONG_LIST_OFFSET, &mut output);
    output.extend_from_slice(payload);
    output
}

fn unsigned_fields_payload(transaction: &Eip1559Transaction) -> Vec<u8> {
    let mut payload = Vec::with_capacity(transaction.data.len() + 96);
    rlp_encode_unsigned(&transaction.chain_id.to_be_bytes(), &mut payload);
    rlp_encode_unsigned(&transaction.nonce.to_be_bytes(), &mut payload);
    rlp_encode_unsigned(&transaction.max_priority_fee_per_gas.to_be_bytes(), &mut payload);
    rlp_encode_unsigned(&transaction.max_fee_per_gas.to_be_bytes(), &mut payload);
    rlp_encode_unsigned(&transaction.gas_limit.to_be_bytes(), &mut payload);
    rlp_encode_bytes(&transaction.to, &mut payload);
    rlp_encode_unsigned(&transaction.value_wei.to_be_bytes(), &mut payload);
    rlp_encode_bytes(&transaction.data, &mut payload);
    payload.extend_from_slice(&rlp_wrap_list(&[]));
    payload
}

pub fn typed_transaction_bytes(rlp_list: &[u8]) -> Vec<u8> {
    let mut typed = Vec::with_capacity(rlp_list.len() + 1);
    typed.push(EIP1559_TRANSACTION_TYPE);
    typed.extend_from_slice(rlp_list);
    typed
}

pub fn unsigned_transaction_bytes(transaction: &Eip1559Transaction) -> Vec<u8> {
    typed_transaction_bytes(&rlp_wrap_list(&unsigned_fields_payload(transaction)))
}

pub fn signing_hash(transaction: &Eip1559Transaction) -> [u8; 32] {
    keccak256_of(&unsigned_transaction_bytes(transaction))
}

pub fn y_parity_from_v(v: u8) -> Result<u8, String> {
    match v {
        0 | 1 => Ok(v),
        27 | 28 => Ok(v - 27),
        _ => Err(format!("bad_recovery_value_{v}")),
    }
}

pub fn raw_signed_transaction(transaction: &Eip1559Transaction, r_hex: &str, s_hex: &str, v: u8) -> Result<Vec<u8>, String> {
    let r_bytes: [u8; 32] = hex::decode(r_hex).map_err(|_| "bad_r")?.try_into().map_err(|_| "bad_r")?;
    let s_bytes: [u8; 32] = hex::decode(s_hex).map_err(|_| "bad_s")?.try_into().map_err(|_| "bad_s")?;
    let mut payload = unsigned_fields_payload(transaction);
    rlp_encode_unsigned(&[y_parity_from_v(v)?], &mut payload);
    rlp_encode_unsigned(&r_bytes, &mut payload);
    rlp_encode_unsigned(&s_bytes, &mut payload);
    Ok(typed_transaction_bytes(&rlp_wrap_list(&payload)))
}

pub fn transaction_hash(raw_signed: &[u8]) -> [u8; 32] {
    keccak256_of(raw_signed)
}
