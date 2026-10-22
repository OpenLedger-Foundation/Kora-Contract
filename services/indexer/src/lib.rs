use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NormalizedEvent {
    pub contract: String,
    pub event_type: String,
    pub ledger_sequence: u64,
    pub event_index: u32,
    pub decoded_payload: serde_json::Value,
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
            return false; // Idempotent skip
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

pub struct IndexerService {
    store: Arc<IndexerStore>,
}

impl IndexerService {
    pub fn new(store: Arc<IndexerStore>) -> Self {
        Self { store }
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
        }
        Ok(inserted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_indexer_idempotency_and_checkpoint_resumption() {
        let store = Arc::new(IndexerStore::new());
        let indexer = IndexerService::new(store.clone());

        // Initial event
        let ok1 = indexer.ingest_raw_event("invoice_nft", "INV_CRT", 100, 1, r#"{"id": 1, "amount": 1000}"#).unwrap();
        assert!(ok1);
        assert_eq!(store.get_checkpoint(), 100);

        // Duplicate event (reorg / restart simulate)
        let ok_dup = indexer.ingest_raw_event("invoice_nft", "INV_CRT", 100, 1, r#"{"id": 1, "amount": 1000}"#).unwrap();
        assert!(!ok_dup); // Not re-inserted

        // Malformed event skip-and-log test
        let err = indexer.ingest_raw_event("marketplace", "MKT_LST", 101, 1, "invalid json");
        assert!(err.is_err());

        // Checkpoint unchanged on error
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
}
