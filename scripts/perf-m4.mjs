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
//      정의로 이동 · 문제 툴 윈도우), `typing-on` · `typing-off`(타이핑 지연), `symbols` · `classes`(합성 저장소: 첫 색인 · ⌘⌥O · ⌘O),
//      `ra-idle`(이 저장소 + rust-analyzer: 콜드 스타트 · 유휴 메모리, 서버는 따로).
//   ⑤ 끝나면(실패해도) 끝난 단계마다 `result.json` · `summary.md` 를 `docs/superpowers/reports/<날짜>-m4-perf-<종류>[-short]-<시각>.json|
//      .summary.md` 로 복사하고, 조건(커밋 · 단계별 load average · 기다린 시간 · 화면)을 `…-m4-perf-conditions[-short]-<시각>.json` 에
//      남긴다. 측정 도중에는 `target/perf/` 에만 쓴다 — 추적되는 경로에 쓰면 다음 단계의 `commit.dirty` 가 true 가 된다.
//
// 단계를 시작하기 직전마다 화면 잠금을 보고 **잠겨 있으면 풀릴 때까지 기다린다**(키 입력은 보내지 않는다 — 앱 안 시나리오의 키는
// 웹뷰 안에서 만든 이벤트다). 1분 load average 가 코어 수를 넘으면 내려갈 때까지 기다린다. 기다린 시간은 조건 파일에 적는다.
// 종료 코드: 0 — 모든 단계가 0. 3 — 결과는 복사했지만 어느 단계든 유효한 회가 적었다. 그 밖 — 실패한 단계의 코드 그대로이고 뒤
// 단계는 돌지 않는다. macOS 전용이다.

import { spawnSync } from "node:child_process";
import { constants, copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
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
      kind: "typing-on",
      args: [
        ...app,
        "--scenario",
        "lsp-typing",
        "--lsp-fake",
        FAKE,
        "--label",
        "typing, LSP on (fake server)",
        ...extra,
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
        ...extra,
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
 * 화면이 풀리고(`screen` 일 때 — 앱이 없는 단계는 잠금과 무관하다) 1분 load average 가 코어 수 이하가 될 때까지 기다린다. 기다린 초와
 * 시작할 때의 load.
 */
async function waitForStart(label, screen = true) {
  const limit = cores();
  const started = Date.now();
  let lockedSecs = 0;
  for (;;) {
    if (screen && locked()) {
      console.error(`perf-m4: ${label}: the screen is locked — waiting (no keys are sent)`);
      await sleep(30_000);
      lockedSecs += 30;
      continue;
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
      if (!step.args) {
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
      const waited = await waitForStart(step.kind);
      const args = ["run", "-q", "--release", "-p", "pw-perf", "--", ...step.args, "--out", out];
      console.log(`perf-m4: cargo ${args.join(" ")}`);
      const record = {
        kind: step.kind,
        args: step.args,
        ...waited,
        startedAt: new Date().toISOString(),
      };
      conditions.steps.push(record);
      save();
      const r = spawnSync("cargo", args, { cwd: root, stdio: "inherit" });
      record.endedAt = new Date().toISOString();
      record.loadAtEnd = load();
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
