// scripts/perf-m4.mjs 의 단계 · 복사 이름 · 출력 읽기 — 앱 · 서버 없이 돈다.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  APP,
  FAKE,
  REPO_COPY,
  SYMBOLS_REPO,
  TYPING_ROUNDS,
  alternateArgs,
  alternateRuns,
  alternateOut,
  baselineDisplays,
  durationMs,
  lockedIn,
  measurements,
  parseIndexPerf,
  reportCopies,
  stepSpawn,
  typingRun,
  typingSummary,
} from "./perf-m4.mjs";

test("each step runs the scenario it is about on the release bundle", () => {
  const steps = measurements({ short: false, ra: "/servers/rust-analyzer" });
  const typing = steps.find((s) => s.kind === "typing");
  const by = Object.fromEntries([
    ...steps.filter((s) => !s.alternate).map((s) => [s.kind, s.args]),
    ...typing.alternate.map((s) => [s.kind, s.args]),
  ]);
  assert.deepEqual(
    steps.map((s) => s.kind),
    ["rust-index", "empty", "lsp", "typing", "symbols", "classes", "ra-idle"],
  );
  // 켬 · 끔은 회 단위로 번갈아 돈다 — 켬이 먼저, 회마다 한 번씩.
  assert.deepEqual(
    typing.alternate.map((s) => s.kind),
    ["typing-on", "typing-off"],
  );
  assert.equal(typing.rounds, TYPING_ROUNDS);
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
  // 실 서버 단계만 사본의 mise 설정을 신뢰한다(그 사본 경로만 — 다른 단계는 환경을 바꾸지 않는다).
  const env = Object.fromEntries(
    [...steps, ...typing.alternate].filter((s) => !s.alternate).map((s) => [s.kind, s.env]),
  );
  assert.match(env["ra-idle"].MISE_TRUSTED_CONFIG_PATHS, /\/target\/perf-m4\/repo$/);
  for (const [kind, e] of Object.entries(env))
    if (kind !== "ra-idle") assert.equal(e, undefined, kind);
  const short = measurements({ short: true, ra: "x" });
  for (const s of short)
    if (s.args) assert.deepEqual(s.args.slice(-4), ["--runs", "1", "--warmup", "0"]);
  const shortTyping = short.find((s) => s.kind === "typing");
  assert.equal(shortTyping.rounds, 1);
  assert.deepEqual(alternateArgs(shortTyping, shortTyping.alternate[0], 0).slice(-4), [
    "--runs",
    "1",
    "--warmup",
    "0",
  ]);
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

test("typing on and off alternate run by run, one measured run each, one warm-up in the first pair", () => {
  const [typing] = measurements({ short: false, only: ["typing"], ra: "x" });
  const order = alternateRuns(typing).map(({ round, side, args }) => {
    const at = (flag) => args[args.indexOf(flag) + 1];
    assert.equal(
      alternateOut(typing, side, round, "", "2026-11-10", "230000"),
      `target/perf/m4-typing-2026-11-10-230000/${side.kind}-r${round}`,
    );
    return `${side.kind}:${at("--runs")}:${at("--warmup")}`;
  });
  assert.deepEqual(order, [
    "typing-on:1:1",
    "typing-off:1:1",
    ...Array.from({ length: TYPING_ROUNDS - 1 }, () => ["typing-on:1:0", "typing-off:1:0"]).flat(),
  ]);
});

test("the typing verdict compares the medians of the per-run p95 and lists each pair", () => {
  const run = (round, lsp, p95) => ({ round, lsp, typing: p95 === null ? null : { p95 } });
  const s = typingSummary([
    run(0, true, 17),
    run(0, false, 14),
    run(1, true, 16),
    run(1, false, 15),
    run(2, true, 18),
    run(2, false, null), // 제외된 회 — 빼고 센다
  ]);
  assert.deepEqual(s.on, { n: 3, median: 17, min: 16, max: 18 });
  assert.deepEqual(s.off, { n: 2, median: 14.5, min: 14, max: 15 });
  assert.equal(s.deltaMs, 2.5);
  assert.deepEqual(s.pairDeltasMs, [3, 1]);
  assert.equal(s.met, false);
  assert.equal(typingSummary([run(0, true, 15), run(0, false, 13)]).met, true);
  assert.equal(typingSummary([run(0, true, 15)]).met, null);
  // pw-perf 결과에서는 준비 · 제외가 아닌 회의 값을 읽는다.
  const painted = { p95: 16, p50: 9, max: 20, within16_7: 0.93 };
  const result = {
    runs: [
      { warmup: true, measure: { typing: { painted: { p95: 99 }, frame: { p95: 99 } } } },
      { warmup: false, excluded: null, measure: { typing: { painted, frame: { p95: 14 } } } },
    ],
  };
  assert.deepEqual(typingRun(result), { ...painted, frameP95: 14 });
  assert.equal(typingRun({ runs: [{ warmup: false, excluded: "hidden", measure: {} }] }), null);
});

test("a step starts only on the baseline displays: one built-in display, no external monitor", () => {
  const builtIn = { name: "Color LCD", resolution: "1512 x 982 @ 120.00Hz", internal: true };
  const external = { name: "DP", resolution: "2560 x 1440 @ 75.00Hz", internal: false };
  assert.equal(baselineDisplays([builtIn]).ok, true);
  assert.equal(baselineDisplays([external, external]).ok, false);
  assert.equal(baselineDisplays([builtIn, external]).ok, false);
  assert.equal(baselineDisplays([external]).ok, false);
  assert.equal(baselineDisplays(null).ok, false);
  assert.match(baselineDisplays([builtIn, external]).why, /DP 2560 x 1440 @ 75\.00Hz/);
});

test("only the real-server step runs pw-perf with the copy's mise config trusted", () => {
  const steps = measurements({ short: false, ra: "/servers/rust-analyzer" });
  const base = { PATH: "/usr/bin", MISE_TRUSTED_CONFIG_PATHS: undefined };
  const ra = steps.find((s) => s.kind === "ra-idle");
  const spawn = stepSpawn(ra, ra.args, "target/perf/x", base);
  assert.equal(spawn.command, "cargo");
  assert.deepEqual(spawn.args.slice(0, 5), ["run", "-q", "--release", "-p", "pw-perf"]);
  assert.deepEqual(spawn.args.slice(-2), ["--out", "target/perf/x"]);
  assert.match(spawn.options.env.MISE_TRUSTED_CONFIG_PATHS, /\/target\/perf-m4\/repo$/);
  assert.equal(spawn.options.env.PATH, "/usr/bin");
  assert.equal(
    base.MISE_TRUSTED_CONFIG_PATHS,
    undefined,
    "the caller's environment is not changed",
  );
  const empty = steps.find((s) => s.kind === "empty");
  assert.equal(
    stepSpawn(empty, empty.args, "o", base).options.env.MISE_TRUSTED_CONFIG_PATHS,
    undefined,
  );
  assert.equal(stepSpawn(empty, empty.args, "o", base).options.env.PATH, "/usr/bin");
});
