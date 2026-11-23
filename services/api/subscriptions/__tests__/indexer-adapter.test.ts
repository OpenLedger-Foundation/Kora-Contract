/**
 * Indexer adapter branch-coverage tests — issue #769.
 *
 * The main subscription-service test suite covers the common event topics
 * (INV_FUNDED, POS_RECORDED, PROTOCOL_REPAYMENT). These tests cover the
 * remaining branches: PROTOCOL_YIELD_DIST, short topics, and unknown topics.
 */

import { EventBus } from "../event-bus";
import { ingestRawEvent, RawSorobanEvent } from "../indexer-adapter";
import { PositionStatusChangeEvent } from "../types";

function freshBus(): EventBus {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return new (EventBus as any)();
}

describe("indexer-adapter — branch coverage", () => {
  it("maps PROTOCOL_YIELD_DIST into a position-status-change event", () => {
    const bus = freshBus();
    const received: PositionStatusChangeEvent[] = [];
    bus.subscribe(
      "position-status-change",
      {},
      "INVESTOR_A",
      (env) => received.push(env.event as PositionStatusChangeEvent),
    );

    const raw: RawSorobanEvent = {
      topics: ["SCHEMA_V", "PROTOCOL_YIELD_DIST", "1"],
      data: [1, "INVESTOR_A", 750, Math.floor(Date.now() / 1000)],
      ledger: 200,
    };
    ingestRawEvent(raw, bus);

    expect(received).toHaveLength(1);
    expect(received[0].investor).toBe("INVESTOR_A");
    expect(received[0].yieldClaimed).toBe(750n);
    expect(received[0].contributed).toBe(0n);
  });

  it("ignores events with fewer than two topics", () => {
    const bus = freshBus();
    const received: unknown[] = [];
    bus.subscribe("repayment", {}, undefined, (env) => received.push(env));
    ingestRawEvent(
      { topics: ["SCHEMA_V"], data: [1, 2, 3], ledger: 1 } as RawSorobanEvent,
      bus,
    );
    expect(received).toHaveLength(0);
  });

  it("ignores events whose topic is not in the known set", () => {
    const bus = freshBus();
    const received: unknown[] = [];
    bus.subscribe("repayment", {}, undefined, (env) => received.push(env));
    ingestRawEvent(
      {
        topics: ["SCHEMA_V", "UNKNOWN_TOPIC", "1"],
        data: [1, 2, 3],
        ledger: 1,
      },
      bus,
    );
    expect(received).toHaveLength(0);
  });

  it("INV_FUNDED falls back to fundedAmount when total/face value are absent", () => {
    const bus = freshBus();
    const received: unknown[] = [];
    bus.subscribe(
      "listing-funding-progress",
      { invoiceId: 7n },
      undefined,
      (env) => received.push(env),
    );
    // Only 4 data elements: data[4] (totalFunded) and data[5] (faceValue) are
    // undefined, exercising the `?? fundedAmount` fallback branches.
    ingestRawEvent(
      {
        topics: ["SCHEMA_V", "INV_FUNDED", "1"],
        data: ["INV_A", 7, 500, 999],
        ledger: 1,
      },
      bus,
    );
    expect(received).toHaveLength(1);
  });

  it("reports 0 progress when the funded amount is zero (faceValue === 0n branch)", () => {
    const bus = freshBus();
    const received: { event: { progressBps: number } }[] = [];
    bus.subscribe(
      "listing-funding-progress",
      {},
      undefined,
      (env) => received.push(env as { event: { progressBps: number } }),
    );
    ingestRawEvent(
      {
        topics: ["SCHEMA_V", "INV_FUNDED", "1"],
        data: ["INV_A", 1, 0, 123],
        ledger: 1,
      },
      bus,
    );
    expect(received).toHaveLength(1);
    expect(received[0].event.progressBps).toBe(0);
  });

  it("PROTOCOL_YIELD_DIST position event is scoped to the owning investor", () => {
    const bus = freshBus();
    const eventsA: PositionStatusChangeEvent[] = [];
    bus.subscribe(
      "position-status-change",
      {},
      "INVESTOR_A",
      (env) => eventsA.push(env.event as PositionStatusChangeEvent),
    );
    ingestRawEvent(
      {
        topics: ["SCHEMA_V", "PROTOCOL_YIELD_DIST", "1"],
        data: [9, "INVESTOR_B", 100, 555],
        ledger: 5,
      },
      bus,
    );
    // INVESTOR_A's subscriber must not receive INVESTOR_B's yield event.
    expect(eventsA).toHaveLength(0);
  });
});
