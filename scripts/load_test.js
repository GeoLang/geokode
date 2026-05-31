// k6 load test for geokode-server
// Usage: k6 run scripts/load_test.js
import http from "k6/http";
import { check, sleep } from "k6";

const BASE_URL = __ENV.BASE_URL || "http://localhost:3001";

export const options = {
  stages: [
    { duration: "10s", target: 10 },
    { duration: "30s", target: 50 },
    { duration: "10s", target: 0 },
  ],
  thresholds: {
    http_req_duration: ["p(95)<500"],
    http_req_failed: ["rate<0.01"],
  },
};

export default function () {
  // Health check
  const health = http.get(`${BASE_URL}/health`);
  check(health, { "health 200": (r) => r.status === 200 });

  // Forward geocode
  const forward = http.get(`${BASE_URL}/forward?q=10+Downing+Street`);
  check(forward, { "forward 200": (r) => r.status === 200 });

  // Reverse geocode
  const reverse = http.get(`${BASE_URL}/reverse?lon=-0.1276&lat=51.5034&limit=3`);
  check(reverse, { "reverse 200": (r) => r.status === 200 });

  // Autocomplete
  const auto = http.get(`${BASE_URL}/autocomplete?q=Buckingham&limit=5`);
  check(auto, { "autocomplete 200": (r) => r.status === 200 });

  sleep(0.1);
}
