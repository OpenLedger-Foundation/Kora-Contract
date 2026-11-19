import http from "node:http";
import pino from "pino";
import { PersistentAlertQueue, type PendingAlert } from "./queue";

const logger = pino({ name: "kora-reconciler" });
const port = Number(process.env.RECONCILER_PORT ?? "4000");
const dbPath = process.env.RECONCILER_DB_PATH ?? "./reconciler.db";
const rpcHeadUrl = process.env.RECONCILER_RPC_HEAD_URL ?? "";
const checkpointUrl = process.env.RECONCILER_INDEXER_CHECKPOINT_URL ?? "";
const sinkUrl = process.env.RECONCILER_ALERT_SINK_URL ?? "";
const lagBudgetMs = Number(process.env.CHAOS_RECONCILER_ALERT_LAG_MS ?? "15000");
const scanIntervalMs = Number(process.env.RECONCILER_SCAN_INTERVAL_MS ?? Math.max(1_000, Math.floor(lagBudgetMs / 3)));
const queue = new PersistentAlertQueue(dbPath);
let lastCheckpoint = 0;
let lastRpcHead = 0;
let gapsDetected = 0;
let alertsDelivered = 0;
let lastSuccessfulAlert: number | null = null;
let scanRunning = false;

async function readLedger(url: string, key: "ledger" | "checkpoint"): Promise<number | null> {
  if (!url) return null;
  const response = await fetch(url);
  if (!response.ok) throw new Error(`ledger source returned ${response.status}`);
  const body = await response.json() as Record<string, unknown>;
  const value = Number(body[key] ?? body.sequence ?? body.latestLedger);
  return Number.isSafeInteger(value) && value >= 0 ? value : null;
}

async function scanGap(): Promise<void> {
  if (scanRunning) return;
  scanRunning = true;
  try {
    const [checkpoint, head] = await Promise.all([readLedger(checkpointUrl, "checkpoint"), readLedger(rpcHeadUrl, "ledger")]);
    if (checkpoint !== null) lastCheckpoint = checkpoint;
    if (head !== null) lastRpcHead = head;
    if (checkpoint !== null && head !== null && head > checkpoint) {
      const upper = Math.min(head, checkpoint + 1_000);
      gapsDetected += upper - checkpoint;
      for (let ledger = checkpoint + 1; ledger <= upper; ledger += 1) queue.enqueueGap(ledger, checkpoint + 1, head);
    }
    await drainQueue();
  } catch (error) {
    logger.warn({ error: String(error) }, "reconciler scan failed");
  } finally {
    scanRunning = false;
  }
}

async function deliver(alert: PendingAlert): Promise<void> {
  if (!sinkUrl) return;
  const response = await fetch(sinkUrl, { method: "POST", headers: { "content-type": "application/json", "idempotency-key": `reconciliation-alert:${alert.id}` }, body: JSON.stringify({ id: alert.id, type: alert.type, severity: alert.severity, ledger: alert.ledger, fromLedger: alert.fromLedger, toLedger: alert.toLedger, createdAt: alert.createdAt }) });
  if (!response.ok) throw new Error(`alert sink returned ${response.status}`);
  queue.acknowledge(alert.id);
  alertsDelivered += 1;
  lastSuccessfulAlert = Date.now();
}

async function drainQueue(): Promise<void> {
  for (const alert of queue.pending(50)) {
    try { await deliver(alert); } catch (error) { queue.markDeliveryFailure(alert.id, String(error)); }
  }
}

function json(res: http.ServerResponse, status: number, body: unknown): void { res.writeHead(status, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" }); res.end(JSON.stringify(body)); }
function statusBody() { return { lastScannedLedger: lastCheckpoint, rpcHeadLedger: lastRpcHead, gapsDetected, alertsFired: alertsDelivered, alertQueueDepth: queue.depth(), lastSuccessfulAlert, scanIntervalMs }; }

const server = http.createServer((req, res) => {
  const path = new URL(req.url ?? "/", `http://${req.headers.host ?? "localhost"}`).pathname;
  if (req.method === "GET" && path === "/health") return json(res, 200, { status: "ok", service: "reconciler" });
  if (req.method === "GET" && path === "/reconciler/status") return json(res, 200, statusBody());
  if (req.method === "GET" && (path === "/reconciler/pending-alerts" || path === "/reconciler/alerts/pending")) return json(res, 200, queue.allPending());
  if (req.method === "DELETE" && path === "/reconciler/alerts/pending") return json(res, 200, { drained: queue.clear() });
  return json(res, 404, { error: "not_found" });
});

const timer = setInterval(() => void scanGap(), scanIntervalMs);
server.listen(port, () => { logger.info({ port, scanIntervalMs }, "reconciler listening"); void scanGap(); });
function shutdown(): void { clearInterval(timer); server.close(() => { queue.close(); process.exit(0); }); }
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
