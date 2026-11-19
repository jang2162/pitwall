// M6-22 측정(17-m6-plan.md — 「가벼움」, D56)을 한 번에 돌리고 결과를 보고서 폴더로 옮긴다(앱을 앞으로 띄우므로 사람이 기기를
// 비웠을 때만 — 키 · 마우스 입력은 보내지 않는다).
//
//   node scripts/perf-m6.mjs [--short] [--only <종류>,…] [--dry-run] [--no-build] [--max-load <1분 load>]     (`mise run perf-m6 -- --short`)
//   `--no-build` 는 이미 옮겨 둔 번들 · 드라이버로 잰다(같은 커밋에서 지은 것일 때만 — 조건 파일의 `bundleBuilt` 가 false).
//
// 차례(측정 방법은 CONTRIBUTING.md 「성능 기준선」 의 「M6 측정」):
//   ① 데스크톱 release 번들(기본 feature — 출시와 같다)을 만들어 `target/release/perf-m6/Pitwall.app` 로 옮기고, `pw-perf` ·
//      `pw-perf-m6` · `pw-perf-flood`(crates/pw-perf/src/bin — 번들에 들지 않는다)를 release 로 짓는다. 디버그 어댑터(codelldb ·
//      js-debug-adapter · debugpy)가 `target/dap-adapters/`(PW_DAP_ADAPTERS_DIR)에 없으면 Pitwall 의 설치기(`tools_install` — 앱과 같은
//      고정 판 · 해시)로 받는다(scripts/dap-live.mjs 와 같은 자리).
//   ② `empty` — pw-perf 의 빈 프로젝트 콜드 스타트 · 유휴 메모리(M5 끝 측정 · 06 「성능 목표」 와 같은 측정).
//   ③ `idle-procs` — 같은 빈 프로젝트에서 앱 프로세스의 깨어남 · 스레드(포트 폴링 `run-ports`) · 자손 프로세스.
//   ④ `run-flood` — 초당 10 MB 를 쓰는 실행 하나의 콘솔 프레임(웹뷰) · Host 메모리.
//   ⑤ `debug-end` — 어댑터 셋 × 끝맺음 둘(대상이 끝남 · 멈춤)에서 세션이 끝난 뒤 어댑터 프로세스가 사라지는 시간.
//   ⑥ 끝나면(실패해도) 끝난 단계마다 `result.json` · `summary.md` 를 `docs/superpowers/reports/<날짜>-m6-perf-<종류>[-short]-<시각>.json|
//      .summary.md` 로 복사하고 조건(커밋 · OS · 단계별 load average · CPU 를 많이 쓰는 프로세스 · 입력 유휴 · 화면)을
//      `…-m6-perf-conditions[-short]-<시각>.json` 에 남긴다. 측정 도중에는 `target/perf/` 에만 쓴다.
//
// 단계 앞의 기다림(화면 잠금 · 화면 구성 · 1분 load average)과 종료 코드는 scripts/perf-m5.mjs 와 같다. macOS 전용이다.

import { spawnSync } from "node:child_process";
import { constants, copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { parseLoad } from "./perf-m3.mjs";
import { baselineDisplays, displays, lockedIn } from "./perf-m4.mjs";
import { busyProcesses, hidIdleSecs } from "./perf-m5.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));

const BUNDLE = "target/release/bundle/macos/Pitwall.app";
export const APP = "target/release/perf-m6/Pitwall.app";
export const DRIVER = "target/release/pw-perf-m6";
/** 디버그 어댑터를 받는 자리(scripts/dap-live.mjs 와 같다). */
export const DAP_DIR = process.env.PW_DAP_ADAPTERS_DIR || "target/dap-adapters";
/** 받을 Mason 패키지(scripts/dap-live.toml 의 [install.*] 와 같다). java-debug 는 jdtls 안에서 돌아 따로 뜨는 어댑터가 없다. */
export const ADAPTER_PACKAGES = {
  codelldb: "codelldb",
  "js-debug": "js-debug-adapter",
  debugpy: "debugpy",
};

/** 측정 단계. `tool` 이 `pw-perf` 면 판정 하네스, `driver` 면 `pw-perf-m6`. */
export function measurements({ short, only }) {
  const app = ["--app", APP];
  const runs = (n) => ["--runs", String(short ? 1 : n)];
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
    { kind: "idle-procs", tool: "driver", args: [...app, "idle-procs", ...runs(3)] },
    { kind: "run-flood", tool: "driver", args: [...app, "run-flood", ...runs(3)] },
    {
      kind: "debug-end",
      tool: "driver",
      args: [
        ...app,
        "debug-end",
        "--dap-dir",
        DAP_DIR,
        "--adapters",
        Object.keys(ADAPTER_PACKAGES).join(","),
        ...runs(3),
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
      `target/perf/m6-conditions${suffix}-${date}-${stamp}.json`,
      `${reports}/${date}-m6-perf-conditions${suffix}-${stamp}.json`,
    ],
  ];
  for (const kind of kinds)
    for (const [from, ext] of [
      ["result.json", "json"],
      ["summary.md", "summary.md"],
    ])
      pairs.push([
        `target/perf/m6-${kind}${suffix}-${date}-${stamp}/${from}`,
        `${reports}/${date}-m6-perf-${kind}${suffix}-${stamp}.${ext}`,
      ]);
  return pairs;
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
      console.error(`perf-m6: ${label}: the screen is locked — waiting (no keys are sent)`);
      await sleep(30_000);
      lockedSecs += 30;
      continue;
    }
    const shown = displays();
    const base = baselineDisplays(shown);
    if (!base.ok) {
      console.error(`perf-m6: ${label}: displays differ from the baseline: ${base.why} — waiting`);
      await sleep(30_000);
      displaySecs += 30;
      continue;
    }
    const now = load();
    if (now[0] > maxLoad) {
      console.error(`perf-m6: ${label}: 1-minute load ${now[0]} > ${maxLoad} — waiting`);
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
  console.log(`perf-m6: pnpm ${args.join(" ")}`);
  const r = spawnSync("pnpm", args, { cwd: root, stdio: "inherit" });
  if (r.status !== 0) throw new Error(`building the bundle failed (exit ${r.status ?? r.signal})`);
  const dest = join(root, APP);
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dirname(dest), { recursive: true });
  must("ditto", [join(root, BUNDLE), dest]);
  console.log("perf-m6: building pw-perf, pw-perf-m6 and pw-perf-flood (release)");
  must("cargo", ["build", "--release", "-p", "pw-perf", "--bins"]);
}

/** 어댑터가 받아져 있지 않으면 앱의 설치기로 받는다. */
function installAdapters() {
  for (const pkg of Object.values(ADAPTER_PACKAGES)) {
    if (existsSync(join(root, DAP_DIR, "data/tools", pkg))) continue;
    console.log(`perf-m6: tools_install ${pkg} (${DAP_DIR})`);
    must("cargo", [
      "run",
      "-q",
      "-p",
      "pw-tools",
      "--example",
      "tools_install",
      "--",
      pkg,
      join(DAP_DIR, "data"),
      join(DAP_DIR, "state"),
    ]);
  }
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
  const skipBuild = argv.includes("--no-build");
  const steps = measurements({ short, only });
  if (process.platform !== "darwin" && !dry) {
    console.error(`perf-m6: macOS only; this is ${process.platform}`);
    return 1;
  }
  const now = new Date();
  const pad = (n) => String(n).padStart(2, "0");
  const date = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  const stamp = `${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  const suffix = short ? "-short" : "";
  const outOf = (kind) => `target/perf/m6-${kind}${suffix}-${date}-${stamp}`;
  if (dry) {
    for (const step of steps) {
      const s = stepSpawn(step, outOf(step.kind));
      console.log(`perf-m6: ${step.kind}: ${s.command} ${s.args.join(" ")}`);
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
    if (!skipBuild) buildBundle();
    if (!existsSync(join(root, APP))) throw new Error(`${APP} is missing — build it first`);
    if (steps.some((s) => s.kind === "debug-end")) installAdapters();
    conditions.app = APP;
    conditions.bundleBuilt = !skipBuild;
    save();
    for (const step of steps) {
      const out = outOf(step.kind);
      const waited = await waitForStart(step.kind, maxLoad);
      const spawn = stepSpawn(step, out);
      console.log(`perf-m6: ${step.kind}: ${spawn.command} ${spawn.args.join(" ")}`);
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
          `perf-m6: ${step.kind}: the displays changed during the step — do not judge it`,
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
        console.error(`perf-m6: ${step.kind}: too few valid runs (exit 3) — see its summary`);
      }
    }
  } catch (e) {
    console.error(`perf-m6: ${e.message}`);
    conditions.error = e.message;
    worst = e.exit ?? 1;
  } finally {
    conditions.ended = new Date().toISOString();
    save();
    const reports = join(root, "docs/superpowers/reports");
    mkdirSync(reports, { recursive: true });
    for (const [src, dst] of reportCopies({ date, stamp, suffix, kinds: done })) {
      copyFileSync(join(root, src), join(root, dst), constants.COPYFILE_EXCL);
      console.log(`perf-m6: ${src} → ${dst}`);
    }
  }
  return worst;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then(
    (code) => (process.exitCode = code),
    (e) => {
      console.error(`perf-m6: ${e.message}`);
      process.exitCode = 1;
    },
  );
}
