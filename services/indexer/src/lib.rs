use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NormalizedEvent {
    pub contract: String,
    pub event_type: String,
    pub ledger_sequence: u64,
    pub event_index: u32,
    pub decoded_payload: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct RpcEventsResponse {
    events: Vec<RpcEvent>,
}

#[derive(Debug, Deserialize, Clone)]
struct RpcEvent {
    ledger: u64,
    contract: String,
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default, rename = "event_index")]
    event_index: Option<u32>,
    #[serde(default)]
    value: serde_json::Value,
}

#[derive(Default)]
pub struct IndexerStore {
    events: Mutex<HashMap<(u64, u32), NormalizedEvent>>,
    last_checkpoint: Mutex<u64>,
}

impl IndexerStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, event: NormalizedEvent) -> bool {
        let mut events = self.events.lock().unwrap();
        let key = (event.ledger_sequence, event.event_index);
        if events.contains_key(&key) {
            return false;
        }
        events.insert(key, event);
        true
    }

    pub fn set_checkpoint(&self, seq: u64) {
        let mut cp = self.last_checkpoint.lock().unwrap();
        *cp = seq;
    }

    pub fn get_checkpoint(&self) -> u64 {
        *self.last_checkpoint.lock().unwrap()
    }

    pub fn query_by_contract(&self, contract: &str) -> Vec<NormalizedEvent> {
        let events = self.events.lock().unwrap();
        events
            .values()
            .filter(|e| e.contract == contract)
            .cloned()
            .collect()
    }
}

pub struct FileCheckpoint {
    path: PathBuf,
}

impl FileCheckpoint {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn read(&self) -> u64 {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0)
    }

    pub fn write(&self, seq: u64) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, seq.to_string())
    }
}

pub struct IndexerService {
    store: Arc<IndexerStore>,
    rpc_url: String,
    checkpoint: Arc<FileCheckpoint>,
}

impl IndexerService {
    pub fn new(store: Arc<IndexerStore>) -> Self {
        Self {
            store,
            rpc_url: String::new(),
            checkpoint: Arc::new(FileCheckpoint::new(PathBuf::from(".indexer-checkpoint"))),
        }
    }

    pub fn with_rpc(store: Arc<IndexerStore>, rpc_url: String, checkpoint_path: PathBuf) -> Self {
        Self {
            store,
            rpc_url,
            checkpoint: Arc::new(FileCheckpoint::new(checkpoint_path)),
        }
    }

    pub fn ingest_raw_event(
        &self,
        contract: &str,
        event_type: &str,
        ledger_seq: u64,
        event_idx: u32,
        raw_payload: &str,
    ) -> Result<bool, String> {
        let decoded: serde_json::Value = serde_json::from_str(raw_payload)
            .map_err(|e| format!("Malformed event payload: {}", e))?;

        let event = NormalizedEvent {
            contract: contract.to_string(),
            event_type: event_type.to_string(),
            ledger_sequence: ledger_seq,
            event_index: event_idx,
            decoded_payload: decoded,
        };

        let inserted = self.store.insert(event);
        if inserted {
            self.store.set_checkpoint(ledger_seq);
            let _ = self.checkpoint.write(ledger_seq);
        }
        Ok(inserted)
    }

    pub async fn fetch_ledgers(&self, start_ledger: u64, limit: u64) -> Result<(u64, u64), String> {
        let params = serde_json::json!({
            "startLedger": start_ledger,
            "limit": limit,
        });
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getEvents",
            "params": params,
        });
        let resp: RpcEventsResponse = reqwest::Client::new()
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("RPC request failed: {}", e))?
            .json()
            .await
            .map_err(|e| format!("RPC response parse failed: {}", e))?;

        let mut inserted = 0u64;
        let mut next_cursor = start_ledger;
        for ev in resp.events {
            let payload = serde_json::to_string(&ev.value).unwrap_or_else(|_| "{}".to_string());
            let idx = ev.event_index.unwrap_or(0);
            if self
                .store
                .ingest_raw_event(&ev.contract, &ev.event_type, ev.ledger, idx, &payload)
                .unwrap_or(false)
            {
                inserted += 1;
            }
            next_cursor = ev.ledger + 1;
        }
        Ok((inserted, next_cursor))
    }

    pub async fn run_ingestion_loop(&self, start_ledger: Option<u64>) {
        let mut backoff_ms = 1_000u64;
        let mut cursor = start_ledger.unwrap_or_else(|| self.checkpoint.read() + 1);
        loop {
            match self.fetch_ledgers(cursor, 100).await {
                Ok((_inserted, next_cursor)) => {
                    cursor = next_cursor;
                    backoff_ms = 1_000;
                    sleep(Duration::from_millis(500)).await;
                }
                Err(_err) => {
                    backoff_ms = (backoff_ms * 2).min(30_000);
                    sleep(Duration::from_millis(backoff_ms)).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_indexer_idempotency_and_checkpoint_resumption() {
        let store = Arc::new(IndexerStore::new());
        let indexer = IndexerService::new(store.clone());

        let ok1 = indexer
            .ingest_raw_event("invoice_nft", "INV_CRT", 100, 1, r#"{"id": 1, "amount": 1000}"#)
            .unwrap();
        assert!(ok1);
        assert_eq!(store.get_checkpoint(), 100);

        let ok_dup = indexer
            .ingest_raw_event("invoice_nft", "INV_CRT", 100, 1, r#"{"id": 1, "amount": 1000}"#)
            .unwrap();
        assert!(!ok_dup);

        let err = indexer
            .ingest_raw_event("marketplace", "MKT_LST", 101, 1, "invalid json");
        assert!(err.is_err());

        assert_eq!(store.get_checkpoint(), 100);

        let queried = store.query_by_contract("invoice_nft");
        assert_eq!(queried.len(), 1);
    }

    #[test]
    fn test_file_checkpoint_read_write_persists_across_restart() {
        let dir = std::env::temp_dir().join("kora_indexer_test");
        let path = dir.join("checkpoint");
        let _ = fs::remove_file(&path);
        let cp = FileCheckpoint::new(path.clone());
        assert_eq!(cp.read(), 0);
        cp.write(42).unwrap();
        assert_eq!(cp.read(), 42);
        let cp2 = FileCheckpoint::new(path);
        assert_eq!(cp2.read(), 42);
        let _ = fs::remove_dir_all(dir);
    }
}
