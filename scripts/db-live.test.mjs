// scripts/db-live.mjs 의 판정(하한 · 0개 · 돈 수와 고른 수)을 실 DB 없이 본다.
// 실행: node --test scripts/db-live.test.mjs   (`mise run test` 에 든다)

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";

import {
  TARGETS,
  exitCode,
  filterExpr,
  isReadyTest,
  judge,
  parseFloors,
  runCount,
  selectedTests,
} from "./db-live.mjs";

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

test("a colored summary line still counts", () => {
  // CI(run 41153877702, db-live)의 로그에서 그대로 뽑은 줄 — `CARGO_TERM_COLOR=always`.
  const colored =
    "\x1b[32;1m     Summary\x1b[0m [   0.005s] \x1b[1m1\x1b[0m test run: \x1b[1m1\x1b[0m \x1b[32;1mpassed\x1b[0m, \x1b[1m30\x1b[0m \x1b[33;1mskipped\x1b[0m";
  assert.equal(runCount(colored), 1);
});

test("an unreadable run count fails and says so", () => {
  const unread = judge({
    db: "pg",
    filter: "live_pg",
    selected: ["a live_pg_ready"],
    run: null,
    floor: 0,
    exitCode: 0,
  });
  assert.equal(unread.ok, false);
  assert.match(unread.message, /could not read how many tests nextest ran/);
});

test("running nothing is not a pass", () => {
  const pass = { ok: true, message: "" };
  const skip = { ok: true, skipped: true, message: "" };
  const fail = { ok: false, message: "" };
  assert.equal(exitCode([skip, skip]), 3);
  assert.equal(exitCode([pass, skip]), 0);
  assert.equal(exitCode([pass, fail, skip]), 1);
});

test("tls servers start only for a running db without a tls url, and only with docker", async () => {
  const { tlsToStart } = await import("./db-live.mjs");
  const pg = TARGETS.pg;
  assert.deepEqual(tlsToStart(pg, { PW_TEST_PG_URL: "postgres://x" }, true), ["pg"]);
  assert.deepEqual(tlsToStart(pg, { PW_TEST_PG_URL: "postgres://x" }, false), []);
  assert.deepEqual(tlsToStart(pg, {}, true), [], "the db itself is not run");
  assert.deepEqual(
    tlsToStart(pg, { PW_TEST_PG_URL: "postgres://x", PW_TEST_PG_TLS_URL: "postgres://y" }, true),
    [],
    "CI started it already",
  );
  assert.deepEqual(tlsToStart(TARGETS.redis, { PW_TEST_REDIS_URL: "redis://x" }, true), ["redis"]);
  assert.deepEqual(tlsToStart(TARGETS.mysql, { PW_TEST_MYSQL_URL: "mysql://x" }, true), []);
});

test("an sshd starts only locally, for the ssh step, without a url, and only with docker", async () => {
  const { sshToStart } = await import("./db-live.mjs");
  const ssh = TARGETS.ssh_tunnel;
  assert.equal(sshToStart(ssh, {}, false, true), true);
  assert.equal(sshToStart(ssh, {}, false, false), false, "no docker");
  assert.equal(
    sshToStart(ssh, {}, true, true),
    false,
    "CI starts it in its own step and must fail without it",
  );
  assert.equal(sshToStart(ssh, { PW_TEST_SSH_URL: "ssh://x" }, false, true), false, "given");
  assert.equal(sshToStart(TARGETS.pg, {}, false, true), false, "not a db");
  assert.equal(filterExpr(ssh.filter), "test(/(^|::)live_ssh_tunnel_/)");
  assert.equal(
    isReadyTest("pw-db-driver tunnel::tests::live::live_ssh_tunnel_ready", ssh.filter),
    true,
  );
});

test("tls container names are per worktree and per run", async () => {
  const { tlsName } = await import("./db-live.mjs");
  const a = tlsName("/w/a", 1, "pg");
  assert.match(a, /^pitwall-tls-[0-9a-f]{8}-1-pg$/);
  assert.notEqual(a, tlsName("/w/b", 1, "pg"));
  assert.notEqual(a, tlsName("/w/a", 2, "pg"));
});

test("the filter picks live tests by the start of the function name only", () => {
  assert.equal(filterExpr("live_pg"), "test(/(^|::)live_pg_/)");
  const re = /(^|::)live_pg_/;
  for (const name of [
    "live::tests::live_pg_ready",
    "live_tests::live_pg_a_read_only_transaction_refuses_writes",
    "live_pg_top_level",
  ]) {
    assert.ok(re.test(name), name);
  }
  for (const name of [
    "risk::tests::hardening::functions_that_passed_read_only_on_live_pg_are_not_reads",
    "tests::not_live_pg_at_all",
  ]) {
    assert.ok(!re.test(name), name);
  }
});
