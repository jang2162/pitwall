// scripts/perf-m5.mjs 의 단계 · 복사 이름 · 조건 읽기 — 앱 없이 돈다.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  APP,
  CLI_CALLS,
  DRIVER,
  MCP_FROM_SECS,
  MCP_PAIRS,
  busyProcesses,
  hidIdleSecs,
  measurements,
  reportCopies,
  stepSpawn,
} from "./perf-m5.mjs";

const arg = (args, flag) => (args.includes(flag) ? args[args.indexOf(flag) + 1] : undefined);

test("the judged step is pw-perf's empty project and the record-only steps run the driver", () => {
  const steps = measurements({ short: false });
  assert.deepEqual(
    steps.map((s) => s.kind),
    ["empty", "cli-headless", "cli-app", "mcp-idle"],
  );
  const by = Object.fromEntries(steps.map((s) => [s.kind, s]));
  for (const s of steps) assert.equal(arg(s.args, "--app"), APP, s.kind);
  // 판정은 M4 기준선과 같은 측정 — 시나리오 없이 기본 회 수.
  assert.equal(by.empty.tool, "pw-perf");
  assert.equal(arg(by.empty.args, "--scenario"), undefined);
  assert.equal(arg(by.empty.args, "--runs"), undefined);
  // 기록 항목은 pw-perf 의 시나리오가 아니다(결정 M5-D19).
  for (const kind of ["cli-headless", "cli-app", "mcp-idle"]) {
    assert.equal(by[kind].tool, "driver", kind);
    assert.ok(by[kind].args.includes(kind), kind);
  }
  assert.equal(arg(by["cli-app"].args, "--calls"), String(CLI_CALLS));
  assert.equal(CLI_CALLS, 20, "M5-22: the median of 20 calls");
  // MCP 세션은 붙은 뒤 40초 이후 표본으로, 대조 회와 함께(첫 측정이 넘긴 것).
  assert.equal(arg(by["mcp-idle"].args, "--from-secs"), String(MCP_FROM_SECS));
  assert.equal(MCP_FROM_SECS, 40);
  assert.equal(arg(by["mcp-idle"].args, "--pairs"), String(MCP_PAIRS));
  assert.ok(Number(arg(by["mcp-idle"].args, "--hold-secs")) >= MCP_FROM_SECS);

  const short = measurements({ short: true });
  const s = Object.fromEntries(short.map((x) => [x.kind, x.args]));
  assert.deepEqual(s.empty.slice(-4), ["--runs", "1", "--warmup", "0"]);
  assert.equal(arg(s["cli-headless"], "--calls"), "3");
  assert.equal(arg(s["mcp-idle"], "--pairs"), "1");
  assert.equal(arg(s["mcp-idle"], "--warmup"), "0");

  assert.deepEqual(
    measurements({ short: false, only: ["mcp-idle", "empty"] }).map((x) => x.kind),
    ["empty", "mcp-idle"],
  );
  assert.throws(() => measurements({ only: ["nope"] }), /unknown step nope/);
});

test("pw-perf runs through cargo and the driver runs as the built binary", () => {
  const [empty, cli] = measurements({ short: false });
  const a = stepSpawn(empty, "target/perf/x");
  assert.equal(a.command, "cargo");
  assert.deepEqual(a.args.slice(0, 6), ["run", "-q", "--release", "-p", "pw-perf", "--"]);
  assert.deepEqual(a.args.slice(-2), ["--out", "target/perf/x"]);
  const b = stepSpawn(cli, "target/perf/y");
  assert.ok(b.command.endsWith(DRIVER), b.command);
  assert.deepEqual(b.args.slice(-2), ["--out", "target/perf/y"]);
});

test("results are copied to dated report names only after measuring", () => {
  const pairs = reportCopies({
    date: "2026-11-17",
    stamp: "235900",
    suffix: "",
    kinds: ["mcp-idle"],
  });
  assert.deepEqual(pairs, [
    [
      "target/perf/m5-conditions-2026-11-17-235900.json",
      "docs/superpowers/reports/2026-11-17-m5-perf-conditions-235900.json",
    ],
    [
      "target/perf/m5-mcp-idle-2026-11-17-235900/result.json",
      "docs/superpowers/reports/2026-11-17-m5-perf-mcp-idle-235900.json",
    ],
    [
      "target/perf/m5-mcp-idle-2026-11-17-235900/summary.md",
      "docs/superpowers/reports/2026-11-17-m5-perf-mcp-idle-235900.summary.md",
    ],
  ]);
  for (const [src] of reportCopies({ date: "d", stamp: "s", suffix: "-short", kinds: ["a"] }))
    assert.match(src, /^target\/perf\//);
});

test("the load next to the measurement is read from ps and the input idle time from ioreg", () => {
  const ps = [
    "  0.0     1 /sbin/launchd",
    "198.5   812 /usr/libexec/PerfPowerServices",
    " 25.1  4242 rustc",
    " 12.0  4243 node",
  ].join("\n");
  assert.deepEqual(busyProcesses(ps), [
    { cpu: 198.5, pid: 812, command: "/usr/libexec/PerfPowerServices" },
    { cpu: 25.1, pid: 4242, command: "rustc" },
  ]);
  assert.equal(hidIdleSecs('    | |   "HIDIdleTime" = 2129000000000\n'), 2129);
  assert.equal(hidIdleSecs("no such key"), null);
});

test("a dry run prints every step without touching the machine", () => {
  const script = fileURLToPath(new URL("perf-m5.mjs", import.meta.url));
  const r = spawnSync(process.execPath, [script, "--dry-run"], { encoding: "utf8" });
  assert.equal(r.status, 0, r.stderr);
  for (const kind of ["empty", "cli-headless", "cli-app", "mcp-idle"])
    assert.match(r.stdout, new RegExp(`perf-m5: ${kind}: `));
});
