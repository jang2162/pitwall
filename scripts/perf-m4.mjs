// M4-19 실기 측정을 한 번에 돌리고 결과를 보고서 폴더로 옮긴다(앱을 앞으로 띄우므로 사람이 기기를 비웠을 때만).
//
//   node scripts/perf-m4.mjs [--short] [--only <종류>,…] [--dry-run] [--ra <rust-analyzer>]     (`mise run perf-m4 -- --short`)
//
// 차례(측정 방법은 CONTRIBUTING.md 「성능 기준선」 의 「LSP 시나리오」):
//   ① 데스크톱 release 번들(기본 feature — 출시와 같다)을 만들어 `target/release/perf-m4/Pitwall.app` 로 옮긴다. 가짜 언어 서버
//      `pw-perf-lsp`(crates/pw-perf/src/bin — 번들에 들지 않는다)를 release 로 짓는다.
//   ② 준비물: 합성 저장소(파일 2만 · 심볼 50만 — `pw-index` 의 `perf_repo` 예제, M4-15 의 release 시험과 같은 저장소)를
//      `target/perf-m4/symbols-repo` 에, 이 저장소의 지금 커밋 사본을 `target/perf-m4/repo` 에(`git clone` — 측정 앱이 신뢰로 등록해
//      rust-analyzer 를 띄운다. 워크트리 자체를 쓰지 않는다: 앱 · 서버가 쓰는 파일이 `commit.dirty` 를 바꾼다), rust-analyzer 를
//      Pitwall 의 설치기(`pw-tools` 의 `tools_install` — lsp-live 와 같은 고정 판 · 해시)로 `target/perf-m4/servers` 에(`--ra` 면 그것).
//   ③ 앱 없이: `rust-index` — M4-15 의 release 시험(`cargo test --release -p pw-index --test perf`)을 5회(Host 의 첫 색인 · 힙 · 질의).
//   ④ pw-perf 를 차례로: `empty`(빈 프로젝트 콜드 스타트 · 유휴 메모리 — M3 기준선과 같은 측정), `lsp`(가짜 서버: 자동완성 · 진단 ·
//      정의로 이동 · 문제 툴 윈도우), `typing`(타이핑 지연 — 켬 · 끔을 회 단위로 번갈아), `symbols` · `classes`(합성 저장소: 첫 색인 · ⌘⌥O · ⌘O),
//      `ra-idle`(이 저장소 + rust-analyzer: 콜드 스타트 · 유휴 메모리, 서버는 따로).
//   ⑤ 끝나면(실패해도) 끝난 단계마다 `result.json` · `summary.md` 를 `docs/superpowers/reports/<날짜>-m4-perf-<종류>[-short]-<시각>.json|
//      .summary.md` 로 복사하고, 조건(커밋 · 단계별 load average · 기다린 시간 · 화면)을 `…-m4-perf-conditions[-short]-<시각>.json` 에
//      남긴다. 측정 도중에는 `target/perf/` 에만 쓴다 — 추적되는 경로에 쓰면 다음 단계의 `commit.dirty` 가 true 가 된다.
//
// 단계를 시작하기 직전마다 화면 잠금을 보고 **잠겨 있으면 풀릴 때까지 기다린다**(키 입력은 보내지 않는다 — 앱 안 시나리오의 키는
// 웹뷰 안에서 만든 이벤트다). 화면 구성이 기준선(내장 화면 하나, 외부 모니터 없음)과 다르면 같아질 때까지, 1분 load average 가
// 코어 수를 넘으면 내려갈 때까지 기다린다. 기다린 시간과 시작 때의 화면은 조건 파일에 적는다.
// 종료 코드: 0 — 모든 단계가 0. 3 — 결과는 복사했지만 어느 단계든 유효한 회가 적었다. 그 밖 — 실패한 단계의 코드 그대로이고 뒤
// 단계는 돌지 않는다. macOS 전용이다.

import { spawnSync } from "node:child_process";
import {
  constants,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { installArgs, installedProgram } from "./lsp-live.mjs";
import { parseLoad } from "./perf-m3.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));

const BUNDLE = "target/release/bundle/macos/Pitwall.app";
export const APP = "target/release/perf-m4/Pitwall.app";
export const FAKE = "target/release/pw-perf-lsp";
export const SYMBOLS_REPO = "target/perf-m4/symbols-repo";
export const REPO_COPY = "target/perf-m4/repo";
export const SERVERS = "target/perf-m4/servers";
/** M4-15 의 release 시험(앱 없이 — Host 몫). */
export const RUST_RUNS = 5;
/** 타이핑 켬 · 끔 짝의 수(회마다 켬 한 번 · 끔 한 번). */
export const TYPING_ROUNDS = 5;
/** `lsp-idle` 의 시나리오 시간 — rust-analyzer 가 이 저장소를 처음 읽는 데(빌드 스크립트 · 프록 매크로) 몇 분 걸린다. */
export const IDLE_TIMEOUT_SECS = 1800;

/** 측정 단계(pw-perf 인자). `rust-index` 는 pw-perf 가 아니라 cargo 시험이다(`args` 없음). */
export function measurements({ short, only, ra }) {
  const extra = short ? ["--runs", "1", "--warmup", "0"] : [];
  const app = ["--app", APP];
  const all = [
    { kind: "rust-index" },
    { kind: "empty", args: [...app, "--label", "default bundle, empty project", ...extra] },
    {
      kind: "lsp",
      args: [...app, "--scenario", "lsp", "--lsp-fake", FAKE, "--label", "fake server", ...extra],
    },
    {
      // 켬 · 끔을 회 단위로 번갈아 돈다(on, off, on, off …) — 차례로 한 번씩 돌면 앞쪽이 부하가 다른 때에 돈다(M4-19 리뷰 M1).
      // pw-perf 를 회마다 `--runs 1` 로 부르고(첫 짝만 준비 1회), 회마다 시작 load 를 적는다.
      kind: "typing",
      rounds: short ? 1 : TYPING_ROUNDS,
      warmupFirst: short ? 0 : 1,
      alternate: [
        {
          kind: "typing-on",
          args: [
            ...app,
            "--scenario",
            "lsp-typing",
            "--lsp-fake",
            FAKE,
            "--label",
            "typing, LSP on (fake server)",
          ],
        },
        {
          kind: "typing-off",
          args: [
            ...app,
            "--scenario",
            "lsp-typing-off",
            "--label",
            "typing, LSP off ([lsp.rust-analyzer] enabled = false)",
          ],
        },
      ],
    },
    {
      kind: "symbols",
      args: [
        ...app,
        "--scenario",
        "symbols",
        "--project",
        SYMBOLS_REPO,
        "--label",
        "synthetic 20k files / 500k symbols, ⌘⌥O first",
        ...extra,
      ],
    },
    {
      kind: "classes",
      args: [
        ...app,
        "--scenario",
        "classes",
        "--project",
        SYMBOLS_REPO,
        "--label",
        "synthetic 20k files / 500k symbols, ⌘O first",
        ...extra,
      ],
    },
    {
      kind: "ra-idle",
      args: [
        ...app,
        "--scenario",
        "lsp-idle",
        "--project",
        REPO_COPY,
        "--lsp-server",
        ra ?? "<rust-analyzer>",
        "--scenario-timeout-secs",
        String(IDLE_TIMEOUT_SECS),
        "--label",
        "this repository + rust-analyzer",
        ...extra,
      ],
      // 사본의 `mise.toml` 을 이 측정에서만 신뢰한다 — 아니면 Pitwall 의 런타임 환경이 mise 의 NotTrusted 로 빠져 서버가 툴체인
      // 없이 뜬다(2026-11-10 실측). 사람의 trust 목록(`mise trust`)은 건드리지 않는다. 앱 → 로그인 셸 → `mise env` 로 이어진다.
      env: { MISE_TRUSTED_CONFIG_PATHS: join(root, REPO_COPY) },
    },
  ];
  if (!only) return all;
  for (const kind of only)
    if (!all.some((m) => m.kind === kind))
      throw new Error(`--only: unknown step ${kind} (${all.map((m) => m.kind).join(", ")})`);
  return all.filter((m) => only.includes(m.kind));
}

/** 끝난 단계마다 보고서 폴더로 옮길 [원본, 대상] 쌍(원본은 모두 `target/perf/` — 측정 도중 추적 경로에 쓰지 않는다). */
export function reportCopies({ date, stamp, suffix, kinds }) {
  const reports = "docs/superpowers/reports";
  const pairs = [
    [
      `target/perf/m4-conditions${suffix}-${date}-${stamp}.json`,
      `${reports}/${date}-m4-perf-conditions${suffix}-${stamp}.json`,
    ],
  ];
  for (const kind of kinds)
    for (const [from, ext] of [
      ["result.json", "json"],
      ["summary.md", "summary.md"],
    ])
      pairs.push([
        `target/perf/m4-${kind}${suffix}-${date}-${stamp}/${from}`,
        `${reports}/${date}-m4-perf-${kind}${suffix}-${stamp}.${ext}`,
      ]);
  return pairs;
}

/** Rust `Duration` 의 Debug 글(`752.1ms` · `1.6s` · `830µs`)을 ms 로. */
export function durationMs(text) {
  const m = /^([\d.]+)(ns|µs|us|ms|s)$/.exec(text.trim());
  if (!m) throw new Error(`cannot read the duration ${JSON.stringify(text)}`);
  const unit = { ns: 1e-6, µs: 1e-3, us: 1e-3, ms: 1, s: 1000 }[m[2]];
  return Number(m[1]) * unit;
}

/** M4-15 release 시험의 출력에서 값을 읽는다. */
export function parseIndexPerf(stdout) {
  const need = (re, what) => {
    const m = re.exec(stdout);
    if (!m) throw new Error(`the pw-index perf test printed no ${what}`);
    return m;
  };
  const first = need(/^first index: (\S+)/m, "first index");
  const memory = need(/^memory: heap \+([\d.]+) MiB.*?rss \+([\d.]+) MiB/m, "memory");
  const query = need(/^query per keystroke: median (\S+), max (\S+)/m, "query time");
  const symbols = need(/^symbols: (\d+)/m, "symbol count");
  return {
    firstIndexMs: durationMs(first[1]),
    heapMiB: Number(memory[1]),
    rssMiB: Number(memory[2]),
    queryMedianMs: durationMs(query[1].replace(/,$/, "")),
    queryMaxMs: durationMs(query[2].split(" ")[0]),
    symbols: Number(symbols[1]),
  };
}

/** `ioreg -n Root -d1 -a` 의 화면 잠금. */
export function lockedIn(ioreg) {
  return /CGSSessionScreenIsLocked<\/key>\s*<true\/>/.test(ioreg);
}

/**
 * 화면 구성이 기준선(M1 · M2 · M3 와 같은 내장 화면 하나, 외부 모니터 없음)인가. 외부 1x 모니터에서는 WebKit(GPU · WebContent)
 * 메모리가 작게 나와 유휴 메모리가 낮게 읽힌다(M4-19 리뷰 H2 — 빈 프로젝트 208.7 MB 대 내장 화면 226.7 MB). `displays` 가 없으면(읽지 못했으면) 기준선이 아니다.
 */
export function baselineDisplays(displays) {
  if (!Array.isArray(displays)) return { ok: false, why: "the displays could not be read" };
  const shown = displays.map((d) => `${d.name} ${d.resolution}${d.internal ? " (built-in)" : ""}`);
  if (displays.length === 1 && displays[0].internal) return { ok: true, why: shown[0] };
  return {
    ok: false,
    why: `${shown.join(", ") || "no display"} — the baseline is one built-in display, no external monitor`,
  };
}

/** pw-perf 단계 하나를 띄우는 명령 · 인자 · 옵션 — 단계의 `env` 는 그 실행에만 더한다(`base` 는 바꾸지 않는다). */
export function stepSpawn(step, args, out, base = process.env) {
  return {
    command: "cargo",
    args: ["run", "-q", "--release", "-p", "pw-perf", "--", ...args, "--out", out],
    options: { cwd: root, stdio: "inherit", env: { ...base, ...step.env } },
  };
}

/** pw-perf 의 `result.json` 에서 측정 회(준비 · 제외 아닌 첫 회)의 타이핑 값. 없으면 null. */
export function typingRun(result) {
  const run = (result?.runs ?? []).find((r) => !r.warmup && !r.excluded && r.measure?.typing);
  if (!run) return null;
  const { painted, frame, breakdown } = run.measure.typing;
  return {
    p95: painted.p95,
    p50: painted.p50,
    max: painted.max,
    within16_7: painted.within16_7,
    frameP95: frame.p95,
    ...(breakdown && { breakdown: typingBreakdownRow(breakdown) }),
  };
}

/** 키별 몫(`breakdown` — perf-scenario-lsp.ts `typingBreakdown`)의 p95 몇 개. 묶음에 키가 없으면 null. */
export function typingBreakdownRow(b) {
  const p95 = (g, field) => (g.keys > 0 ? g[field].p95 : null);
  return {
    dispatchP95: p95(b.all, "dispatch"),
    stateP95: b.all.keys > 0 && b.all.state ? b.all.state.p95 : null,
    popupLayoutP95: b.popup.keys > 0 && b.popup.layout ? b.popup.layout.p95 : null,
    changeP95: p95(b.all, "change"),
    waitP95: p95(b.all, "wait"),
    renderP95: p95(b.all, "render"),
    popupKeys: b.popup.keys,
    popupP95: p95(b.popup, "painted"),
    popupDispatchP95: p95(b.popup, "dispatch"),
    popupRenderP95: p95(b.popup, "render"),
    noPopupP95: p95(b.noPopup, "painted"),
  };
}

const median = (xs) => {
  if (xs.length === 0) return null;
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};

/**
 * 번갈아 돈 타이핑 회(`{ round, kind, lsp, typing, loadAtStart }`)의 판정: 06 은 켬 − 끔 ≤ 2 ms 를 회별 p95 의 중앙값끼리 견준다.
 * 같은 짝(같은 회 번호)의 켬 − 끔도 참고로 싣는다. 값이 없는 회(제외 · 실패)는 빼고, 남은 회 수를 적는다.
 */
export function typingSummary(rounds, targetMs = 2) {
  const p95 = (lsp) => rounds.filter((r) => r.lsp === lsp && r.typing).map((r) => r.typing.p95);
  const on = p95(true);
  const off = p95(false);
  const mOn = median(on);
  const mOff = median(off);
  const delta = mOn === null || mOff === null ? null : Math.round((mOn - mOff) * 100) / 100;
  const pairs = [];
  for (const r of rounds.filter((x) => x.lsp && x.typing)) {
    const o = rounds.find((x) => !x.lsp && x.round === r.round && x.typing);
    if (o) pairs.push(Math.round((r.typing.p95 - o.typing.p95) * 100) / 100);
  }
  return {
    on: {
      n: on.length,
      median: mOn,
      min: on.length ? Math.min(...on) : null,
      max: on.length ? Math.max(...on) : null,
    },
    off: {
      n: off.length,
      median: mOff,
      min: off.length ? Math.min(...off) : null,
      max: off.length ? Math.max(...off) : null,
    },
    deltaMs: delta,
    pairDeltasMs: pairs,
    targetMs,
    met: delta === null ? null : delta <= targetMs,
  };
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

/**
 * 화면이 풀리고 화면 구성이 기준선이고(`screen` 일 때 — 앱이 없는 단계는 둘과 무관하다) 1분 load average 가 코어 수 이하가 될 때까지 기다린다. 기다린 초와
 * 시작할 때의 load.
 */
async function waitForStart(label, screen = true) {
  const limit = cores();
  const started = Date.now();
  let lockedSecs = 0;
  let displaySecs = 0;
  let shown;
  for (;;) {
    if (screen && locked()) {
      console.error(`perf-m4: ${label}: the screen is locked — waiting (no keys are sent)`);
      await sleep(30_000);
      lockedSecs += 30;
      continue;
    }
    if (screen) {
      shown = displays();
      const base = baselineDisplays(shown);
      if (!base.ok) {
        console.error(
          `perf-m4: ${label}: displays differ from the baseline: ${base.why} — waiting`,
        );
        await sleep(30_000);
        displaySecs += 30;
        continue;
      }
    }
    const now = load();
    if (now[0] > limit) {
      console.error(`perf-m4: ${label}: 1-minute load ${now[0]} > ${limit} cores — waiting`);
      await sleep(15_000);
      continue;
    }
    return {
      waitedSecs: Math.round((Date.now() - started) / 1000),
      lockedSecs,
      displaySecs,
      ...(screen ? { displays: shown } : {}),
      loadAtStart: now,
    };
  }
}

function buildBundle() {
  const args = ["--filter", "@pitwall/desktop", "tauri", "build", "--bundles", "app"];
  console.log(`perf-m4: pnpm ${args.join(" ")}`);
  const r = spawnSync("pnpm", args, { cwd: root, stdio: "inherit" });
  if (r.status !== 0) throw new Error(`building the bundle failed (exit ${r.status ?? r.signal})`);
  const dest = join(root, APP);
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dirname(dest), { recursive: true });
  must("ditto", [join(root, BUNDLE), dest]);
}

function prepare(ra) {
  console.log("perf-m4: building pw-perf and the fake language server (release)");
  must("cargo", ["build", "--release", "-p", "pw-perf", "--bins"]);
  console.log(`perf-m4: synthetic repository → ${SYMBOLS_REPO}`);
  must("cargo", [
    "run",
    "-q",
    "--release",
    "-p",
    "pw-index",
    "--example",
    "perf_repo",
    "--",
    SYMBOLS_REPO,
  ]);
  const head = must("git", ["rev-parse", "HEAD"]).trim();
  const copy = join(root, REPO_COPY);
  if (!existsSync(join(copy, ".git"))) {
    console.log(`perf-m4: cloning this repository → ${REPO_COPY}`);
    must("git", ["clone", "--quiet", "--no-checkout", root, copy]);
  }
  must("git", ["-C", copy, "fetch", "--quiet", root, head]);
  must("git", ["-C", copy, "checkout", "--quiet", "--force", "--detach", head]);
  if (ra) return { ra, raVersion: null, head };
  console.log(`perf-m4: installing rust-analyzer → ${SERVERS}`);
  const out = must("cargo", installArgs("rust-analyzer", join(root, SERVERS)));
  const installed = installedProgram(out, "rust-analyzer");
  return { ra: installed.program, raVersion: installed.version, head };
}

function displays() {
  const r = run("system_profiler", ["SPDisplaysDataType", "-json"]);
  if (r.status !== 0) return null;
  try {
    return (JSON.parse(r.stdout).SPDisplaysDataType ?? []).flatMap((g) =>
      (g.spdisplays_ndrvs ?? []).map((d) => ({
        name: d._name,
        resolution: d._spdisplays_resolution ?? d.spdisplays_resolution ?? null,
        pixels: d._spdisplays_pixels ?? null,
        main: d.spdisplays_main === "spdisplays_yes",
        internal: d.spdisplays_connection_type === "spdisplays_internal",
      })),
    );
  } catch {
    return null;
  }
}

/** M4-15 의 release 시험을 `RUST_RUNS` 번(`--short` 면 1번). 회마다 출력을 남기고 값을 모은다. */
function rustIndex(out, short) {
  mkdirSync(join(root, out), { recursive: true });
  const runs = [];
  for (let i = 0; i < (short ? 1 : RUST_RUNS); i++) {
    const args = [
      "test",
      "--release",
      "-p",
      "pw-index",
      "--test",
      "perf",
      "--",
      "--ignored",
      "--nocapture",
      "--test-threads",
      "1",
    ];
    console.log(`perf-m4: cargo ${args.join(" ")} (${i + 1})`);
    const r = run("cargo", args);
    writeFileSync(join(root, out, `run-${i}.txt`), `${r.stdout}\n${r.stderr}`);
    if (r.status !== 0) throw new Error(`the pw-index perf test failed (exit ${r.status})`);
    if (!/1 passed/.test(r.stdout)) throw new Error("the pw-index perf test did not run");
    runs.push({ ...parseIndexPerf(r.stdout), load: load() });
  }
  const result = {
    command:
      "cargo test --release -p pw-index --test perf -- --ignored --nocapture --test-threads 1",
    runs,
  };
  writeFileSync(join(root, out, "result.json"), `${JSON.stringify(result, null, 2)}\n`);
  const lines = runs.map(
    (r, i) =>
      `| ${i} | ${r.firstIndexMs.toFixed(0)} | ${r.heapMiB} | ${r.rssMiB} | ${r.queryMedianMs.toFixed(1)} | ${r.queryMaxMs.toFixed(1)} | ${r.symbols} | ${r.load[0]} |`,
  );
  writeFileSync(
    join(root, out, "summary.md"),
    [
      "## pw-index release perf test (M4-15, Host only — 7 threads, the 8-core condition)",
      "",
      `\`${result.command}\``,
      "",
      "| Run | First index ms | Heap MiB | RSS MiB | Query median ms | Query max ms | Symbols | 1-min load after |",
      "|---|---|---|---|---|---|---|---|",
      ...lines,
      "",
    ].join("\n"),
  );
}

/** 번갈아 도는 단계의 회 `r` 인자 — 회마다 측정 1회, 첫 짝만 준비 1회. */
export function alternateArgs(step, side, r) {
  return [...side.args, "--runs", "1", "--warmup", r === 0 ? String(step.warmupFirst) : "0"];
}

/** 번갈아 도는 단계의 차례: 회마다 `alternate` 를 한 번씩(on, off, on, off …). */
export function alternateRuns(step) {
  const runs = [];
  for (let round = 0; round < step.rounds; round++)
    for (const side of step.alternate)
      runs.push({ round, side, args: alternateArgs(step, side, round) });
  return runs;
}

export function alternateOut(step, side, r, suffix, date, stamp) {
  return `target/perf/m4-${step.kind}${suffix}-${date}-${stamp}/${side.kind}-r${r}`;
}

/**
 * pw-perf 단계 하나: 시작 조건(잠금 · 부하 · 화면)을 기다려 돌리고 조건 파일에 적는다. 끝날 때 화면 구성이 바뀌었으면
 * `displaysKept: false` 로 적고 경고한다(그 단계는 판정하지 않는다). 0 · 3 밖의 종료는 던진다.
 */
async function pwPerf(step, args, out, conditions, save, extra = {}) {
  const waited = await waitForStart(step.kind);
  const spawn = stepSpawn(step, args, out);
  console.log(`perf-m4: cargo ${spawn.args.join(" ")}`);
  const record = {
    kind: step.kind,
    ...extra,
    args,
    env: step.env ?? null,
    ...waited,
    startedAt: new Date().toISOString(),
  };
  conditions.steps.push(record);
  save();
  const r = spawnSync(spawn.command, spawn.args, spawn.options);
  record.endedAt = new Date().toISOString();
  record.loadAtEnd = load();
  const after = displays();
  record.displaysKept = JSON.stringify(after) === JSON.stringify(waited.displays);
  if (!record.displaysKept) {
    record.displaysAtEnd = after;
    console.error(
      `perf-m4: ${step.kind}: the displays changed during the step (${baselineDisplays(after).why}) — do not judge it`,
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
  return record;
}

/** 번갈아 돈 타이핑 회를 한 결과(`result.json` · `summary.md`)로 묶는다. */
function writeTyping(out, rounds) {
  const summary = typingSummary(rounds);
  const result = {
    schema: "pitwall-perf-m4-typing/1",
    definition:
      "LSP on and off alternate run by run (on, off, on, off …), each a pw-perf `lsp-typing` / `lsp-typing-off` run with `--runs 1` (the first pair adds one warm-up); per run: insert → after the next frame's rendering, p95 over 300 keys 50 ms apart on average (each pause spread over one frame — not comparable with the fixed-50 ms runs before 2026-11-12); 06 judges median(on p95) − median(off p95) ≤ 2 ms",
    rounds,
    summary,
  };
  writeFileSync(join(root, out, "result.json"), `${JSON.stringify(result, null, 2)}\n`);
  const f = (x) => (x === null || x === undefined ? "—" : String(x));
  const lines = rounds.map(
    (r) =>
      `| ${r.round} | ${r.lsp ? "on" : "off"} | ${f(r.typing?.p95)} | ${f(r.typing?.p50)} | ${f(r.typing?.frameP95)} | ${f(r.typing?.within16_7)} | ${r.loadAtStart?.join(" / ")} | ${r.exit}${r.displaysKept ? "" : " (displays changed)"} |`,
  );
  writeFileSync(
    join(root, out, "summary.md"),
    [
      "## Typing latency, LSP on − off, alternating runs (M4-19)",
      "",
      `- commit: \`${f(rounds[0]?.commit?.sha)}\` (dirty ${f(rounds[0]?.commit?.dirty)})`,
      `- os: ${rounds[0]?.os ? `${rounds[0].os.name} ${rounds[0].os.version} (${rounds[0].os.build})` : "—"}`,
      `- on: median p95 ${f(summary.on.median)} ms (${f(summary.on.min)}–${f(summary.on.max)}, n=${summary.on.n})`,
      `- off: median p95 ${f(summary.off.median)} ms (${f(summary.off.min)}–${f(summary.off.max)}, n=${summary.off.n})`,
      `- on − off: **${f(summary.deltaMs)} ms** (target ≤ ${summary.targetMs} ms → ${summary.met === null ? "no verdict" : summary.met ? "met" : "not met"}); per pair: ${summary.pairDeltasMs.join(", ")}`,
      "",
      "| Round | LSP | p95 ms | p50 ms | frame p95 ms | ≤16.7 share | 1/5/15-min load at start | exit |",
      "|---|---|---|---|---|---|---|---|",
      ...lines,
      "",
      "Per-key split (reference, p95 ms per run): `view.dispatch` (sync — state · view update, didChange send), of which the state update (autocomplete re-filtering) and sending didChange, forced layout during a popup key's update, dispatch → next frame, frame → after its rendering; keys with the autocomplete popup drawn after the frame vs without.",
      "",
      "| Round | LSP | dispatch | of which state | didChange | wait | render | popup keys | popup p95 | popup dispatch | popup forced layout | popup render | no-popup p95 |",
      "|---|---|---|---|---|---|---|---|---|---|---|---|---|",
      ...rounds.map((r) => {
        const b = r.typing?.breakdown;
        return `| ${r.round} | ${r.lsp ? "on" : "off"} | ${f(b?.dispatchP95)} | ${f(b?.stateP95)} | ${f(b?.changeP95)} | ${f(b?.waitP95)} | ${f(b?.renderP95)} | ${f(b?.popupKeys)} | ${f(b?.popupP95)} | ${f(b?.popupDispatchP95)} | ${f(b?.popupLayoutP95)} | ${f(b?.popupRenderP95)} | ${f(b?.noPopupP95)} |`;
      }),
      "",
    ].join("\n"),
  );
}

async function main(argv) {
  const short = argv.includes("--short");
  const dry = argv.includes("--dry-run");
  const at = argv.indexOf("--only");
  const only = at >= 0 ? (argv[at + 1] ?? "").split(",").filter(Boolean) : undefined;
  const raAt = argv.indexOf("--ra");
  const raGiven = raAt >= 0 ? argv[raAt + 1] : undefined;
  measurements({ short, only, ra: raGiven });
  if (process.platform !== "darwin" && !dry) {
    console.error(`perf-m4: macOS only; this is ${process.platform}`);
    return 1;
  }
  const now = new Date();
  const pad = (n) => String(n).padStart(2, "0");
  const date = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  const stamp = `${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  const suffix = short ? "-short" : "";
  if (dry) {
    for (const m of measurements({ short, only, ra: raGiven }))
      if (m.alternate)
        for (const { round, side, args } of alternateRuns(m))
          console.log(
            `perf-m4: ${m.kind} round ${round}: pw-perf ${args.join(" ")} --out ${alternateOut(m, side, round, suffix, date, stamp)}`,
          );
      else
        console.log(
          m.args
            ? `perf-m4: pw-perf ${m.args.join(" ")} --out target/perf/m4-${m.kind}${suffix}-${date}-${stamp}`
            : `perf-m4: ${m.kind}: cargo test --release -p pw-index --test perf × ${short ? 1 : RUST_RUNS}`,
        );
    return 0;
  }
  const conditions = {
    started: now.toISOString(),
    commit: must("git", ["rev-parse", "HEAD"]).trim(),
    dirty: must("git", ["status", "--porcelain"]).trim() !== "",
    cores: cores(),
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
    const ready = prepare(raGiven);
    conditions.prepared = {
      app: APP,
      fake: FAKE,
      symbolsRepo: SYMBOLS_REPO,
      repoCopy: REPO_COPY,
      ...ready,
    };
    conditions.only = only ?? null;
    save();
    for (const step of measurements({ short, only, ra: ready.ra })) {
      const out = `target/perf/m4-${step.kind}${suffix}-${date}-${stamp}`;
      if (step.kind === "rust-index") {
        const waited = await waitForStart(step.kind, false);
        const record = { kind: step.kind, ...waited, startedAt: new Date().toISOString() };
        conditions.steps.push(record);
        save();
        rustIndex(out, short);
        record.endedAt = new Date().toISOString();
        record.exit = 0;
        save();
        done.push(step.kind);
        continue;
      }
      if (step.alternate) {
        const rounds = [];
        for (const { round: r, side, args } of alternateRuns(step)) {
          const sub = alternateOut(step, side, r, suffix, date, stamp);
          const record = await pwPerf(side, args, sub, conditions, save, { round: r });
          if (record.exit === 3) worst = 3;
          const result = JSON.parse(readFileSync(join(root, sub, "result.json"), "utf8"));
          rounds.push({
            round: r,
            kind: side.kind,
            lsp: side.kind === "typing-on",
            exit: record.exit,
            loadAtStart: record.loadAtStart,
            loadAtEnd: record.loadAtEnd,
            displaysKept: record.displaysKept,
            typing: typingRun(result),
            coldStartMs: result.runs?.find((x) => !x.warmup)?.cold_start_ms ?? null,
            os: result.os ?? null,
            commit: result.commit ?? null,
          });
        }
        writeTyping(out, rounds);
        done.push(step.kind);
        continue;
      }
      const record = await pwPerf(step, step.args, out, conditions, save);
      done.push(step.kind);
      if (record.exit === 3) {
        worst = 3;
        console.error(`perf-m4: ${step.kind}: too few valid runs (exit 3) — see its summary`);
      }
    }
  } catch (e) {
    console.error(`perf-m4: ${e.message}`);
    conditions.error = e.message;
    worst = e.exit ?? 1;
  } finally {
    conditions.ended = new Date().toISOString();
    save();
    const reports = join(root, "docs/superpowers/reports");
    mkdirSync(reports, { recursive: true });
    for (const [src, dst] of reportCopies({ date, stamp, suffix, kinds: done })) {
      copyFileSync(join(root, src), join(root, dst), constants.COPYFILE_EXCL);
      console.log(`perf-m4: ${src} → ${dst}`);
    }
  }
  return worst;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then(
    (code) => (process.exitCode = code),
    (e) => {
      console.error(`perf-m4: ${e.message}`);
      process.exitCode = 1;
    },
  );
}
