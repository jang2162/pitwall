// M3-21 실기 측정을 한 번에 돌리고 결과를 보고서 폴더로 옮긴다(앱을 앞으로 띄우므로 사람이 기기를 비웠을 때만).
//
//   node scripts/perf-m3.mjs [--short] [--only <종류>,…] [--dry-run] [--keep-db]     (`mise run perf-m3 -- --short`)
//   node scripts/perf-m3.mjs check-default    — 데스크톱 크레이트의 기본 feature 가 가짜 드라이버를 켜지 않는지(`mise run lint`)
//
// 차례(정본은 docs/ops/m3-realcheck.md, 측정 방법은 CONTRIBUTING.md 「성능 기준선」 의 「DB 시나리오」):
//   ① 도커로 이 워크트리 전용 PG(10만 행 × 10열 테이블 `perf_rows` 와 같은 행을 5,000행마다 150 ms 쉬며 내는 뷰 `perf_stream`, trust 인증) · Redis(키 100만 개
//      `k:0` … `k:999999`)를 빈 포트에 띄운다 — CI `db-live` 와 같은 이미지. 이름은 `pitwall-perf-<워크트리 해시>-pg|redis` 이고
//      끝나면 그 둘만 끈다(`--keep-db` 면 남긴다). 같은 이름이 이미 있으면(앞 실행이 남긴 것) 지우고 다시 띄운다.
//   ② 데스크톱 release 번들을 두 벌 만든다: 기본(출시와 같다)과 `perf-fake-db` feature 를 켠 것(`FAKE_FEATURE` — 가짜 postgres
//      드라이버가 든다, apps/desktop/src-tauri/src/perf_fake_db.rs). 각각 `target/release/perf-m3/<default|fake>/Pitwall.app` 로
//      옮긴다. 기본 번들의 바이너리에 가짜의 로그 글(`FAKE_MARK`)이 없고 가짜 번들에는 있는지 본다 — 어긋나면 멈춘다.
//   ③ pw-perf 를 차례로: `empty-default` · `empty-fake`(빈 프로젝트의 콜드 스타트 · 유휴 메모리 — 두 번들의 차이가 잡음 안인지),
//      `db-idle`(기본 번들, PG 연결 하나를 연 프로젝트), `db-fake`(가짜 번들 — 그리드 · 탐색기는 가짜 postgres, Redis 는 ①의
//      서버), `db-pg`(기본 번들 — 그리드를 ①의 PG 로).
//   ④ 끝나면(실패해도) 끝난 단계마다 `result.json` · `summary.md` 를
//      `docs/superpowers/reports/<날짜>-m3-perf-<종류>[-short]-<시각>.json|.summary.md` 로 복사하고, 조건(커밋 · 단계별 load
//      average 와 기다린 시간 · 화면과 주사율)을 `…-m3-perf-conditions[-short]-<시각>.json` 에 남긴다. 측정 도중에는 `target/perf/`
//      에만 쓴다 — 추적되는 경로에 쓰면 다음 단계의 `commit.dirty` 가 true 가 된다(`reportCopies`).
//
// `--only empty-default,db-fake` 처럼 측정 단계를 고른다(번들 둘은 늘 만들고 견준다 — 실패한 단계만 다시 잴 때).
// `--short` 는 측정 단계마다 `--runs 1 --warmup 0`(끝까지 도는지만 본다 — 파일 이름에 `-short`). 단계를 시작하기 직전마다 화면
// 잠금을 보고(잠겨 있으면 멈춘다), 1분 load average 가 코어 수를 넘으면 내려갈 때까지 기다린다(기다린 시간을 조건 파일에 적는다).
// OS 키 입력은 보내지 않는다 — 앱 안 시나리오의 키는 웹뷰 안에서 만든 이벤트다.
// 종료 코드: 0 — 모든 단계가 0. 3 — 결과는 복사했지만 어느 단계든 유효한 회가 적었다(pw-perf 의 3). 그 밖 — 실패한 단계의 코드
// 그대로이고 뒤 단계는 돌지 않는다. macOS 전용이다.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
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

import { PG_IMAGE, REDIS_IMAGE } from "./db-tls.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));

/** 가짜 드라이버를 넣는 cargo feature(apps/desktop/src-tauri/Cargo.toml 「features」 와 같은 이름). */
export const FAKE_FEATURE = "perf-fake-db";
/** 가짜를 등록한 앱이 남기는 로그 글(perf_fake_db.rs 의 `LOG_MARK`, crates/pw-perf/src/db.rs 의 `FAKE_DB_LOG`) — 바이너리에 든다. */
export const FAKE_MARK = "perf-fake-db: fake postgres driver registered";
export const PG_ROWS = 100_000;
export const REDIS_KEYS = 1_000_000;
const BUNDLE = "target/release/bundle/macos/Pitwall.app";
const BUNDLES = {
  default: "target/release/perf-m3/default/Pitwall.app",
  fake: "target/release/perf-m3/fake/Pitwall.app",
};

/** `sysctl -n vm.loadavg` 의 `{ 1.2 3.4 5.6 }` 를 읽는다. */
export function parseLoad(text) {
  const m = /\{\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*\}/.exec(text);
  if (!m) throw new Error(`cannot read the load average from ${JSON.stringify(text)}`);
  return [Number(m[1]), Number(m[2]), Number(m[3])];
}

/** 바이너리(Buffer)에 `mark` 가 들었는가. */
export function contains(binary, mark) {
  return binary.includes(Buffer.from(mark, "utf8"));
}

/** 번들 둘의 가짜 코드 판정: 기본에는 없고 가짜에는 있어야 한다. 어긋나면 까닭, 맞으면 `undefined`. */
export function bundleProblem(defaultBinary, fakeBinary) {
  if (contains(defaultBinary, FAKE_MARK))
    return `the default bundle contains the fake driver (${FAKE_MARK}) — it must not ship`;
  if (fakeBinary !== undefined && !contains(fakeBinary, FAKE_MARK))
    return `the ${FAKE_FEATURE} bundle has no fake driver (${FAKE_MARK}) — the feature did not reach the build`;
  return undefined;
}

/** 측정 단계(pw-perf 인자). `pg` · `redis` 는 `host:port`. `only` 가 있으면 그 종류만(모르는 이름은 오류). */
export function measurements({ pg, redis, short, only }) {
  const extra = short ? ["--runs", "1", "--warmup", "0"] : [];
  const all = [
    {
      kind: "empty-default",
      args: [
        "--app",
        BUNDLES.default,
        "--label",
        "default bundle (release, no perf-fake-db)",
        ...extra,
      ],
    },
    {
      kind: "empty-fake",
      args: [
        "--app",
        BUNDLES.fake,
        "--label",
        `${FAKE_FEATURE} bundle (release + ${FAKE_FEATURE})`,
        ...extra,
      ],
    },
    {
      kind: "db-idle",
      args: [
        "--app",
        BUNDLES.default,
        "--db-idle",
        "--pg",
        `${pg}/perf`,
        "--label",
        "default bundle, one PG connection opened",
        ...extra,
      ],
    },
    {
      kind: "db-fake",
      args: [
        "--app",
        BUNDLES.fake,
        "--db",
        "--fake-db",
        "--redis",
        redis,
        "--label",
        `${FAKE_FEATURE} bundle: fake postgres grid/explorer + Redis 1M keys`,
        ...extra,
      ],
    },
    {
      kind: "db-pg",
      args: [
        "--app",
        BUNDLES.default,
        "--db",
        "--pg",
        `${pg}/perf`,
        "--label",
        "default bundle: local PG grid (100k × 10)",
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

/** 가짜 드라이버 feature 를 둔 크레이트(apps/desktop/src-tauri). */
export const DESKTOP_CRATE = "pitwall-desktop";

/**
 * `cargo metadata` 에서 데스크톱 크레이트의 `default` feature 가 (다른 feature 를 거쳐서라도) 가짜 드라이버를 켜는지.
 * 켜면 그 까닭을, 아니면 null. 시험 · 린트는 `--all-features` 로 돌아 기본으로 켜져도 실패하지 않으므로
 * `mise run lint` 가 이것으로 본다(`check-default`). 다른 크레이트가 의존성 선언에서 이 feature 를 켜는 길은 보지 않는다.
 */
export function defaultFeatureProblem(metadata) {
  const pkg = metadata.packages.find((p) => p.name === DESKTOP_CRATE);
  if (!pkg) return `${DESKTOP_CRATE} is not in cargo metadata`;
  const seen = new Set();
  const stack = ["default"];
  while (stack.length) {
    const name = stack.pop();
    if (seen.has(name)) continue;
    seen.add(name);
    for (const item of pkg.features[name] ?? []) if (item in pkg.features) stack.push(item);
  }
  return seen.has(FAKE_FEATURE)
    ? `${DESKTOP_CRATE}: the default features turn on ${FAKE_FEATURE} — release bundles would carry the fake driver`
    : null;
}

/**
 * 측정이 끝난 뒤 보고서 폴더로 옮길 [원본, 대상] 쌍 — 조건 파일과 끝난 단계마다 `result.json` · `summary.md`.
 * 원본은 모두 `target/perf/` 아래다: 측정 도중 추적되는 경로(`docs/`)에 무엇이든 쓰면 다음 단계의 pw-perf 가
 * `git status` 로 트리를 dirty 로 읽어 `commit.dirty` 가 true 로 남는다.
 */
export function reportCopies({ date, stamp, suffix, kinds }) {
  const reports = "docs/superpowers/reports";
  const pairs = [
    [
      `target/perf/m3-conditions${suffix}-${date}-${stamp}.json`,
      `${reports}/${date}-m3-perf-conditions${suffix}-${stamp}.json`,
    ],
  ];
  for (const kind of kinds)
    for (const [from, ext] of [
      ["result.json", "json"],
      ["summary.md", "summary.md"],
    ])
      pairs.push([
        `target/perf/m3-${kind}${suffix}-${date}-${stamp}/${from}`,
        `${reports}/${date}-m3-perf-${kind}${suffix}-${stamp}.${ext}`,
      ]);
  return pairs;
}

/** Redis 의 MSET 명령들(RESP) — `k:<i>` 의 값은 `<i>`(live 시험 `kv_live.rs` 의 100만 키와 같다). */
export function redisSeed(keys = REDIS_KEYS, batch = 10_000) {
  const parts = [];
  const bulk = (s) => `$${Buffer.byteLength(s)}\r\n${s}\r\n`;
  for (let from = 0; from < keys; from += batch) {
    const n = Math.min(batch, keys - from);
    let cmd = `*${1 + 2 * n}\r\n${bulk("MSET")}`;
    for (let i = from; i < from + n; i++) cmd += bulk(`k:${i}`) + bulk(String(i));
    parts.push(cmd);
  }
  return parts.join("");
}

/**
 * 스트림 뷰 `perf_stream` 의 몰아 보내기: `PG_STREAM_BURST` 행마다 서버가 `PG_STREAM_PAUSE_MS` 쉰다(10만 행이면 19번, 3초 남짓).
 * 쉬지 않는 뷰는 로컬 PG 가 10만 행을 0.35초에 다 보내 스트리밍 중 스크롤이 17 프레임뿐이라 판정하지 못했다(2026-11-01 보고서
 * §3 ⑧). 행을 늘리거나 넓혀서는 늘릴 수 없다 — 행 수는 그리드 보유 상한(`[db] grid_max_rows`, 기본 10만)이, 바이트는
 * `grid_max_bytes`(기본 64 MiB)가 막고, 넓힌 행(행당 1 KB)도 64 MiB 를 0.36초에 보냈다(2026-11-02 짧은 실기). 5,000행은 쉬지 않는
 * 스트림이 한 프레임(16.7 ms)에 보내는 양(10만 행 / 0.35초 ≈ 4,800행)이라, 한 묶음은 쉬지 않는 스트림의 한 프레임만큼 몰아 온다.
 * 판정은 여전히 pw-perf 가 60 프레임 이상에서만 한다(`MIN_STREAM_FRAMES`).
 */
export const PG_STREAM_BURST = 5_000;
export const PG_STREAM_PAUSE_MS = 150;

/**
 * PG 픽스처: 10열(첫 열 정수, 나머지 `r<행>c<열>` — 가짜 드라이버의 행과 같은 꼴, 첫 행 둘째 칸이 `r0c1`). `perf_stream` 은 같은
 * 행 · 같은 값 · 같은 순서의 뷰이고, `c1` 에 붙인 `pg_sleep(…)::text`(빈 글)로 묶음 사이에 쉰다. 휘발 함수라 정렬 뒤 내보내는
 * 행마다 계산된다(인덱스 스캔이든 정렬이든 — `EXPLAIN VERBOSE` 로 확인).
 */
export function pgSeed(rows = PG_ROWS, burst = PG_STREAM_BURST, pauseMs = PG_STREAM_PAUSE_MS) {
  const cols = Array.from({ length: 9 }, (_, k) => `'r' || i || 'c${k + 1}' AS c${k + 1}`).join(
    ", ",
  );
  const pause = `CASE WHEN id > 0 AND id % ${burst} = 0 THEN pg_sleep(${pauseMs / 1000})::text ELSE '' END`;
  const rest = Array.from({ length: 8 }, (_, k) => `c${k + 2}`).join(", ");
  return (
    [
      "DROP VIEW IF EXISTS perf_stream",
      "DROP TABLE IF EXISTS perf_rows",
      `CREATE TABLE perf_rows AS SELECT i::int8 AS id, ${cols} FROM generate_series(0, ${rows - 1}) AS i`,
      "ALTER TABLE perf_rows ADD PRIMARY KEY (id)",
      `CREATE VIEW perf_stream AS SELECT id, c1 || ${pause} AS c1, ${rest} FROM perf_rows ORDER BY id`,
      "ANALYZE perf_rows",
    ].join(";\n") + ";\n"
  );
}

function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { cwd: root, encoding: "utf8", maxBuffer: 1 << 28, ...opts });
  return r;
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

function locked() {
  const r = run("ioreg", ["-n", "Root", "-d1", "-a"]);
  if (r.status !== 0) throw new Error("cannot read the screen lock state (ioreg)");
  return /CGSSessionScreenIsLocked<\/key>\s*<true\/>/.test(r.stdout ?? "");
}

function load() {
  return parseLoad(must("sysctl", ["-n", "vm.loadavg"]));
}

const cores = () => Number(must("sysctl", ["-n", "hw.ncpu"]).trim());

/** 1분 load average 가 코어 수 이하가 될 때까지 기다린다. 기다린 초와 시작할 때의 load 를 돌려준다. */
async function waitForQuiet(label) {
  const limit = cores();
  const started = Date.now();
  let now = load();
  while (now[0] > limit) {
    console.error(`perf-m3: ${label}: 1-minute load ${now[0]} > ${limit} cores — waiting`);
    await sleep(15_000);
    now = load();
  }
  return { waitedSecs: Math.round((Date.now() - started) / 1000), loadAtStart: now };
}

function worktreeTag() {
  return createHash("sha256").update(root).digest("hex").slice(0, 8);
}

/** 컨테이너를 띄우고 그 포트를 돌려준다. 같은 이름이 있으면(앞 실행이 남긴 이 워크트리의 것) 지운다. */
function startContainer(name, image, innerPort, args) {
  run("docker", ["rm", "-f", name]);
  must("docker", [
    "run",
    "-d",
    "--rm",
    "--name",
    name,
    "-p",
    `127.0.0.1::${innerPort}`,
    ...args,
    image,
  ]);
  const out = must("docker", ["port", name, `${innerPort}/tcp`]);
  const m = /:(\d+)\s*$/m.exec(out.trim().split("\n")[0]);
  if (!m) throw new Error(`cannot read the port of ${name}: ${out}`);
  return Number(m[1]);
}

async function until(what, test, timeoutMs = 120_000) {
  const started = Date.now();
  for (;;) {
    if (test()) return;
    if (Date.now() - started > timeoutMs) throw new Error(`timed out waiting for ${what}`);
    await sleep(500);
  }
}

async function startDatabases(names) {
  const pgPort = startContainer(names.pg, PG_IMAGE, 5432, [
    "-e",
    "POSTGRES_HOST_AUTH_METHOD=trust",
    "-e",
    "POSTGRES_DB=perf",
  ]);
  const redisPort = startContainer(names.redis, REDIS_IMAGE, 6379, []);
  await until(
    "PostgreSQL",
    () =>
      run("docker", ["exec", names.pg, "pg_isready", "-U", "postgres", "-d", "perf"]).status === 0,
  );
  // initdb 뒤 한 번 다시 시작하는 이미지라 준비 직후의 연결이 끊길 수 있다 — 쿼리가 될 때까지.
  await until(
    "PostgreSQL queries",
    () =>
      run("docker", ["exec", names.pg, "psql", "-U", "postgres", "-d", "perf", "-c", "SELECT 1"])
        .status === 0,
  );
  await until(
    "Redis",
    () => run("docker", ["exec", names.redis, "redis-cli", "ping"]).stdout?.trim() === "PONG",
  );
  must(
    "docker",
    ["exec", "-i", names.pg, "psql", "-v", "ON_ERROR_STOP=1", "-q", "-U", "postgres", "-d", "perf"],
    { input: pgSeed() },
  );
  const rows = must("docker", [
    "exec",
    names.pg,
    "psql",
    "-tA",
    "-U",
    "postgres",
    "-d",
    "perf",
    "-c",
    "SELECT count(*) FROM perf_stream",
  ]).trim();
  if (Number(rows) !== PG_ROWS) throw new Error(`perf_stream has ${rows} rows, not ${PG_ROWS}`);
  must("docker", ["exec", "-i", names.redis, "redis-cli", "--pipe"], { input: redisSeed() });
  const size = must("docker", ["exec", names.redis, "redis-cli", "DBSIZE"]).trim();
  if (Number(size) !== REDIS_KEYS) throw new Error(`Redis has ${size} keys, not ${REDIS_KEYS}`);
  return { pg: `127.0.0.1:${pgPort}`, redis: `127.0.0.1:${redisPort}` };
}

function stopDatabases(names) {
  for (const name of [names.pg, names.redis]) run("docker", ["rm", "-f", name]);
}

function buildBundle(kind) {
  const args = ["--filter", "@pitwall/desktop", "tauri", "build", "--bundles", "app"];
  if (kind === "fake") args.push("--features", FAKE_FEATURE);
  console.log(`perf-m3: pnpm ${args.join(" ")}`);
  const r = spawnSync("pnpm", args, { cwd: root, stdio: "inherit" });
  if (r.status !== 0)
    throw new Error(`building the ${kind} bundle failed (exit ${r.status ?? r.signal})`);
  const dest = join(root, BUNDLES[kind]);
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(join(dest, ".."), { recursive: true });
  must("ditto", [join(root, BUNDLE), dest]);
  return readFileSync(join(dest, "Contents/MacOS/pitwall"));
}

function displays() {
  const r = run("system_profiler", ["SPDisplaysDataType", "-json"]);
  if (r.status !== 0) return null;
  try {
    const gpus = JSON.parse(r.stdout).SPDisplaysDataType ?? [];
    return gpus.flatMap((g) =>
      (g.spdisplays_ndrvs ?? []).map((d) => ({
        name: d._name,
        resolution: d._spdisplays_resolution ?? d.spdisplays_resolution ?? null,
        pixels: d._spdisplays_pixels ?? null,
        main: d.spdisplays_main === "spdisplays_yes",
        internal: d.spdisplays_connection_type === "spdisplays_internal",
        connection: d.spdisplays_connection_type ?? null,
      })),
    );
  } catch {
    return null;
  }
}

async function main(argv) {
  if (argv[0] === "check-default") {
    const problem = defaultFeatureProblem(
      JSON.parse(must("cargo", ["metadata", "--no-deps", "--format-version", "1"])),
    );
    if (problem) console.error(`perf-m3: ${problem}`);
    return problem ? 1 : 0;
  }
  const short = argv.includes("--short");
  const dry = argv.includes("--dry-run");
  const keepDb = argv.includes("--keep-db");
  const at = argv.indexOf("--only");
  const only = at >= 0 ? (argv[at + 1] ?? "").split(",").filter(Boolean) : undefined;
  measurements({ pg: "", redis: "", short, only });
  if (process.platform !== "darwin" && !dry) {
    console.error(`perf-m3: macOS only; this is ${process.platform}`);
    return 1;
  }
  const now = new Date();
  const pad = (n) => String(n).padStart(2, "0");
  const date = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  const stamp = `${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  const suffix = short ? "-short" : "";
  const reports = join(root, "docs/superpowers/reports");
  const tag = worktreeTag();
  const names = { pg: `pitwall-perf-${tag}-pg`, redis: `pitwall-perf-${tag}-redis` };
  if (dry) {
    for (const m of measurements({ pg: "127.0.0.1:<pg>", redis: "127.0.0.1:<redis>", short, only }))
      console.log(
        `perf-m3: pw-perf ${m.args.join(" ")} --out target/perf/m3-${m.kind}${suffix}-${date}-${stamp}`,
      );
    return 0;
  }
  const conditions = {
    started: now.toISOString(),
    commit: must("git", ["rev-parse", "HEAD"]).trim(),
    dirty: must("git", ["status", "--porcelain"]).trim() !== "",
    cores: cores(),
    displays: displays(),
    containers: { ...names, pgImage: PG_IMAGE, redisImage: REDIS_IMAGE },
    steps: [],
  };
  // 측정 도중에는 target/perf 에만 쓴다 — 보고서 폴더로 옮기는 것은 finally 에서 한 번(reportCopies).
  const conditionsFile = join(root, reportCopies({ date, stamp, suffix, kinds: [] })[0][0]);
  const save = () => {
    mkdirSync(dirname(conditionsFile), { recursive: true });
    writeFileSync(conditionsFile, `${JSON.stringify(conditions, null, 2)}\n`);
  };
  const done = [];
  let worst = 0;
  try {
    console.log(`perf-m3: starting ${names.pg} and ${names.redis}`);
    const db = await startDatabases(names);
    conditions.databases = db;
    const defaultBinary = buildBundle("default");
    const fakeBinary = buildBundle("fake");
    const problem = bundleProblem(defaultBinary, fakeBinary);
    if (problem) throw new Error(problem);
    conditions.bundles = { ...BUNDLES, defaultHasFake: false, fakeHasFake: true };
    conditions.only = only ?? null;
    for (const step of measurements({ ...db, short, only })) {
      if (locked())
        throw new Error(
          `the screen is locked — not starting ${step.kind}; unlock it and leave the device alone`,
        );
      const quiet = await waitForQuiet(step.kind);
      const out = `target/perf/m3-${step.kind}${suffix}-${date}-${stamp}`;
      const args = ["run", "-q", "--release", "-p", "pw-perf", "--", ...step.args, "--out", out];
      console.log(`perf-m3: cargo ${args.join(" ")}`);
      const record = {
        kind: step.kind,
        args: step.args,
        ...quiet,
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
      for (const from of ["result.json", "summary.md"]) {
        const src = join(root, out, from);
        if (!existsSync(src)) throw new Error(`${step.kind}: ${src} was not written`);
      }
      done.push(step.kind);
      if (r.status === 3) {
        worst = 3;
        console.error(
          `perf-m3: ${step.kind}: too few valid runs (exit 3) — see the warning in its summary`,
        );
      }
    }
  } catch (e) {
    console.error(`perf-m3: ${e.message}`);
    conditions.error = e.message;
    worst = e.exit ?? 1;
  } finally {
    conditions.ended = new Date().toISOString();
    save();
    if (!keepDb) stopDatabases(names);
    mkdirSync(reports, { recursive: true });
    for (const [src, dst] of reportCopies({ date, stamp, suffix, kinds: done })) {
      copyFileSync(join(root, src), join(root, dst), constants.COPYFILE_EXCL);
      console.log(`perf-m3: ${src} → ${dst}`);
    }
  }
  return worst;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then(
    (code) => (process.exitCode = code),
    (e) => {
      console.error(`perf-m3: ${e.message}`);
      process.exitCode = 1;
    },
  );
}
