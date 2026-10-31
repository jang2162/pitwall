// 실 DB 시험(live_*)을 DB 마다 따로 돌리고 실행 개수를 하한(scripts/db-live.toml)과 견준다(14-m3-plan.md M3-01, 결정 M3-D16).
//
//   node scripts/db-live.mjs [--ci] [db …]     db: pg · mysql · mariadb · redis(없으면 전부)
//
// CI 의 `db-live` 잡이 DB 마다 한 단계씩, `mise run test-db` 가 전부를 한 번에 부른다. DB 마다:
//   1. URL 환경 변수(PW_TEST_<DB>_URL)가 없으면 로컬은 「건너뜀」 으로 알리고 통과로 세지 않는다 — 모두 건너뛰었으면 종료 코드
//      3 이다. `--ci` 면 실패다.
//   2. `cargo nextest list` 로 필터가 고른 시험을 세고, `cargo nextest run --release --no-tests=fail` 로 돌린다 — 필터가
//      0개를 고르면 nextest 가 실패한다(준비 확인 시험이 있으므로 정상이면 늘 1개 이상).
//   3. 돈 수(nextest 의 Summary 줄)가 고른 수와 같은지, 준비 확인 시험(`live_<db>_ready`)을 뺀 수가 하한 이상인지 본다.
// MariaDB 는 MySQL 시험(live_mysql)을 그 URL 로 한 번 더 돈다 — PW_TEST_MARIADB_URL 을 PW_TEST_MYSQL_URL 로 넘긴다.
// 실 DB 시험을 다는 법은 crates/pw-db-driver/src/live.rs 머리 주석.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { TARGETS as TLS_TARGETS, makeCertificates, startTls, stopTls } from "./db-tls.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));

// DB → (URL 변수, nextest 이름 필터, 시험에 넘길 변수, 함께 쓰는 TLS 시험 서버 — scripts/db-tls.mjs 의 TARGETS). 새 단계(M3-11 의
// ssh_tunnel 등)는 여기와 db-live.toml 에 한 줄씩.
export const TARGETS = {
  pg: { env: "PW_TEST_PG_URL", filter: "live_pg", testEnv: "PW_TEST_PG_URL", tls: ["pg"] },
  mysql: { env: "PW_TEST_MYSQL_URL", filter: "live_mysql", testEnv: "PW_TEST_MYSQL_URL" },
  mariadb: { env: "PW_TEST_MARIADB_URL", filter: "live_mysql", testEnv: "PW_TEST_MYSQL_URL" },
  redis: {
    env: "PW_TEST_REDIS_URL",
    filter: "live_redis",
    testEnv: "PW_TEST_REDIS_URL",
    tls: ["redis"],
  },
};

/** `name = 정수` 줄들. 빈 줄 · `#` 주석은 건너뛴다. 모르는 꼴 · 같은 이름 두 번은 오류다. */
export function parseFloors(text) {
  const floors = new Map();
  text.split("\n").forEach((raw, i) => {
    const line = raw.replace(/#.*/, "").trim();
    if (line === "") return;
    const match = /^([a-z_]+)\s*=\s*(\d+)$/.exec(line);
    if (!match)
      throw new Error(`db-live.toml line ${i + 1}: expected \`name = <count>\`, got ${raw}`);
    if (floors.has(match[1])) throw new Error(`db-live.toml: ${match[1]} is listed twice`);
    floors.set(match[1], Number(match[2]));
  });
  return floors;
}

/** `cargo nextest list --message-format json` 에서 필터가 고른(무시되지 않은) 시험 이름. */
export function selectedTests(listJson) {
  const names = [];
  for (const [binary, suite] of Object.entries(listJson["rust-suites"] ?? {})) {
    for (const [name, test] of Object.entries(suite.testcases ?? {})) {
      if (test["filter-match"]?.status === "matches" && !test.ignored)
        names.push(`${binary} ${name}`);
    }
  }
  return names.sort();
}

/**
 * 그 DB 의 nextest 필터 식. 실 DB 시험은 함수 이름이 `live_<db>_` 로 시작한다(CONTRIBUTING.md 「명령」 의 「실 DB 시험」) — 이름
 * 가운데에 `live_pg` 가 든 단위 시험(`…_on_live_pg_are_not_reads`)은 고르지 않는다. 경로 조각(`::`)의 처음에만 맞춘다.
 */
export function filterExpr(filter) {
  return `test(/(^|::)${filter}_/)`;
}

/** 준비 확인 시험인가(이름의 마지막 조각이 `<필터>_ready`). */
export function isReadyTest(name, filter) {
  const last = name.split(/::|\s/).at(-1);
  return last === `${filter}_ready`;
}

/**
 * nextest 의 `Summary [ … ] N test(s) run: …` 줄의 N. 없으면 null. 색 코드는 먼저 지운다 — CI 의 `CARGO_TERM_COLOR=always` 가
 * `--color never` 보다 앞서는 출력이 섞여도 센다.
 */
export function runCount(output) {
  // oxlint-disable-next-line no-control-regex -- ANSI 색 코드(ESC)를 지운다.
  const plain = output.replace(/\x1b\[[0-9;]*m/g, "");
  const matches = [...plain.matchAll(/Summary \[[^\]]*\]\s+(\d+) tests? run:/g)];
  return matches.length === 0 ? null : Number(matches.at(-1)[1]);
}

/** 한 DB 의 판정. */
export function judge({ db, filter, selected, run, floor, exitCode }) {
  const ready = selected.filter((n) => isReadyTest(n, filter)).length;
  const counted = selected.length - ready;
  const line = `${db}: ${run ?? "?"} run (selected ${selected.length}, ready checks ${ready}, counted ${counted}), floor ${floor}`;
  if (exitCode !== 0) return { ok: false, message: `${line} — nextest failed (exit ${exitCode})` };
  if (selected.length === 0)
    return { ok: false, message: `${line} — the filter selected no tests` };
  if (run === null) {
    return {
      ok: false,
      message: `${line} — could not read how many tests nextest ran (no Summary line)`,
    };
  }
  if (run !== selected.length) {
    return {
      ok: false,
      message: `${line} — nextest ran ${run} but the filter selected ${selected.length}`,
    };
  }
  if (counted < floor)
    return { ok: false, message: `${line} — below the floor in scripts/db-live.toml` };
  return { ok: true, message: `${line} — ok` };
}

/** 사람이 끊었다(Ctrl-C) — 띄운 TLS 서버를 끄고 끝낸다. */
class Interrupted extends Error {}

function cargo(args, env, { capture }) {
  const result = spawnSync("cargo", args, {
    cwd: root,
    env,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
  if (result.error) throw result.error;
  if (result.signal === "SIGINT" || result.signal === "SIGTERM")
    throw new Interrupted(`cargo stopped by ${result.signal}`);
  return result;
}

/** 시험이 사는 크레이트: 워크스페이스의 pw-db*. 데스크톱(Tauri) 크레이트까지 빌드하지 않게 좁힌다. */
function packages(env) {
  const meta = cargo(["metadata", "--no-deps", "--format-version", "1"], env, { capture: true });
  if (meta.status !== 0) throw new Error(`cargo metadata failed:\n${meta.stderr}`);
  return JSON.parse(meta.stdout)
    .packages.map((p) => p.name)
    .filter((name) => name.startsWith("pw-db"))
    .sort()
    .flatMap((name) => ["-p", name]);
}

/**
 * 띄워야 할 TLS 시험 서버(scripts/db-tls.mjs 의 db 이름들). 그 DB 의 URL 이 있고(시험이 돈다), TLS URL 이 없고, 도커가 있을 때만 —
 * CI 는 `tls test servers` 단계가 먼저 띄워 TLS URL 이 있다. 도커가 없으면 띄우지 않는다(그 TLS 시험은 건너뛰지 않고 실패한다).
 */
export function tlsToStart(target, env, dockerAvailable) {
  if (!target.tls || !env[target.env] || !dockerAvailable) return [];
  return target.tls.filter((db) => !env[TLS_TARGETS[db].urlEnv]);
}

function dockerAvailable() {
  const result = spawnSync("docker", ["info", "--format", "{{.ServerVersion}}"], {
    encoding: "utf8",
  });
  return !result.error && result.status === 0;
}

/** 이 워크트리 · 이 실행만의 컨테이너 이름 — 다른 워크트리의 서버와 겹치지 않고, 끌 때 이 이름만 끈다. */
export function tlsName(rootPath, pid, db) {
  const worktree = createHash("sha256").update(rootPath).digest("hex").slice(0, 8);
  return `pitwall-tls-${worktree}-${pid}-${db}`;
}

/** 필요한 TLS 서버를 띄우고 시험에 넘길 변수를 돌려준다. 띄운 이름은 `started` 에 쌓는다(끌 때 쓴다). */
function startTlsServers(dbs, started) {
  if (dbs.length === 0) return {};
  const out = join(root, "target", "db-tls");
  makeCertificates(out);
  const env = {};
  for (const db of dbs) {
    const name = tlsName(root, process.pid, db);
    started.push(name);
    const server = startTls(db, { name, port: 0, out });
    console.log(`== tls: started ${name} on 127.0.0.1:${server.port} (stopped when this run ends)`);
    Object.assign(env, server.env);
  }
  return env;
}

function runTarget(db, floor, ci, started) {
  const target = TARGETS[db];
  const url = process.env[target.env];
  if (!url) {
    const message = `${db}: ${target.env} is not set — not run`;
    return ci
      ? { ok: false, message: `${message} (the CI job must set it)` }
      : { ok: true, skipped: true, message: `${message}; skipped, not counted as passed` };
  }
  const tls = startTlsServers(tlsToStart(target, process.env, dockerAvailable()), started);
  const env = { ...process.env, ...tls, [target.testEnv]: url };
  const expr = filterExpr(target.filter);
  const pkgs = packages(env);
  console.log(`\n== ${db}: cargo nextest run --release -E '${expr}' (${target.env})`);
  const list = cargo(
    ["nextest", "list", "--release", ...pkgs, "-E", expr, "--message-format", "json"],
    env,
    {
      capture: true,
    },
  );
  if (list.status !== 0) {
    process.stderr.write(list.stderr);
    return { ok: false, message: `${db}: cargo nextest list failed (exit ${list.status})` };
  }
  const selected = selectedTests(JSON.parse(list.stdout));
  for (const name of selected) console.log(`   selected: ${name}`);
  const run = cargo(
    ["nextest", "run", "--release", ...pkgs, "-E", expr, "--no-tests=fail", "--color", "never"],
    env,
    { capture: true },
  );
  process.stdout.write(run.stdout);
  process.stderr.write(run.stderr);
  return judge({
    db,
    filter: target.filter,
    selected,
    run: runCount(run.stderr + run.stdout),
    floor,
    exitCode: run.status,
  });
}

/** 종료 코드: 하나라도 실패하면 1, 하나도 돌지 않았으면(모두 건너뜀) 3 — 「통과」 로 읽히지 않게, 그 밖은 0. */
export function exitCode(results) {
  if (!results.every((r) => r.ok)) return 1;
  if (results.length > 0 && results.every((r) => r.skipped)) return 3;
  return 0;
}

function main(argv) {
  const ci = argv.includes("--ci");
  const asked = argv.filter((a) => a !== "--ci");
  const floors = parseFloors(readFileSync(join(root, "scripts", "db-live.toml"), "utf8"));
  for (const db of Object.keys(TARGETS)) {
    if (!floors.has(db)) throw new Error(`scripts/db-live.toml has no floor for ${db}`);
  }
  const dbs = asked.length > 0 ? asked : Object.keys(TARGETS);
  for (const db of dbs) {
    if (!TARGETS[db]) throw new Error(`unknown db ${db} (${Object.keys(TARGETS).join(", ")})`);
  }
  // 띄운 TLS 서버는 끝날 때 — 실패 · Ctrl-C 를 포함해 — 이름으로 끈다(자기가 띄운 것만).
  const started = [];
  const stopAll = () => {
    for (const name of started.splice(0)) {
      stopTls(name);
      console.log(`== tls: stopped ${name}`);
    }
  };
  process.once("SIGINT", stopAll);
  process.once("SIGTERM", stopAll);
  let results;
  try {
    results = dbs.map((db) => runTarget(db, floors.get(db), ci, started));
  } finally {
    stopAll();
  }
  console.log("\n== db-live summary");
  for (const r of results)
    console.log(`${r.ok ? (r.skipped ? "SKIP" : "PASS") : "FAIL"}  ${r.message}`);
  const skipped = results.filter((r) => r.skipped).length;
  if (skipped > 0)
    console.log(
      `${skipped} of ${results.length} not run — set the URLs to run them (CONTRIBUTING.md 「명령」)`,
    );
  return exitCode(results);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (e) {
    console.error(`db-live: ${e.message}`);
    process.exitCode = e instanceof Interrupted ? 130 : 2;
  }
}
