// scripts/perf-m4.mjs 의 단계 · 복사 이름 · 출력 읽기 — 앱 · 서버 없이 돈다.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  APP,
  FAKE,
  REPO_COPY,
  SYMBOLS_REPO,
  durationMs,
  lockedIn,
  measurements,
  parseIndexPerf,
  reportCopies,
} from "./perf-m4.mjs";

test("each step runs the scenario it is about on the release bundle", () => {
  const steps = measurements({ short: false, ra: "/servers/rust-analyzer" });
  const by = Object.fromEntries(steps.map((s) => [s.kind, s.args]));
  assert.deepEqual(Object.keys(by), [
    "rust-index",
    "empty",
    "lsp",
    "typing-on",
    "typing-off",
    "symbols",
    "classes",
    "ra-idle",
  ]);
  assert.equal(by["rust-index"], undefined, "the Host-only step runs no app");
  const arg = (args, flag) => (args.includes(flag) ? args[args.indexOf(flag) + 1] : undefined);
  for (const [kind, args] of Object.entries(by))
    if (args) assert.equal(arg(args, "--app"), APP, kind);
  assert.equal(arg(by.empty, "--scenario"), undefined);
  assert.equal(arg(by.lsp, "--scenario"), "lsp");
  assert.equal(arg(by.lsp, "--lsp-fake"), FAKE);
  // 켬 · 끔: 끔에는 가짜 서버를 끼우지 않는다.
  assert.equal(arg(by["typing-on"], "--scenario"), "lsp-typing");
  assert.equal(arg(by["typing-on"], "--lsp-fake"), FAKE);
  assert.equal(arg(by["typing-off"], "--scenario"), "lsp-typing-off");
  assert.equal(arg(by["typing-off"], "--lsp-fake"), undefined);
  assert.equal(arg(by.symbols, "--project"), SYMBOLS_REPO);
  assert.equal(arg(by.classes, "--scenario"), "classes");
  assert.equal(arg(by["ra-idle"], "--project"), REPO_COPY);
  assert.equal(arg(by["ra-idle"], "--lsp-server"), "/servers/rust-analyzer");
  const short = measurements({ short: true, ra: "x" });
  for (const s of short)
    if (s.args) assert.deepEqual(s.args.slice(-4), ["--runs", "1", "--warmup", "0"]);
  assert.deepEqual(
    measurements({ short: false, only: ["lsp", "empty"], ra: "x" }).map((s) => s.kind),
    ["empty", "lsp"],
  );
  assert.throws(() => measurements({ only: ["nope"] }), /unknown step nope/);
});

test("results are copied to dated report names only after measuring", () => {
  const pairs = reportCopies({ date: "2026-11-03", stamp: "101500", suffix: "", kinds: ["lsp"] });
  assert.deepEqual(pairs, [
    [
      "target/perf/m4-conditions-2026-11-03-101500.json",
      "docs/superpowers/reports/2026-11-03-m4-perf-conditions-101500.json",
    ],
    [
      "target/perf/m4-lsp-2026-11-03-101500/result.json",
      "docs/superpowers/reports/2026-11-03-m4-perf-lsp-101500.json",
    ],
    [
      "target/perf/m4-lsp-2026-11-03-101500/summary.md",
      "docs/superpowers/reports/2026-11-03-m4-perf-lsp-101500.summary.md",
    ],
  ]);
  // 측정 도중에 쓰는 것(원본)은 모두 target/perf 아래다 — 추적 경로에 쓰면 다음 단계가 dirty 로 잰다.
  for (const [src] of reportCopies({ date: "d", stamp: "s", suffix: "-short", kinds: ["a", "b"] }))
    assert.match(src, /^target\/perf\//);
});

test("the pw-index perf test's output is read", () => {
  const stdout = [
    "machine: Mac17,9 · Apple M5 Pro · 15 logical cores",
    "first index: 912.345678ms (target < 15s)",
    "memory: heap +38.8 MiB (target ≤ 120 MiB), rss +52.1 MiB, symbol index estimate 30.2 MiB",
    "symbols: 500000",
    'query "h": 7.1ms (1000 matched)',
    "query per keystroke: median 6.7ms, max 15.733ms (target < 30ms)",
    "test twenty_thousand_files_index_within_the_targets ... ok",
  ].join("\n");
  assert.deepEqual(parseIndexPerf(stdout), {
    firstIndexMs: 912.345678,
    heapMiB: 38.8,
    rssMiB: 52.1,
    queryMedianMs: 6.7,
    queryMaxMs: 15.733,
    symbols: 500000,
  });
  assert.equal(durationMs("1.6s"), 1600);
  assert.ok(Math.abs(durationMs("830µs") - 0.83) < 1e-9);
  assert.throws(() => parseIndexPerf("nothing"), /no first index/);
});

test("the screen lock is read from ioreg", () => {
  assert.equal(lockedIn("<key>CGSSessionScreenIsLocked</key>\n\t\t\t<true/>"), true);
  assert.equal(lockedIn("<key>CGSSessionScreenIsLocked</key>\n<false/>"), false);
  assert.equal(lockedIn("<key>Other</key>"), false);
});
