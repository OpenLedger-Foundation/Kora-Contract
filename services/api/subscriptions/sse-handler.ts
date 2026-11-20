/**
 * SSE handler — Server-Sent Events transport for Kora subscriptions.
 *
 * Attach this handler to any HTTP framework.  The caller is responsible for
 * authentication; pass `authenticatedAs` (the verified investor address) into
 * the handler so that per-user scoping is enforced in the EventBus.
 *
 * Usage (Express):
 *   app.get("/subscriptions/sse", (req, res) => {
 *     const addr = req.user.address; // from JWT / session middleware
 *     handleSseConnection(req, res, addr);
 *   });
 *
 * Issue: #769
 */

import { IncomingMessage, ServerResponse } from "http";
import { EventBus } from "./event-bus";
import { EventEnvelope, SubscriptionFilter, SubscriptionTopic } from "./types";

/** Keep-alive ping interval (ms). */
const KEEPALIVE_MS = 15_000;

export function handleSseConnection(
  req: IncomingMessage,
  res: ServerResponse,
  authenticatedAs: string | undefined,
  health?: () => boolean,
): void {
  const url = new URL(req.url ?? "/", "http://localhost");
  const topic = (url.searchParams.get("topic") ?? "") as SubscriptionTopic;
  const invoiceIdParam = url.searchParams.get("invoiceId");
  const afterCursor = url.searchParams.get("after") ?? undefined;

  const filter: SubscriptionFilter = {};
  if (invoiceIdParam) filter.invoiceId = BigInt(invoiceIdParam);

  res.setHeader("Content-Type", "text/event-stream");
  res.setHeader("Cache-Control", "no-cache");
  res.setHeader("Connection", "keep-alive");
  res.setHeader("X-Accel-Buffering", "no");
  res.flushHeaders?.();

  const send = (env: EventEnvelope): void => {
    res.write(`id: ${env.cursor}\n`);
    res.write(`data: ${JSON.stringify(env.event, (_k, v) =>
      typeof v === "bigint" ? v.toString() : v
    )}\n\n`);
  };

  let subscriptionId: string;
  try {
    subscriptionId = EventBus.instance.subscribe(
      topic,
      filter,
      authenticatedAs,
      send,
      afterCursor
    );
  } catch (err: unknown) {
    const message = err instanceof Error ? err.message : "Subscription error";
    res.writeHead(403);
    res.end(message);
    return;
  }

  // Keep-alive ping so proxies don't time out idle connections.
  const ping = setInterval(() => {
    if (health && !health()) {
      // Do not leave clients attached to a stream backed by an unavailable
      // indexer. A reconnect lets the caller obtain a fresh cursor/state.
      res.write(`event: degraded\ndata: ${JSON.stringify({ retryAfterSeconds: 5 })}\n\n`);
      clearInterval(ping);
      EventBus.instance.unsubscribe(subscriptionId);
      res.end();
      return;
    }
    res.write(": ping\n\n");
  }, KEEPALIVE_MS);

  req.on("close", () => {
    clearInterval(ping);
    EventBus.instance.unsubscribe(subscriptionId);
  });
}
