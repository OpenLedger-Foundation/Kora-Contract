/**
 * Reporting Service — Treasury Transparency REST API Tests
 * Issue #1099
 */

import * as http from "http";
import * as os from "os";
import * as path from "path";
import * as fs from "fs";
import { createReportingServer } from "../src/httpServer";
import { ExportJobQueue } from "../src/exportJobQueue";
import { FixtureReportingDataSource } from "../src/fixtureDataSource";
import { IndexerReportingDataSource } from "../src/indexerDataSource";

function requestJson(
  server: http.Server,
  urlPath: string,
  headers: Record<string, string> = {}
): Promise<{ status: number; body: Record<string, unknown> }> {
  return new Promise((resolve, reject) => {
    const addr = server.address();
    if (!addr || typeof addr === "string") {
      return reject(new Error("Server not listening"));
    }
    const req = http.request(
      {
        hostname: "127.0.0.1",
        port: addr.port,
        path: urlPath,
        method: "GET",
        headers,
      },
      (res) => {
        let raw = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => (raw += chunk));
        res.on("end", () => {
          try {
            resolve({
              status: res.statusCode ?? 0,
              body: JSON.parse(raw) as Record<string, unknown>,
            });
          } catch (err) {
            reject(err);
          }
        });
      }
    );
    req.on("error", reject);
    req.end();
  });
}

describe("Treasury Transparency REST API (Issue #1099)", () => {
  let tmpDir: string;
  let queue: ExportJobQueue;
  let server: http.Server;

  beforeEach(async () => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "kora-treasury-test-"));
    const ds = new FixtureReportingDataSource();
    queue = new ExportJobQueue(path.join(tmpDir, "jobs.db"), ds, tmpDir);
    server = createReportingServer(queue, tmpDir, ds);
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  });

  afterEach(async () => {
    await new Promise<void>((resolve) => server.close(() => resolve()));
    queue.close();
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  test("allows public read access without API key", async () => {
    const [balRes, infRes, outRes] = await Promise.all([
      requestJson(server, "/treasury/balances"),
      requestJson(server, "/treasury/inflows"),
      requestJson(server, "/treasury/outflows"),
    ]);

    expect(balRes.status).toBe(200);
    expect(infRes.status).toBe(200);
    expect(outRes.status).toBe(200);
  });

  test("returns per-token balances, inflows, and outflows", async () => {
    const balRes = await requestJson(server, "/treasury/balances");
    expect(balRes.status).toBe(200);
    expect(balRes.body["lastIndexedLedger"]).toBe(51200000);
    expect(balRes.body["partial"]).toBe(false);
    const balances = balRes.body["balances"] as Array<Record<string, unknown>>;
    expect(balances).toHaveLength(2);
    expect(balances[0]).toMatchObject({
      asset: "USDC",
      balance: "2450000000",
      totalInflows: "3000000000",
      totalOutflows: "550000000",
    });

    const infRes = await requestJson(server, "/treasury/inflows?asset=USDC&limit=1");
    expect(infRes.status).toBe(200);
    expect(infRes.body["total"]).toBe(2);
    expect((infRes.body["inflows"] as unknown[]).length).toBe(1);

    const outRes = await requestJson(server, "/treasury/outflows");
    expect(outRes.status).toBe(200);
    expect(outRes.body["total"]).toBe(1);
    const outflows = outRes.body["outflows"] as Array<Record<string, unknown>>;
    expect(outflows[0]).toMatchObject({
      asset: "USDC",
      amount: "550000000",
      recipient: "GFIXTURE_TREASURY_OPS_RECIPIENT",
    });
  });

  test("marks partial indexer lag in response", async () => {
    const mockFetch = async (input: RequestInfo | URL): Promise<Response> => {
      const url = String(input);
      if (url.endsWith("/status")) {
        return new Response(
          JSON.stringify({
            lastIndexedLedger: 51199900,
            latestChainLedger: 51200050,
          }),
          { status: 200, headers: { "Content-Type": "application/json" } }
        );
      }
      return new Response(
        JSON.stringify({
          events: [
            {
              contract: "treasury",
              event_type: "fee_collected",
              ledger_sequence: 51199900,
              event_index: 0,
              decoded_payload: {
                asset: "USDC",
                feeAmount: "100000000",
                invoiceId: "inv-lag-1",
                txHash: "0xlag1",
                timestamp: 1700000000,
              },
            },
          ],
          next_cursor: null,
        }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      );
    };

    const lagDs = new IndexerReportingDataSource({
      indexerUrl: "http://mock-indexer",
      fetchFn: mockFetch as typeof fetch,
    });
    const lagServer = createReportingServer(queue, tmpDir, lagDs);
    await new Promise<void>((resolve) =>
      lagServer.listen(0, "127.0.0.1", resolve)
    );

    try {
      const res = await requestJson(lagServer, "/treasury/balances");
      expect(res.status).toBe(200);
      expect(res.body["partial"]).toBe(true);
      expect(res.body["lastIndexedLedger"]).toBe(51199900);
      expect(res.body["latestChainLedger"]).toBe(51200050);
      expect(typeof res.body["lagWarning"]).toBe("string");
    } finally {
      await new Promise<void>((resolve) => lagServer.close(() => resolve()));
    }
  });

  test("handles empty treasury cleanly", async () => {
    const emptyFetch = async (input: RequestInfo | URL): Promise<Response> => {
      const url = String(input);
      if (url.endsWith("/status")) {
        return new Response(
          JSON.stringify({
            lastIndexedLedger: 51200000,
            latestChainLedger: 51200000,
          }),
          { status: 200, headers: { "Content-Type": "application/json" } }
        );
      }
      return new Response(
        JSON.stringify({ events: [], next_cursor: null }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      );
    };

    const emptyDs = new IndexerReportingDataSource({
      indexerUrl: "http://mock-indexer",
      fetchFn: emptyFetch as typeof fetch,
    });
    const emptyServer = createReportingServer(queue, tmpDir, emptyDs);
    await new Promise<void>((resolve) =>
      emptyServer.listen(0, "127.0.0.1", resolve)
    );

    try {
      const [balRes, infRes, outRes] = await Promise.all([
        requestJson(emptyServer, "/treasury/balances"),
        requestJson(emptyServer, "/treasury/inflows"),
        requestJson(emptyServer, "/treasury/outflows"),
      ]);

      expect(balRes.status).toBe(200);
      expect(balRes.body["balances"]).toEqual([]);
      expect(balRes.body["partial"]).toBe(false);

      expect(infRes.status).toBe(200);
      expect(infRes.body["inflows"]).toEqual([]);
      expect(infRes.body["total"]).toBe(0);

      expect(outRes.status).toBe(200);
      expect(outRes.body["outflows"]).toEqual([]);
      expect(outRes.body["total"]).toBe(0);
    } finally {
      await new Promise<void>((resolve) => emptyServer.close(() => resolve()));
    }
  });
});
