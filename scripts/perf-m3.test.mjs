// scripts/perf-m3.mjs 의 판정 · 픽스처 · 단계 — 도커 · 앱 없이 돈다.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  FAKE_FEATURE,
  FAKE_MARK,
  bundleProblem,
  contains,
  measurements,
  parseLoad,
  pgSeed,
  redisSeed,
} from "./perf-m3.mjs";

test("the load average is read from sysctl", () => {
  assert.deepEqual(parseLoad("{ 2.75 3.69 2.33 }\n"), [2.75, 3.69, 2.33]);
  assert.throws(() => parseLoad("nope"), /cannot read the load average/);
});

test("the default bundle must not carry the fake driver and the fake one must", () => {
  const plain = Buffer.from("\0pitwall\0postgres\0");
  const fake = Buffer.concat([plain, Buffer.from(FAKE_MARK)]);
  assert.equal(contains(fake, FAKE_MARK), true);
  assert.equal(contains(plain, FAKE_MARK), false);
  assert.equal(bundleProblem(plain, fake), undefined);
  assert.match(bundleProblem(fake, fake), /must not ship/);
  assert.match(
    bundleProblem(plain, plain),
    new RegExp(`${FAKE_FEATURE} bundle has no fake driver`),
  );
});

test("each measurement runs on the bundle it is about", () => {
  const steps = measurements({ pg: "127.0.0.1:1", redis: "127.0.0.1:2", short: false });
  const by = Object.fromEntries(steps.map((s) => [s.kind, s.args]));
  assert.deepEqual(Object.keys(by), ["empty-default", "empty-fake", "db-idle", "db-fake", "db-pg"]);
  const app = (args) => args[args.indexOf("--app") + 1];
  assert.match(app(by["empty-default"]), /perf-m3\/default\//);
  assert.match(app(by["empty-fake"]), /perf-m3\/fake\//);
  // 출시와 같은 번들로: 유휴(DB 연결) · 실 PG 그리드. 가짜 번들로: 가짜 postgres + Redis.
  assert.match(app(by["db-idle"]), /default/);
  assert.match(app(by["db-pg"]), /default/);
  assert.match(app(by["db-fake"]), /fake/);
  assert.ok(by["db-fake"].includes("--fake-db"));
  assert.ok(!by["db-pg"].includes("--fake-db"));
  assert.deepEqual(
    by["db-idle"].slice(by["db-idle"].indexOf("--pg"), by["db-idle"].indexOf("--pg") + 2),
    ["--pg", "127.0.0.1:1/perf"],
  );
  const short = measurements({ pg: "a:1", redis: "b:2", short: true });
  for (const s of short) assert.deepEqual(s.args.slice(-4), ["--runs", "1", "--warmup", "0"]);
  const only = measurements({
    pg: "a:1",
    redis: "b:2",
    short: false,
    only: ["db-pg", "empty-fake"],
  });
  assert.deepEqual(
    only.map((s) => s.kind),
    ["empty-fake", "db-pg"],
  );
  assert.throws(
    () => measurements({ pg: "a:1", redis: "b:2", short: false, only: ["db"] }),
    /unknown step db/,
  );
});

test("the Redis seed is RESP MSET of k:<i> = <i>", () => {
  const seed = redisSeed(3, 2);
  assert.equal(
    seed,
    "*5\r\n$4\r\nMSET\r\n$3\r\nk:0\r\n$1\r\n0\r\n$3\r\nk:1\r\n$1\r\n1\r\n" +
      "*3\r\n$4\r\nMSET\r\n$3\r\nk:2\r\n$1\r\n2\r\n",
  );
});

test("the PG seed makes rows shaped like the fake driver's", () => {
  const sql = pgSeed(10);
  assert.match(sql, /generate_series\(0, 9\)/);
  assert.match(sql, /'r' \|\| i \|\| 'c1' AS c1/);
  assert.match(sql, /'r' \|\| i \|\| 'c9' AS c9/);
  assert.doesNotMatch(sql, /c10/);
  assert.match(sql, /CREATE VIEW perf_stream/);
});
