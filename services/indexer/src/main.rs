use kora_indexer::{IndexerService, IndexerStore};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serve the health and metrics endpoints over a minimal hand-rolled HTTP/1.1
/// server so the binary stays dependency-free beyond `tokio`.
///
/// Routes:
///   GET /api/v1/health  — 200 JSON `{"status":"ok"}`
///   GET /metrics        — 200 Prometheus 0.0.4 text
///   anything else       — 404
async fn serve(store: Arc<IndexerStore>, bind: String) {
    let listener = TcpListener::bind(&bind)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {bind}: {e}"));

    eprintln!("kora-indexer HTTP listening on {bind}");

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };
        let store = store.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            let n = match stream.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => return,
            };
            let req = String::from_utf8_lossy(&buf[..n]);
            let first_line = req.lines().next().unwrap_or("");

            let (status, content_type, body) =
                if first_line.contains("GET /api/v1/health") {
                    (
                        "200 OK",
                        "application/json",
                        r#"{"status":"ok","service":"kora-indexer"}"#.to_string(),
                    )
                } else if first_line.contains("GET /metrics") {
                    ("200 OK", "text/plain; version=0.0.4", store.metrics_text())
                } else {
                    ("404 Not Found", "text/plain", "not found".to_string())
                };

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
    }
}

#[tokio::main]
async fn main() {
    // ── Configuration from environment ────────────────────────────────────────
    let rpc_url = std::env::var("SOROBAN_RPC_URL")
        .unwrap_or_else(|_| String::new());

    let start_ledger: Option<u64> = std::env::var("START_LEDGER")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| v > 0);

    let checkpoint_path = std::env::var("CHECKPOINT_PATH")
        .unwrap_or_else(|_| "/opt/kora/indexer/data/checkpoint".to_string());

    let bind = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:3000".to_string());

    // Fail fast with a clear message when the RPC URL is not configured.
    if rpc_url.is_empty() {
        eprintln!(
            "FATAL: SOROBAN_RPC_URL is not set.\n\
             Set it to a valid Soroban RPC endpoint and restart the indexer.\n\
             Example: export SOROBAN_RPC_URL=https://soroban-testnet.stellar.org"
        );
        std::process::exit(1);
    }

    eprintln!(
        "kora-indexer starting | rpc={rpc_url} | checkpoint={checkpoint_path} | bind={bind}"
    );

    // ── Shared store ──────────────────────────────────────────────────────────
    let store = Arc::new(IndexerStore::new());

    // ── Ingestion loop (background) ───────────────────────────────────────────
    let indexer = Arc::new(IndexerService::with_rpc(
        store.clone(),
        rpc_url,
        PathBuf::from(checkpoint_path),
    ));

    tokio::spawn(async move {
        indexer.run_ingestion_loop(start_ledger).await;
    });

    // ── HTTP server (foreground) ──────────────────────────────────────────────
    serve(store, bind).await;
}
