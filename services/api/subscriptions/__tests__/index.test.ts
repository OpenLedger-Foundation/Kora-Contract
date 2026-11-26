/**
 * Barrel smoke test — exercises the public re-export surface so the package
 * entry point is covered. Issue #769.
 */

import * as api from "../index";

describe("subscriptions package public API", () => {
  it("exports the public surface", () => {
    expect(typeof api.handleSseConnection).toBe("function");
    expect(typeof api.extractSessionToken).toBe("function");
    expect(typeof api.SubscriptionAuthz).toBe("function");
    expect(typeof api.SubscriptionRateLimiter).toBe("function");
    expect(typeof api.createHmacSessionVerifier).toBe("function");
    expect(typeof api.issueSessionToken).toBe("function");
    expect(typeof api.EventBus).toBe("function");
    expect(typeof api.ingestRawEvent).toBe("function");
  });
});
