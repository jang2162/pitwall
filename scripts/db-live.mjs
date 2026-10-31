// 실 DB 시험(live_*)을 DB 마다 따로 돌리고 실행 개수를 하한(scripts/db-live.toml)과 견준다(14-m3-plan.md M3-01, 결정 M3-D16).
//
//   node scripts/db-live.mjs [--ci] [db …]     db: pg · mysql · mariadb · redis(없으면 전부)
//
// CI 의 `db-live` 잡이 DB 마다 한 단계씩, `mise run test-db` 가 전부를 한 번에 부른다. DB 마다:
//   1. URL 환경 변수(PW_TEST_<DB>_URL)가 없으면 로컬은 「건너뜀」 으로 알리고 통과로 세지 않는다. `--ci` 면 실패다.
//   2. `cargo nextest list` 로 필터가 고른 시험을 세고, `cargo nextest run --release --no-tests=fail` 로 돌린다 — 필터가
//      0개를 고르면 nextest 가 실패한다(준비 확인 시험이 있으므로 정상이면 늘 1개 이상).
//   3. 돈 수(nextest 의 Summary 줄)가 고른 수와 같은지, 준비 확인 시험(`live_<db>_ready`)을 뺀 수가 하한 이상인지 본다.
// MariaDB 는 MySQL 시험(live_mysql)을 그 URL 로 한 번 더 돈다 — PW_TEST_MARIADB_URL 을 PW_TEST_MYSQL_URL 로 넘긴다.
// 실 DB 시험을 다는 법은 crates/pw-db-driver/src/live.rs 머리 주석.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));

// DB → (URL 변수, nextest 이름 필터, 시험에 넘길 변수). 새 단계(M3-11 의 ssh_tunnel 등)는 여기와 db-live.toml 에 한 줄씩.
export const TARGETS = {
  pg: { env: "PW_TEST_PG_URL", filter: "live_pg", testEnv: "PW_TEST_PG_URL" },
  mysql: { env: "PW_TEST_MYSQL_URL", filter: "live_mysql", testEnv: "PW_TEST_MYSQL_URL" },
  mariadb: { env: "PW_TEST_MARIADB_URL", filter: "live_mysql", testEnv: "PW_TEST_MYSQL_URL" },
  redis: { env: "PW_TEST_REDIS_URL", filter: "live_redis", testEnv: "PW_TEST_REDIS_URL" },
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

/** 준비 확인 시험인가(이름의 마지막 조각이 `<필터>_ready`). */
export function isReadyTest(name, filter) {
  const last = name.split(/::|\s/).at(-1);
  return last === `${filter}_ready`;
}

/** nextest 의 `Summary [ … ] N test(s) run: …` 줄의 N. 없으면 null. */
export function runCount(output) {
  const matches = [...output.matchAll(/Summary \[[^\]]*\]\s+(\d+) tests? run:/g)];
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

function cargo(args, env, { capture }) {
  const result = spawnSync("cargo", args, {
    cwd: root,
    env,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
  if (result.error) throw result.error;
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

function runTarget(db, floor, ci) {
  const target = TARGETS[db];
  const url = process.env[target.env];
  if (!url) {
    const message = `${db}: ${target.env} is not set — not run`;
    return ci
      ? { ok: false, message: `${message} (the CI job must set it)` }
      : { ok: true, skipped: true, message: `${message}; skipped, not counted as passed` };
  }
  const env = { ...process.env, [target.testEnv]: url };
  const expr = `test(/${target.filter}/)`;
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
  const run = cargo(["nextest", "run", "--release", ...pkgs, "-E", expr, "--no-tests=fail"], env, {
    capture: true,
  });
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
  const results = dbs.map((db) => runTarget(db, floors.get(db), ci));
  console.log("\n== db-live summary");
  for (const r of results)
    console.log(`${r.ok ? (r.skipped ? "SKIP" : "PASS") : "FAIL"}  ${r.message}`);
  const skipped = results.filter((r) => r.skipped).length;
  if (skipped > 0)
    console.log(
      `${skipped} of ${results.length} not run — set the URLs to run them (CONTRIBUTING.md 「명령」)`,
    );
  return results.every((r) => r.ok) ? 0 : 1;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (e) {
    console.error(`db-live: ${e.message}`);
    process.exitCode = 2;
  }
}
