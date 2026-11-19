import * as http from "node:http";
import { TreasuryDataSource } from "./data";

const source = new TreasuryDataSource();
const port = Number(process.env.PORT || 3000);
const maxIndexerLag = Number(process.env.MAX_INDEXER_LAG_LEDGERS || 100);

function json(res: http.ServerResponse, status: number, body: unknown, lastIndexedLedger?: number) {
  const payload = JSON.stringify(body);
  const headers: Record<string, string> = { "content-type": "application/json", "content-length": String(Buffer.byteLength(payload)), "cache-control": "public, max-age=15" };
  if (lastIndexedLedger != null) headers["x-last-indexed-ledger"] = String(lastIndexedLedger);
  res.writeHead(status, headers);
  res.end(payload);
}

function scrub(row: Record<string, unknown>) {
  const forbidden = new Set(["debtor", "debtorHash", "rawDebtor", "adminSecret", "privateKey", "apiKey"]);
  return Object.fromEntries(Object.entries(row).filter(([key]) => !forbidden.has(key)));
}

async function route(req: http.IncomingMessage, res: http.ServerResponse) {
  const url = new URL(req.url || "/", `http://${req.headers.host || "localhost"}`);
  if (req.method === "GET" && url.pathname === "/health") return json(res, 200, { status: "ok" });
  if (req.method !== "GET" || !url.pathname.startsWith("/treasury")) return json(res, 404, { error: "Not found" });
  try {
    if (url.pathname === "/treasury/balances") return json(res, 200, await source.balances());
    if (url.pathname === "/treasury/stats") {
      const stats = await source.stats();
      return json(res, stats.lastIndexedLedger && stats.lastIndexedLedger < maxIndexerLag ? 503 : 200, stats, stats.lastIndexedLedger);
    }
    const match = url.pathname.match(/^\/treasury\/(fee-sweeps|grants|governance)(?:\/([^/]+))?$/);
    if (match) {
      const kind = match[1] === "fee-sweeps" ? "fee-sweeps" : match[1] === "grants" ? "grants" : "governance";
      if (match[2]) {
        const row = await source.byTxHash(decodeURIComponent(match[2]));
        return row ? json(res, 200, scrub(row)) : json(res, 404, { error: "Record not found" });
      }
      const result = await source.events(kind, url.searchParams);
      const key = kind === "fee-sweeps" ? "sweeps" : kind === "grants" ? "grants" : "proposals";
      return json(res, 200, { [key]: result.rows.map(scrub), total: result.total, lastIndexedLedger: result.lastIndexedLedger }, result.lastIndexedLedger);
    }
    return json(res, 404, { error: "Not found" });
  } catch (error) {
    return json(res, 503, { error: "Treasury data temporarily unavailable", details: error instanceof Error ? error.message : "upstream failure" });
  }
}

http.createServer((req, res) => void route(req, res)).listen(port, () => {
  console.log(`treasury-api listening on ${port}`);
});
