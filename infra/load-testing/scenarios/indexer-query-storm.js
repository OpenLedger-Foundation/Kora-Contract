/**
 * Indexer query storm: exercises wide-but-bounded filters and deep pagination
 * without generating high-cardinality labels or unbounded query parameters.
 */
import { check } from "k6";
import { Trend, Rate } from "k6/metrics";
import { apiGet, assertNotProduction, STANDARD_THRESHOLDS, pick, thinkTime } from "../lib/common.js";

const pages = ["1", "2", "5", "10", "20"];
const statuses = ["active", "funded", "settled"];
const queryDuration = new Trend("indexer_query_duration", true);
const querySuccess = new Rate("indexer_query_success");
export const options = {
  scenarios: { indexer_query_storm: { executor: "ramping-vus", startVUs: 0, stages: [{ duration: "30s", target: 40 }, { duration: "3m", target: 40 }, { duration: "30s", target: 0 }] } },
  thresholds: { ...STANDARD_THRESHOLDS, "http_req_duration{component:indexer}": ["p(95)<700", "p(99)<1500"], "http_req_failed{component:indexer}": ["rate<0.01"] },
};
export function setup() { assertNotProduction(); }
export default function () {
  const page = pick(pages);
  const status = pick(statuses);
  const response = apiGet(`/api/v1/listings?status=${status}&page=${page}&limit=50`, { tags: { type: "read", component: "indexer" } });
  queryDuration.add(response.timings.duration);
  querySuccess.add(response.status === 200);
  check(response, { "indexer listing query: success": (r) => r.status === 200, "indexer listing query: bounded response": (r) => r.body.length < 5_000_000 });
  thinkTime(100, 500);
}
