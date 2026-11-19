// 실 디버그 어댑터 시험(live_dap_*)을 어댑터마다 따로 돌리고 실행 개수를 하한(scripts/dap-live.toml 의 [floors])과 견준다
// (17-m6-plan.md M6-19 — scripts/lsp-live.mjs 와 같은 판정).
//
//   node scripts/dap-live.mjs [--ci] [--install] [adapter …]
//     adapter: codelldb · js_debug · debugpy · java_debug(없으면 전부)
//
// CI 의 `dap-live` 잡이 어댑터마다 한 단계씩(`--ci --install`), `mise run test-dap` 이 전부를 한 번에 부른다. 어댑터마다:
//   1. 경로: PW_TEST_DAP_<어댑터>(실행 파일 · java-debug 는 jar). 없고 `--install` 이면 Pitwall 의 설치기로 받는다 —
//      `cargo run -q -p pw-tools --example tools_install -- <패키지> <$DATA> <$STATE>`(앱과 같은 고정 판 · 해시 · 풀기 검사, 결정 D71 의
//      java-debug 패치까지). `$DATA` · `$STATE` 는 PW_DAP_ADAPTERS_DIR(기본 target/dap-adapters/) 아래. 아니면 로컬은 「건너뜀」 으로
//      알리고 통과로 세지 않는다(모두 건너뛰었으면 종료 코드 3). `--ci` 인데 경로가 없으면 실패다.
//      java-debug 는 jdtls(PW_TEST_LSP_JDTLS — `--install` 이면 같은 설치기로)와 상류 그대로의 jar(PW_TEST_DAP_JAVA_DEBUG_UPSTREAM —
//      없으면 scripts/java-debug-patch.mjs 가 고정 해시의 vsix 에서 꺼낸다)도 준다. jdtls · 대상 프로그램은 PATH 의 JDK 25 로 돈다.
//   2. `cargo nextest list` 로 필터가 고른 시험을 세고, `cargo nextest run --no-tests=fail` 로 돌린다 — 필터가 0개를 고르면 실패다.
//   3. 돈 수(nextest 의 Summary 줄)가 고른 수와 같은지, 하한 이상인지 본다.
// 시험은 crates/pw-debug/tests/live_dap.rs, 표지는 crates/pw-debug/live_cfg.rs.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { exitCode, runCount, selectedTests } from "./db-live.mjs";
import { installArgs, judge, parseToml } from "./lsp-live.mjs";

export { exitCode, runCount, selectedTests };

const root = fileURLToPath(new URL("..", import.meta.url));

// 어댑터 → 시험에 줄 변수. 받을 패키지는 dap-live.toml 의 [install.<어댑터>]. 새 어댑터는 여기와 dap-live.toml,
// crates/pw-debug 의 live_cfg.rs · tests/live_dap.rs 에 한 줄씩.
export const TARGETS = {
  codelldb: { env: "PW_TEST_DAP_CODELLDB" },
  js_debug: { env: "PW_TEST_DAP_JS_DEBUG" },
  debugpy: { env: "PW_TEST_DAP_DEBUGPY" },
  java_debug: { env: "PW_TEST_DAP_JAVA_DEBUG" },
};

/** 설정을 읽고 어댑터마다 하한 · 받을 패키지가 있는지 본다. */
export function readConfig(text) {
  const config = parseToml(text, "dap-live.toml");
  for (const name of Object.keys(TARGETS)) {
    if (!Number.isInteger(config.floors?.[name]) || config.floors[name] < 1)
      throw new Error(`scripts/dap-live.toml has no positive [floors] ${name}`);
    const install = config.install?.[name];
    if (typeof install?.package !== "string")
      throw new Error(`scripts/dap-live.toml has no [install.${name}] package`);
    if ((typeof install.bin === "string") === (typeof install.share === "string"))
      throw new Error(`scripts/dap-live.toml [install.${name}] needs exactly one of bin and share`);
  }
  for (const name of Object.keys(config.floors ?? {})) {
    if (!TARGETS[name])
      throw new Error(`scripts/dap-live.toml: unknown adapter ${name} in [floors]`);
  }
  return config;
}

/** 그 어댑터의 nextest 필터 식(함수 이름이 `live_dap_<어댑터>_` 로 시작 — 이름 조각의 처음에만 맞춘다). */
export function filterExpr(adapter) {
  return `test(/(^|::)live_dap_${adapter}_/)`;
}

/**
 * 설치 결과 JSON(`{name, version, checksum, bins, shares}`)에서 `spec` 이 가리키는 경로. 고정 해시 없이 설치됐으면 오류다
 * (기본 번들 — java-debug 는 패치 결과까지 고정 해시로 대조한 것이다).
 */
export function installedPath(stdout, spec) {
  const out = JSON.parse(stdout);
  if (out.checksum !== "pinned")
    throw new Error(
      `${out.name} ${out.version} was installed without a pinned hash (${out.checksum})`,
    );
  const [kind, key] = spec.bin ? ["bins", spec.bin] : ["shares", spec.share];
  const path = out[kind]?.[key];
  if (typeof path !== "string")
    throw new Error(`the installer gave no ${key} for ${out.name}: ${stdout.trim()}`);
  return path;
}

function adaptersDir() {
  return process.env.PW_DAP_ADAPTERS_DIR || join(root, "target", "dap-adapters");
}

/** Pitwall 의 설치기로 받는다(이미 그 판이면 받지 않는다). */
function install(spec) {
  const dir = adaptersDir();
  const started = Date.now();
  console.log(`== tools_install ${spec.package} (${dir})`);
  const result = spawnSync("cargo", installArgs(spec.package, dir), {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    stdio: ["ignore", "pipe", "inherit"],
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`tools_install ${spec.package} failed (exit ${result.status})`);
  const path = installedPath(result.stdout, spec);
  console.log(`   ${spec.package}: ${((Date.now() - started) / 1000).toFixed(1)} s — ${path}`);
  // 설치 결과의 경로는 저장소 기준 상대 경로일 수 있다(PW_DAP_ADAPTERS_DIR 이 상대 경로) — 시험은 다른 작업 폴더에서 돈다.
  return resolve(root, path);
}

/** 사람이 끊었다(Ctrl-C). */
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

/** java-debug 가 함께 쓰는 것: jdtls 와 상류 그대로의 jar. */
async function javaExtras(env, wantInstall) {
  if (!env.PW_TEST_LSP_JDTLS) {
    if (!wantInstall)
      throw new Error("PW_TEST_LSP_JDTLS is not set (java-debug runs inside jdtls)");
    env.PW_TEST_LSP_JDTLS = install({ package: "jdtls", bin: "jdtls" });
  }
  if (!env.PW_TEST_DAP_JAVA_DEBUG_UPSTREAM) {
    const { upstreamJars } = await import("./java-debug-patch.mjs");
    env.PW_TEST_DAP_JAVA_DEBUG_UPSTREAM = (await upstreamJars()).jar;
  }
}

async function runTarget(adapter, config, { ci, install: wantInstall }) {
  const target = TARGETS[adapter];
  const env = { ...process.env };
  if (!env[target.env] && wantInstall) env[target.env] = install(config.install[adapter]);
  if (!env[target.env]) {
    const message = `${adapter}: ${target.env} is not set — not run`;
    return ci
      ? { ok: false, message: `${message} (the CI job must set it or pass --install)` }
      : {
          ok: true,
          skipped: true,
          message: `${message}; skipped, not counted as passed (--install fetches it with the Pitwall installer)`,
        };
  }
  if (adapter === "java_debug") await javaExtras(env, wantInstall);
  const expr = filterExpr(adapter);
  console.log(
    `\n== ${adapter}: cargo nextest run -p pw-debug -E '${expr}' (${target.env}=${env[target.env]})`,
  );
  const list = cargo(
    ["nextest", "list", "-p", "pw-debug", "-E", expr, "--message-format", "json"],
    env,
    { capture: true },
  );
  if (list.status !== 0) {
    process.stderr.write(list.stderr);
    return { ok: false, message: `${adapter}: cargo nextest list failed (exit ${list.status})` };
  }
  const selected = selectedTests(JSON.parse(list.stdout));
  for (const name of selected) console.log(`   selected: ${name}`);
  const run = cargo(
    ["nextest", "run", "-p", "pw-debug", "-E", expr, "--no-tests=fail", "--color", "never"],
    env,
    { capture: true },
  );
  process.stdout.write(run.stdout);
  process.stderr.write(run.stderr);
  return judge({
    server: adapter,
    selected,
    run: runCount(run.stderr + run.stdout),
    floor: config.floors[adapter],
    exitCode: run.status,
    required: null,
  });
}

async function main(argv) {
  const ci = argv.includes("--ci");
  const install = argv.includes("--install");
  const asked = argv.filter((a) => !a.startsWith("--"));
  const config = readConfig(readFileSync(join(root, "scripts", "dap-live.toml"), "utf8"));
  const adapters = asked.length > 0 ? asked : Object.keys(TARGETS);
  for (const adapter of adapters) {
    if (!TARGETS[adapter])
      throw new Error(`unknown adapter ${adapter} (${Object.keys(TARGETS).join(", ")})`);
  }
  const results = [];
  for (const adapter of adapters) {
    try {
      results.push(await runTarget(adapter, config, { ci, install }));
    } catch (e) {
      if (e instanceof Interrupted) throw e;
      results.push({ ok: false, message: `${adapter}: ${e.message}` });
    }
  }
  console.log("\n== dap-live summary");
  for (const r of results)
    console.log(`${r.ok ? (r.skipped ? "SKIP" : "PASS") : "FAIL"}  ${r.message}`);
  const skipped = results.filter((r) => r.skipped).length;
  if (skipped > 0)
    console.log(
      `${skipped} of ${results.length} not run — set the adapter paths or pass --install (CONTRIBUTING.md 「명령」)`,
    );
  return exitCode(results);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    process.exitCode = await main(process.argv.slice(2));
  } catch (e) {
    console.error(`dap-live: ${e.message}`);
    process.exitCode = e instanceof Interrupted ? 130 : 2;
  }
}
