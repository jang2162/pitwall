// M5-22 측정(16-m5-plan.md — 결정 M5-D19 「가볍게」)을 한 번에 돌리고 결과를 보고서 폴더로 옮긴다(앱을 앞으로 띄우므로 사람이
// 기기를 비웠을 때만).
//
//   node scripts/perf-m5.mjs [--short] [--only <종류>,…] [--dry-run] [--max-load <1분 load>]     (`mise run perf-m5 -- --short`)
//
// 차례(측정 방법은 CONTRIBUTING.md 「성능 기준선」 의 「M5 기록 항목」):
//   ① 데스크톱 release 번들(기본 feature — 출시와 같다)을 만들어 `target/release/perf-m5/Pitwall.app` 로 옮기고, `pw-perf` 와 기록
//      항목 드라이버 `pw-perf-m5`(crates/pw-perf/src/bin — 번들에 들지 않는다)를 release 로 짓는다.
//   ② 판정: `empty` — pw-perf 의 빈 프로젝트 콜드 스타트 · 유휴 메모리(M4 기준선 · 06 「성능 목표」 와 같은 측정).
//   ③ 기록만(판정 아님 — pw-perf 의 시나리오로 두지 않는다): `cli-headless` · `cli-app` — `pitwall tool project_list` 벽시계(준비 1 +
//      20회), `mcp-idle` — MCP 세션 하나의 유휴 메모리 증가분(세션 · 대조 회를 번갈아, 붙은 뒤 40초 이후 표본).
//   ④ 끝나면(실패해도) 끝난 단계마다 `result.json` · `summary.md` 를 `docs/superpowers/reports/<날짜>-m5-perf-<종류>[-short]-<시각>.json|
//      .summary.md` 로 복사하고, 조건(커밋 · OS · 단계별 load average · CPU 를 많이 쓰는 프로세스 · 입력 유휴 · 화면)을
//      `…-m5-perf-conditions[-short]-<시각>.json` 에 남긴다. 측정 도중에는 `target/perf/` 에만 쓴다.
//
// 단계를 시작하기 직전마다 화면 잠금 · 화면 구성(기준선은 내장 화면 하나) · 1분 load average(`--max-load`, 기본은 코어 수)를 보고
// 맞을 때까지 기다린다(키 입력은 보내지 않는다). 종료 코드: 0 — 모든 단계가 0. 3 — 결과는 복사했지만 어느 단계든 유효한 회가
// 적었다. 그 밖 — 실패한 단계의 코드 그대로이고 뒤 단계는 돌지 않는다. macOS 전용이다.

import { spawnSync } from "node:child_process";
import { constants, copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { parseLoad } from "./perf-m3.mjs";
import { baselineDisplays, displays, lockedIn } from "./perf-m4.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));

const BUNDLE = "target/release/bundle/macos/Pitwall.app";
export const APP = "target/release/perf-m5/Pitwall.app";
export const DRIVER = "target/release/pw-perf-m5";
/** `pitwall tool project_list` 의 측정 회 수(16-m5-plan.md M5-22 — 20회 중앙값). */
export const CLI_CALLS = 20;
/** MCP 세션 · 대조 짝의 수. */
export const MCP_PAIRS = 5;
/** 붙은 뒤 이 초 이후의 표본으로 견준다 — 10초 표본은 WebKit 이 가라앉기 전이다(M5-22 첫 측정 §3 ②). */
export const MCP_FROM_SECS = 40;
export const MCP_HOLD_SECS = 60;

/** 측정 단계. `tool` 이 `pw-perf` 면 판정 하네스, `driver` 면 기록 항목 드라이버(`pw-perf-m5`). */
export function measurements({ short, only }) {
  const app = ["--app", APP];
  const all = [
    {
      kind: "empty",
      tool: "pw-perf",
      args: [
        ...app,
        "--label",
        "default bundle, empty project",
        ...(short ? ["--runs", "1", "--warmup", "0"] : []),
      ],
    },
    {
      kind: "cli-headless",
      tool: "driver",
      args: [...app, "cli-headless", "--calls", String(short ? 3 : CLI_CALLS)],
    },
    {
      kind: "cli-app",
      tool: "driver",
      args: [...app, "cli-app", "--calls", String(short ? 3 : CLI_CALLS)],
    },
    {
      kind: "mcp-idle",
      tool: "driver",
      args: [
        ...app,
        "mcp-idle",
        "--pairs",
        String(short ? 1 : MCP_PAIRS),
        "--warmup",
        short ? "0" : "1",
        "--hold-secs",
        String(MCP_HOLD_SECS),
        "--every-secs",
        "10",
        "--from-secs",
        String(MCP_FROM_SECS),
      ],
    },
  ];
  if (!only) return all;
  for (const kind of only)
    if (!all.some((m) => m.kind === kind))
      throw new Error(`--only: unknown step ${kind} (${all.map((m) => m.kind).join(", ")})`);
  return all.filter((m) => only.includes(m.kind));
}

/** 단계 하나를 띄우는 명령 · 인자. */
export function stepSpawn(step, out) {
  if (step.tool === "pw-perf")
    return {
      command: "cargo",
      args: ["run", "-q", "--release", "-p", "pw-perf", "--", ...step.args, "--out", out],
    };
  return { command: join(root, DRIVER), args: [...step.args, "--out", out] };
}

/** 끝난 단계마다 보고서 폴더로 옮길 [원본, 대상] 쌍(원본은 모두 `target/perf/`). */
export function reportCopies({ date, stamp, suffix, kinds }) {
  const reports = "docs/superpowers/reports";
  const pairs = [
    [
      `target/perf/m5-conditions${suffix}-${date}-${stamp}.json`,
      `${reports}/${date}-m5-perf-conditions${suffix}-${stamp}.json`,
    ],
  ];
  for (const kind of kinds)
    for (const [from, ext] of [
      ["result.json", "json"],
      ["summary.md", "summary.md"],
    ])
      pairs.push([
        `target/perf/m5-${kind}${suffix}-${date}-${stamp}/${from}`,
        `${reports}/${date}-m5-perf-${kind}${suffix}-${stamp}.${ext}`,
      ]);
  return pairs;
}

/**
 * `ps -Ao pcpu=,pid=,comm=` 의 출력에서 CPU 를 `min` % 넘게 쓰는 프로세스(많은 차례로). 측정 동안 같이 돈 부하
 * (다른 빌드 · `PerfPowerServices` 등)를 조건 파일에 남긴다.
 */
export function busyProcesses(ps, min = 20) {
  return ps
    .split("\n")
    .map((line) => /^\s*([\d.]+)\s+(\d+)\s+(.+?)\s*$/.exec(line))
    .filter(Boolean)
    .map((m) => ({ cpu: Number(m[1]), pid: Number(m[2]), command: m[3] }))
    .filter((p) => p.cpu > min)
    .sort((a, b) => b.cpu - a.cpu);
}

/** `ioreg -c IOHIDSystem -d 4` 의 `HIDIdleTime`(ns)을 초로. 없으면 null. */
export function hidIdleSecs(ioreg) {
  const m = /"HIDIdleTime"\s*=\s*(\d+)/.exec(ioreg);
  return m ? Math.round(Number(m[1]) / 1e9) : null;
}

function run(cmd, args, opts = {}) {
  return spawnSync(cmd, args, { cwd: root, encoding: "utf8", maxBuffer: 1 << 28, ...opts });
}

function must(cmd, args, opts = {}) {
  const r = run(cmd, args, opts);
  if (r.status !== 0)
    throw new Error(
      `${cmd} ${args.join(" ")} exited ${r.status ?? r.signal}: ${(r.stderr ?? "").trim().slice(-800)}`,
    );
  return r.stdout ?? "";
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const locked = () => lockedIn(must("ioreg", ["-n", "Root", "-d1", "-a"]));
const load = () => parseLoad(must("sysctl", ["-n", "vm.loadavg"]));
const cores = () => Number(must("sysctl", ["-n", "hw.ncpu"]).trim());
const busy = () => busyProcesses(must("ps", ["-Ao", "pcpu=,pid=,comm="]));
const hidIdle = () => hidIdleSecs(run("ioreg", ["-c", "IOHIDSystem", "-d", "4"]).stdout ?? "");

/** 화면이 풀리고 화면 구성이 기준선이고 1분 load average 가 `maxLoad` 이하가 될 때까지 기다린다. */
async function waitForStart(label, maxLoad) {
  const started = Date.now();
  let lockedSecs = 0;
  let displaySecs = 0;
  let loadSecs = 0;
  for (;;) {
    if (locked()) {
      console.error(`perf-m5: ${label}: the screen is locked — waiting (no keys are sent)`);
      await sleep(30_000);
      lockedSecs += 30;
      continue;
    }
    const shown = displays();
    const base = baselineDisplays(shown);
    if (!base.ok) {
      console.error(`perf-m5: ${label}: displays differ from the baseline: ${base.why} — waiting`);
      await sleep(30_000);
      displaySecs += 30;
      continue;
    }
    const now = load();
    if (now[0] > maxLoad) {
      console.error(`perf-m5: ${label}: 1-minute load ${now[0]} > ${maxLoad} — waiting`);
      await sleep(15_000);
      loadSecs += 15;
      continue;
    }
    return {
      waitedSecs: Math.round((Date.now() - started) / 1000),
      lockedSecs,
      displaySecs,
      loadSecs,
      displays: shown,
      loadAtStart: now,
      busyAtStart: busy(),
      hidIdleAtStart: hidIdle(),
    };
  }
}

function buildBundle() {
  const args = ["--filter", "@pitwall/desktop", "tauri", "build", "--bundles", "app"];
  console.log(`perf-m5: pnpm ${args.join(" ")}`);
  const r = spawnSync("pnpm", args, { cwd: root, stdio: "inherit" });
  if (r.status !== 0) throw new Error(`building the bundle failed (exit ${r.status ?? r.signal})`);
  const dest = join(root, APP);
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dirname(dest), { recursive: true });
  must("ditto", [join(root, BUNDLE), dest]);
  console.log("perf-m5: building pw-perf and pw-perf-m5 (release)");
  must("cargo", ["build", "--release", "-p", "pw-perf", "--bins"]);
}

async function main(argv) {
  const short = argv.includes("--short");
  const dry = argv.includes("--dry-run");
  const at = argv.indexOf("--only");
  const only = at >= 0 ? (argv[at + 1] ?? "").split(",").filter(Boolean) : undefined;
  const loadAt = argv.indexOf("--max-load");
  const maxLoadGiven = loadAt >= 0 ? Number(argv[loadAt + 1]) : undefined;
  if (maxLoadGiven !== undefined && !(maxLoadGiven > 0))
    throw new Error("--max-load needs a positive number");
  const steps = measurements({ short, only });
  if (process.platform !== "darwin" && !dry) {
    console.error(`perf-m5: macOS only; this is ${process.platform}`);
    return 1;
  }
  const now = new Date();
  const pad = (n) => String(n).padStart(2, "0");
  const date = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  const stamp = `${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  const suffix = short ? "-short" : "";
  const outOf = (kind) => `target/perf/m5-${kind}${suffix}-${date}-${stamp}`;
  if (dry) {
    for (const step of steps) {
      const s = stepSpawn(step, outOf(step.kind));
      console.log(`perf-m5: ${step.kind}: ${s.command} ${s.args.join(" ")}`);
    }
    return 0;
  }
  const maxLoad = maxLoadGiven ?? cores();
  const conditions = {
    started: now.toISOString(),
    commit: must("git", ["rev-parse", "HEAD"]).trim(),
    dirty: must("git", ["status", "--porcelain"]).trim() !== "",
    cores: cores(),
    maxLoad,
    os: `${must("sw_vers", ["-productVersion"]).trim()} (${must("sw_vers", ["-buildVersion"]).trim()})`,
    displays: displays(),
    steps: [],
  };
  const conditionsFile = join(root, reportCopies({ date, stamp, suffix, kinds: [] })[0][0]);
  const save = () => {
    mkdirSync(dirname(conditionsFile), { recursive: true });
    writeFileSync(conditionsFile, `${JSON.stringify(conditions, null, 2)}\n`);
  };
  const done = [];
  let worst = 0;
  try {
    buildBundle();
    conditions.app = APP;
    save();
    for (const step of steps) {
      const out = outOf(step.kind);
      const waited = await waitForStart(step.kind, maxLoad);
      const spawn = stepSpawn(step, out);
      console.log(`perf-m5: ${step.kind}: ${spawn.command} ${spawn.args.join(" ")}`);
      const record = {
        kind: step.kind,
        args: spawn.args,
        ...waited,
        startedAt: new Date().toISOString(),
      };
      conditions.steps.push(record);
      save();
      const r = spawnSync(spawn.command, spawn.args, { cwd: root, stdio: "inherit" });
      record.endedAt = new Date().toISOString();
      record.loadAtEnd = load();
      record.busyAtEnd = busy();
      record.hidIdleAtEnd = hidIdle();
      const after = displays();
      record.displaysKept = JSON.stringify(after) === JSON.stringify(waited.displays);
      if (!record.displaysKept) {
        record.displaysAtEnd = after;
        console.error(
          `perf-m5: ${step.kind}: the displays changed during the step — do not judge it`,
        );
      }
      record.exit = r.status;
      save();
      if (r.status !== 0 && r.status !== 3)
        throw Object.assign(
          new Error(
            `${step.kind} failed (exit ${r.status ?? `signal ${r.signal}`}); results already written stay in ${out}`,
          ),
          { exit: r.status ?? 1 },
        );
      for (const from of ["result.json", "summary.md"])
        if (!existsSync(join(root, out, from)))
          throw new Error(`${step.kind}: ${join(out, from)} was not written`);
      done.push(step.kind);
      if (r.status === 3) {
        worst = 3;
        console.error(`perf-m5: ${step.kind}: too few valid runs (exit 3) — see its summary`);
      }
    }
  } catch (e) {
    console.error(`perf-m5: ${e.message}`);
    conditions.error = e.message;
    worst = e.exit ?? 1;
  } finally {
    conditions.ended = new Date().toISOString();
    save();
    const reports = join(root, "docs/superpowers/reports");
    mkdirSync(reports, { recursive: true });
    for (const [src, dst] of reportCopies({ date, stamp, suffix, kinds: done })) {
      copyFileSync(join(root, src), join(root, dst), constants.COPYFILE_EXCL);
      console.log(`perf-m5: ${src} → ${dst}`);
    }
  }
  return worst;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then(
    (code) => (process.exitCode = code),
    (e) => {
      console.error(`perf-m5: ${e.message}`);
      process.exitCode = 1;
    },
  );
}
