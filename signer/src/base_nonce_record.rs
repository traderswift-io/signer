use crate::base_policy_guards::check_chain_nonce;
use crate::base_venue_adapters::BaseNewRisk;
use crate::nonce_record::{write_file_durably, DailySize};
use crate::order_intent::sha256_hex;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::OnceLock;
use tokio::sync::Mutex as AsyncMutex;

const SECONDS_PER_DAY: u64 = 86_400;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BaseAddressRecord {
    pub address: String,
    pub highest_signed_chain_nonce: Option<u64>,
    pub daily_size_by_delegate_and_asset: BTreeMap<String, DailySize>,
}

static BASE_RECORD_LOCK: OnceLock<AsyncMutex<()>> = OnceLock::new();

// One lock for every Base record: the check, the reservation and the write happen under it, so two requests cannot
// both take one chain nonce.
pub fn base_record_lock() -> &'static AsyncMutex<()> {
    BASE_RECORD_LOCK.get_or_init(|| AsyncMutex::new(()))
}

fn record_file_path(state_directory: &Path, address: &str) -> PathBuf {
    state_directory.join("base_nonce_records").join(format!("{}.json", sha256_hex(address.as_bytes())))
}

pub fn load_base_record(state_directory: &Path, address: &str) -> Result<BaseAddressRecord, String> {
    match fs::read(record_file_path(state_directory, address)) {
        Ok(raw_record) => serde_json::from_slice(&raw_record).map_err(|error| format!("base nonce record corrupt: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(BaseAddressRecord { address: address.to_string(), ..BaseAddressRecord::default() })
        }
        Err(error) => Err(format!("base nonce record unreadable: {error}")),
    }
}

pub fn persist_base_record(state_directory: &Path, record: &BaseAddressRecord) -> Result<(), String> {
    let serialized_record = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    write_file_durably(&record_file_path(state_directory, &record.address), &serialized_record)
}

fn daily_size_key(delegate_key: &str, asset: &str) -> String {
    format!("{delegate_key}/{asset}")
}

pub fn base_size_spent_today(record: &BaseAddressRecord, delegate_key: &str, asset: &str, now_seconds: u64) -> Decimal {
    record
        .daily_size_by_delegate_and_asset
        .get(&daily_size_key(delegate_key, asset))
        .filter(|daily| daily.utc_day == now_seconds / SECONDS_PER_DAY)
        .and_then(|daily| Decimal::from_str(&daily.size).ok())
        .unwrap_or(Decimal::ZERO)
}

pub fn reserve_base_nonce(
    record: &BaseAddressRecord,
    chain_nonce: u64,
    delegate_key: &str,
    new_risk: Option<&BaseNewRisk>,
    now_seconds: u64,
) -> Result<BaseAddressRecord, String> {
    check_chain_nonce(record.highest_signed_chain_nonce, chain_nonce)?;
    let mut reserved_record = record.clone();
    reserved_record.highest_signed_chain_nonce = Some(chain_nonce);
    if let Some(new_risk) = new_risk {
        let spent = base_size_spent_today(record, delegate_key, &new_risk.asset, now_seconds);
        let total = spent.checked_add(new_risk.size).ok_or("over_max_daily_size")?;
        reserved_record.daily_size_by_delegate_and_asset.insert(
            daily_size_key(delegate_key, &new_risk.asset),
            DailySize { utc_day: now_seconds / SECONDS_PER_DAY, size: total.normalize().to_string() },
        );
    }
    Ok(reserved_record)
}
