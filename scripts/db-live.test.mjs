// scripts/db-live.mjs 의 판정(하한 · 0개 · 돈 수와 고른 수)을 실 DB 없이 본다.
// 실행: node --test scripts/db-live.test.mjs   (`mise run test` 에 든다)

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";

import { TARGETS, isReadyTest, judge, parseFloors, runCount, selectedTests } from "./db-live.mjs";

const listed = {
  "rust-suites": {
    "pw-db-driver": {
      testcases: {
        "live::tests::live_pg_ready": { ignored: false, "filter-match": { status: "matches" } },
        "live::tests::urls_give_host_port_and_password": {
          ignored: false,
          "filter-match": { status: "mismatch", reason: "expression" },
        },
      },
    },
    "pw-db-driver-pg": {
      testcases: {
        "tests::live_pg_cancel": { ignored: false, "filter-match": { status: "matches" } },
        "tests::live_pg_slow": { ignored: true, "filter-match": { status: "matches" } },
      },
    },
  },
};

test("the floor file has a line for every target and nothing else", () => {
  const floors = parseFloors(readFileSync(resolve(import.meta.dirname, "db-live.toml"), "utf8"));
  assert.deepEqual([...floors.keys()].sort(), Object.keys(TARGETS).sort());
  assert.throws(() => parseFloors("pg = 1\npg = 2"), /twice/);
  assert.throws(() => parseFloors("pg = one"), /expected/);
});

test("ready checks are not counted toward the floor", () => {
  const selected = selectedTests(listed);
  assert.deepEqual(selected, [
    "pw-db-driver live::tests::live_pg_ready",
    "pw-db-driver-pg tests::live_pg_cancel",
  ]);
  assert.equal(isReadyTest(selected[0], "live_pg"), true);
  assert.equal(isReadyTest(selected[1], "live_pg"), false);
  assert.equal(isReadyTest("x tests::live_pg_ready_twice", "live_pg"), false);

  const base = { db: "pg", filter: "live_pg", selected, run: 2, exitCode: 0 };
  assert.equal(judge({ ...base, floor: 1 }).ok, true);
  // 하한을 실제 수(준비 확인을 뺀 1)보다 크게 올리면 실패한다.
  const above = judge({ ...base, floor: 2 });
  assert.equal(above.ok, false);
  assert.match(above.message, /below the floor/);
});

test("no tests, a failed run or a count mismatch fail", () => {
  const none = judge({
    db: "redis",
    filter: "live_redis",
    selected: [],
    run: 0,
    floor: 0,
    exitCode: 0,
  });
  assert.match(none.message, /selected no tests/);
  assert.equal(none.ok, false);
  const failed = judge({
    db: "pg",
    filter: "live_pg",
    selected: ["a live_pg_ready"],
    run: 1,
    floor: 0,
    exitCode: 100,
  });
  assert.equal(failed.ok, false);
  const mismatch = judge({
    db: "pg",
    filter: "live_pg",
    selected: ["a live_pg_ready", "b live_pg_x"],
    run: 1,
    floor: 0,
    exitCode: 0,
  });
  assert.match(mismatch.message, /ran 1 but the filter selected 2/);
});

test("the run count comes from the last nextest summary line", () => {
  const output = [
    "        PASS [   0.004s] (1/1) pw-db-driver live::tests::live_pg_ready",
    "────────────",
    "     Summary [   0.005s] 1 test run: 1 passed, 0 skipped",
  ].join("\n");
  assert.equal(runCount(output), 1);
  assert.equal(runCount("Summary [   1.000s] 12 tests run: 12 passed, 3 skipped"), 12);
  assert.equal(runCount("error: no tests to run"), null);
});
