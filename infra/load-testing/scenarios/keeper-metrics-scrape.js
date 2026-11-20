/**
 * Keeper metrics scrape: validates Prometheus endpoint behavior under bounded
 * concurrent reads. Set K6_KEEPER_URL to a local or staging keeper endpoint.
 */
import http from "k6/http";
import { check } from "k6";
import { Trend, Rate } from "k6/metrics";
import { assertNotProduction, STANDARD_THRESHOLDS } from "../lib/common.js";

const keeperUrl = (__ENV.K6_KEEPER_URL || __ENV.K6_BASE_URL || "").replace(/\/$/, "");
const scrapeDuration = new Trend("keeper_metrics_scrape_duration", true);
const scrapeSuccess = new Rate("keeper_metrics_scrape_success");
export const options = {
  scenarios: { keeper_metrics: { executor: "ramping-vus", startVUs: 0, stages: [{ duration: "30s", target: 25 }, { duration: "2m", target: 25 }, { duration: "30s", target: 0 }] } },
  thresholds: { ...STANDARD_THRESHOLDS, "http_req_duration{component:keeper}": ["p(95)<500"], "http_req_failed{component:keeper}": ["rate<0.01"] },
};
export function setup() { assertNotProduction(); if (!keeperUrl) throw new Error("K6_KEEPER_URL or K6_BASE_URL is required"); }
export default function () {
  const response = http.get(`${keeperUrl}/metrics`, { tags: { type: "read", component: "keeper" } });
  scrapeDuration.add(response.timings.duration);
  scrapeSuccess.add(response.status === 200);
  check(response, { "keeper metrics: 200": (r) => r.status === 200, "keeper metrics: prometheus text": (r) => r.body.includes("kora_keeper_jobs") });
}
