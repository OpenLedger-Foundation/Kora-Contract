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
    metrics: Mutex<MetricsRegistry>,
}

/// Low-cardinality event types only — never use invoice_id or similar.
const ALLOWED_EVENT_TYPES: &[&str] = &["default", "circuit_breaker_trip", "treasury_sweep"];

fn normalize_event_type(t: &str) -> &'static str {
    for a in ALLOWED_EVENT_TYPES {
        if *a == t {
            return match *a {
                "default" => "default",
                "circuit_breaker_trip" => "circuit_breaker_trip",
                "treasury_sweep" => "treasury_sweep",
                _ => "default",
            };
        }
    }
    "default"
}

#[derive(Default)]
pub struct MetricsRegistry {
    /// Low-cardinality counters keyed by normalized event_type.
    event_counts: HashMap<String, u64>,
    /// Last network head ledger seen (for lag gauge).
    head_ledger: Option<u64>,
}

impl MetricsRegistry {
    pub fn inc_event(&mut self, event_type: &str) {
        let key = normalize_event_type(event_type).to_string();
        *self.event_counts.entry(key).or_insert(0) += 1;
    }

    pub fn set_head(&mut self, seq: u64) {
        self.head_ledger = Some(seq);
    }

    /// Lag in seconds ≈ (head - checkpoint) * 5s/ledger; None when head unknown.
    pub fn lag_seconds(&self, checkpoint: u64) -> Option<u64> {
        self.head_ledger.map(|h| h.saturating_sub(checkpoint).saturating_mul(5))
    }

    /// Prometheus 0.0.4 text exposition. Always emits TYPE/HELP lines even with zero events.
    pub fn render(&self, checkpoint: u64) -> String {
        let mut out = String::new();
        out.push_str("# HELP kora_indexer_lag_seconds Current processed-ledger lag.\n");
        out.push_str("# TYPE kora_indexer_lag_seconds gauge\n");
        match self.lag_seconds(checkpoint) {
            Some(lag) => out.push_str(&format!("kora_indexer_lag_seconds {}\n", lag)),
            None => out.push_str("# kora_indexer_lag_seconds: head unavailable\n"),
        }
        out.push_str("# HELP kora_onchain_events_total Monotonically increasing counters from finalized indexed events.\n");
        out.push_str("# TYPE kora_onchain_events_total counter\n");
        for t in ALLOWED_EVENT_TYPES {
            let v = self.event_counts.get(*t).copied().unwrap_or(0);
            out.push_str(&format!("kora_onchain_events_total{{event_type=\"{}\"}} {}\n", t, v));
        }
        out
    }
}

impl IndexerStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn metrics_text(&self) -> String {
        let m = self.metrics.lock().unwrap();
        m.render(*self.last_checkpoint.lock().unwrap())
    }

    pub fn set_head_ledger(&self, seq: u64) {
        self.metrics.lock().unwrap().set_head(seq);
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
            // Count only on actual insert: duplicate/reorg replays do not double-increment.
            self.store.metrics.lock().unwrap().inc_event(event_type);
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
    fn test_metrics_line_format() {
        let store = Arc::new(IndexerStore::new());
        let indexer = IndexerService::new(store.clone());
        indexer.ingest_raw_event("invoice_nft", "default", 100, 1, r#"{"id": 1}"#).unwrap();
        indexer.ingest_raw_event("invoice_nft", "default", 101, 1, r#"{"id": 2}"#).unwrap();
        // Duplicate must not double-increment.
        indexer.ingest_raw_event("invoice_nft", "default", 101, 1, r#"{"id": 2}"#).unwrap();
        store.set_head_ledger(110);
        let text = store.metrics_text();
        assert!(text.contains("# TYPE kora_indexer_lag_seconds gauge"));
        assert!(text.contains("kora_indexer_lag_seconds 45"));
        assert!(text.contains("kora_onchain_events_total{event_type=\"default\"} 2"));
        assert!(text.contains("# TYPE kora_onchain_events_total counter"));
        assert!(!text.contains("invoice"));
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
