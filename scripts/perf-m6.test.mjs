// scripts/perf-m6.mjs 의 단계 · 복사 이름 — 앱 없이 돈다.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  ADAPTER_PACKAGES,
  APP,
  DRIVER,
  measurements,
  reportCopies,
  stepSpawn,
} from "./perf-m6.mjs";

const arg = (args, flag) => (args.includes(flag) ? args[args.indexOf(flag) + 1] : undefined);

test("the empty project is pw-perf as at the end of M5 and the M6 steps run the driver", () => {
  const steps = measurements({ short: false });
  assert.deepEqual(
    steps.map((s) => s.kind),
    ["empty", "idle-procs", "run-flood", "debug-end"],
  );
  const by = Object.fromEntries(steps.map((s) => [s.kind, s]));
  for (const s of steps) assert.equal(arg(s.args, "--app"), APP, s.kind);
  // M5 끝 측정(perf-m5 의 `empty`)과 같은 인자 — 시나리오 없이 기본 회 수.
  assert.equal(by.empty.tool, "pw-perf");
  assert.equal(arg(by.empty.args, "--scenario"), undefined);
  assert.equal(arg(by.empty.args, "--runs"), undefined);
  for (const kind of ["idle-procs", "run-flood", "debug-end"]) {
    assert.equal(by[kind].tool, "driver", kind);
    assert.ok(by[kind].args.includes(kind), kind);
    assert.equal(arg(by[kind].args, "--runs"), "3", kind);
  }
  // 번들 어댑터 넷 중 따로 뜨는 어댑터 프로세스가 있는 셋(java-debug 는 jdtls 안에서 돈다).
  assert.deepEqual(
    arg(by["debug-end"].args, "--adapters").split(","),
    Object.keys(ADAPTER_PACKAGES),
  );
  assert.deepEqual(Object.keys(ADAPTER_PACKAGES), ["codelldb", "js-debug", "debugpy"]);

  const short = Object.fromEntries(measurements({ short: true }).map((x) => [x.kind, x.args]));
  assert.deepEqual(short.empty.slice(-4), ["--runs", "1", "--warmup", "0"]);
  for (const kind of ["idle-procs", "run-flood", "debug-end"])
    assert.equal(arg(short[kind], "--runs"), "1");
  assert.throws(() => measurements({ only: ["nope"] }), /unknown step nope/);
});

test("steps write under target/perf and are copied to dated report names", () => {
  const [empty, idle] = measurements({ short: false });
  assert.equal(stepSpawn(empty, "target/perf/x").command, "cargo");
  const d = stepSpawn(idle, "target/perf/y");
  assert.ok(d.command.endsWith(DRIVER));
  assert.equal(arg(d.args, "--out"), "target/perf/y");
  const pairs = reportCopies({
    date: "2026-11-19",
    stamp: "101010",
    suffix: "",
    kinds: ["run-flood"],
  });
  assert.deepEqual(
    pairs.map(([, to]) => to),
    [
      "docs/superpowers/reports/2026-11-19-m6-perf-conditions-101010.json",
      "docs/superpowers/reports/2026-11-19-m6-perf-run-flood-101010.json",
      "docs/superpowers/reports/2026-11-19-m6-perf-run-flood-101010.summary.md",
    ],
  );
  for (const [from] of pairs) assert.ok(from.startsWith("target/perf/m6-"), from);
});
