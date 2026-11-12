use std::collections::BTreeMap;

pub const WORD_BYTES: usize = 32;
pub const SELECTOR_BYTES: usize = 4;

pub const fn address_from_hex(text: &str) -> [u8; 20] {
    let bytes = text.as_bytes();
    assert!(bytes.len() == 42 && bytes[0] == b'0' && bytes[1] == b'x');
    let mut address = [0u8; 20];
    let mut position = 0;
    while position < 20 {
        address[position] = hex_digit_value(bytes[2 + 2 * position]) * 16 + hex_digit_value(bytes[3 + 2 * position]);
        position += 1;
    }
    address
}

const fn hex_digit_value(digit: u8) -> u8 {
    match digit {
        b'0'..=b'9' => digit - b'0',
        b'a'..=b'f' => digit - b'a' + 10,
        b'A'..=b'F' => digit - b'A' + 10,
        _ => panic!("not a hex digit"),
    }
}

pub fn parse_address_text(text: &str) -> Result<[u8; 20], String> {
    let Some(hex_part) = text.strip_prefix("0x") else {
        return Err("bad_address".to_string());
    };
    if hex_part.len() != 40 || !hex_part.chars().all(|character| matches!(character, '0'..='9' | 'a'..='f')) {
        return Err("bad_address".to_string());
    }
    let decoded = hex::decode(hex_part).map_err(|_| "bad_address")?;
    decoded.try_into().map_err(|_| "bad_address".to_string())
}

pub fn address_text(address: &[u8; 20]) -> String {
    format!("0x{}", hex::encode(address))
}

pub fn parse_unsigned_text(field_name: &str, text: &str) -> Result<u128, String> {
    let is_plain = !text.is_empty()
        && text.len() <= 39
        && text.chars().all(|character| character.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    if !is_plain {
        return Err(format!("bad_{field_name}"));
    }
    text.parse::<u128>().map_err(|_| format!("bad_{field_name}"))
}

pub fn parse_bool_text(field_name: &str, text: &str) -> Result<bool, String> {
    match text {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("bad_{field_name}")),
    }
}

pub fn argument<'a>(arguments: &'a BTreeMap<String, String>, field_name: &str) -> Result<&'a str, String> {
    arguments.get(field_name).map(String::as_str).ok_or(format!("missing_{field_name}"))
}

pub fn refuse_unexpected_arguments(arguments: &BTreeMap<String, String>, expected_names: &[&str]) -> Result<(), String> {
    match arguments.keys().find(|name| !expected_names.contains(&name.as_str())) {
        Some(unexpected_name) => Err(format!("unexpected_argument_{unexpected_name}")),
        None => Ok(()),
    }
}

pub fn unsigned_argument(arguments: &BTreeMap<String, String>, field_name: &str) -> Result<u128, String> {
    parse_unsigned_text(field_name, argument(arguments, field_name)?)
}

pub fn bool_argument(arguments: &BTreeMap<String, String>, field_name: &str) -> Result<bool, String> {
    parse_bool_text(field_name, argument(arguments, field_name)?)
}

pub fn word_from_unsigned(value: u128) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[16..].copy_from_slice(&value.to_be_bytes());
    word
}

pub fn word_from_address(address: &[u8; 20]) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(address);
    word
}

pub fn word_from_bool(flag: bool) -> [u8; 32] {
    word_from_unsigned(flag as u128)
}

pub fn calldata_from_words(selector: [u8; 4], words: &[[u8; 32]]) -> Vec<u8> {
    let mut calldata = Vec::with_capacity(SELECTOR_BYTES + words.len() * WORD_BYTES);
    calldata.extend_from_slice(&selector);
    for word in words {
        calldata.extend_from_slice(word);
    }
    calldata
}

pub fn selector_of_calldata(data: &[u8]) -> Option<[u8; 4]> {
    data.get(..SELECTOR_BYTES).map(|selector| selector.try_into().expect("four bytes"))
}

pub fn argument_word(data: &[u8], index: usize) -> Result<[u8; 32], String> {
    let start = SELECTOR_BYTES + index * WORD_BYTES;
    data.get(start..start + WORD_BYTES)
        .map(|word| word.try_into().expect("thirty two bytes"))
        .ok_or("calldata_too_short".to_string())
}

pub fn word_as_address(word: &[u8; 32]) -> Result<[u8; 20], String> {
    if word[..12].iter().any(|byte| *byte != 0) {
        return Err("address_word_has_high_bytes".to_string());
    }
    Ok(word[12..].try_into().expect("twenty bytes"))
}

pub fn word_as_unsigned(word: &[u8; 32]) -> Result<u128, String> {
    if word[..16].iter().any(|byte| *byte != 0) {
        return Err("unsigned_word_above_u128".to_string());
    }
    Ok(u128::from_be_bytes(word[16..].try_into().expect("sixteen bytes")))
}
