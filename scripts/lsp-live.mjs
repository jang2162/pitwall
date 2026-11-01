// 실 언어 서버 시험(live_lsp_*)을 서버마다 따로 돌리고 실행 개수를 하한(scripts/lsp-live.toml 의 [floors])과 견준다
// (15-m4-plan.md M4-01 ⑧ — M3 의 scripts/db-live.mjs 와 같은 판정).
//
//   node scripts/lsp-live.mjs [--ci] [--install] [server …]     server: rust_analyzer · typescript_language_server(없으면 전부)
//
// CI 의 `lsp-live` 잡이 서버마다 한 단계씩(`--ci --install`), `mise run test-lsp` 가 전부를 한 번에 부른다. 서버마다:
//   1. 실행 파일 경로(PW_TEST_LSP_<서버>)가 없으면: `--install` 이면 scripts/lsp-live.toml 의 [download.<서버>] 대로 판 고정 · 해시
//      확인으로 받아(target/lsp-servers/ — PW_LSP_SERVERS_DIR 로 바꾼다) 그 경로를 준다. 아니면 로컬은 「건너뜀」 으로 알리고 통과로
//      세지 않는다(모두 건너뛰었으면 종료 코드 3). `--ci` 인데 경로가 없으면 실패다.
//   2. `cargo nextest list` 로 필터가 고른 시험을 세고, `cargo nextest run --no-tests=fail` 로 돌린다 — 필터가 0개를 고르면 실패다.
//   3. 돈 수(nextest 의 Summary 줄)가 고른 수와 같은지, 하한 이상인지 본다.
// 받기는 M4-04 의 설치기가 대신하고(그때 [download] 와 install 을 걷는다), M4-18 이 서버를 넓힌다. 시험을 다는 법은
// crates/pw-lsp/src/live.rs 머리 주석.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { gunzipSync } from "node:zlib";

import { exitCode, runCount, selectedTests } from "./db-live.mjs";

export { exitCode, runCount, selectedTests };

const root = fileURLToPath(new URL("..", import.meta.url));

// 서버 → (실행 파일 경로 변수, nextest 이름 필터, 받기). 새 서버는 여기와 lsp-live.toml, crates/pw-lsp 의 live_cfg.rs · src/live.rs 에 한 줄씩.
export const TARGETS = {
  rust_analyzer: {
    env: "PW_TEST_LSP_RUST_ANALYZER",
    filter: "live_lsp_rust_analyzer",
    install: installRustAnalyzer,
  },
  typescript_language_server: {
    env: "PW_TEST_LSP_TYPESCRIPT_LANGUAGE_SERVER",
    filter: "live_lsp_typescript_language_server",
    install: installTypescriptLanguageServer,
  },
};

/**
 * lsp-live.toml 이 쓰는 TOML 의 부분집합: `[절]` · `[절.하위]` 머리, `키 = 정수 | "글" | ["글", …]`(한 줄), `#` 주석. 키는 글자 ·
 * 숫자 · `_` · `-`. 모르는 꼴 · 같은 키 두 번은 오류다. `{ 절: { 키: 값 } }`(하위 절은 중첩)을 돌려준다.
 */
export function parseToml(text, file = "lsp-live.toml") {
  const out = {};
  let table = out;
  text.split("\n").forEach((raw, i) => {
    const line = stripComment(raw).trim();
    if (line === "") return;
    const where = `${file} line ${i + 1}`;
    const head = /^\[([A-Za-z0-9_.-]+)\]$/.exec(line);
    if (head) {
      table = out;
      for (const part of head[1].split(".")) {
        table[part] ??= {};
        table = table[part];
      }
      return;
    }
    const pair = /^([A-Za-z0-9_-]+)\s*=\s*(.+)$/.exec(line);
    if (!pair) throw new Error(`${where}: expected \`key = value\` or \`[table]\`, got ${raw}`);
    const [, key, value] = pair;
    if (Object.hasOwn(table, key)) throw new Error(`${where}: ${key} is listed twice`);
    table[key] = parseValue(value, where);
  });
  return out;
}

function stripComment(line) {
  let quoted = false;
  for (let i = 0; i < line.length; i++) {
    if (line[i] === '"') quoted = !quoted;
    else if (line[i] === "#" && !quoted) return line.slice(0, i);
  }
  return line;
}

function parseValue(value, where) {
  if (/^\d+$/.test(value)) return Number(value);
  if (/^"[^"]*"$/.test(value)) return value.slice(1, -1);
  if (/^\[.*\]$/.test(value)) {
    const inner = value.slice(1, -1).trim();
    if (inner === "") return [];
    return inner.split(",").map((item) => {
      const s = item.trim();
      if (!/^"[^"]*"$/.test(s)) throw new Error(`${where}: array items must be strings, got ${s}`);
      return s.slice(1, -1);
    });
  }
  throw new Error(`${where}: unsupported value ${value}`);
}

/** 설정 전체를 읽고 서버마다 하한 · 기대 능력 표 · 받기 절이 있는지 본다. */
export function readConfig(text) {
  const config = parseToml(text);
  for (const name of Object.keys(TARGETS)) {
    if (!Number.isInteger(config.floors?.[name]))
      throw new Error(`scripts/lsp-live.toml has no [floors] ${name}`);
    if (!Array.isArray(config.capabilities?.[name]))
      throw new Error(`scripts/lsp-live.toml has no [capabilities] ${name}`);
    if (!config.download?.[name])
      throw new Error(`scripts/lsp-live.toml has no [download.${name}]`);
  }
  for (const name of Object.keys(config.floors ?? {})) {
    if (!TARGETS[name])
      throw new Error(`scripts/lsp-live.toml: unknown server ${name} in [floors]`);
  }
  return config;
}

/**
 * 그 서버의 nextest 필터 식. 실 서버 시험은 함수 이름이 `live_lsp_<서버>_` 로 시작한다 — 경로 조각(`::`)의 처음에만 맞춘다(이름
 * 가운데에 든 단위 시험은 고르지 않는다).
 */
export function filterExpr(filter) {
  return `test(/(^|::)${filter}_/)`;
}

/** 한 서버의 판정. */
export function judge({ server, selected, run, floor, exitCode: code }) {
  const line = `${server}: ${run ?? "?"} run (selected ${selected.length}), floor ${floor}`;
  // 0개를 먼저 본다 — nextest 는 `--no-tests=fail` 로 그때도 실패하므로 종료 코드보다 까닭이 분명하다.
  if (selected.length === 0)
    return { ok: false, message: `${line} — the filter selected no tests` };
  if (code !== 0) return { ok: false, message: `${line} — nextest failed (exit ${code})` };
  if (run === null)
    return {
      ok: false,
      message: `${line} — could not read how many tests nextest ran (no Summary line)`,
    };
  if (run !== selected.length)
    return {
      ok: false,
      message: `${line} — nextest ran ${run} but the filter selected ${selected.length}`,
    };
  if (run < floor)
    return { ok: false, message: `${line} — below the floor in scripts/lsp-live.toml` };
  return { ok: true, message: `${line} — ok` };
}

/** rust-analyzer 릴리스 자산의 대상 이름. 받을 수 없는 기기면 null. */
export function rustTarget(platform = process.platform, arch = process.arch) {
  const cpu = { arm64: "aarch64", x64: "x86_64" }[arch];
  const os = { darwin: "apple-darwin", linux: "unknown-linux-gnu" }[platform];
  return cpu && os ? `${cpu}-${os}` : null;
}

/** pnpm-lock.yaml 에서 `<이름>@<판>` 의 integrity. 없으면 null. */
export function lockIntegrity(lock, name, version) {
  const escaped = `${name}@${version}`.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = new RegExp(
    `^ {2}'?${escaped}'?:\\s*\\n\\s+resolution: \\{integrity: ([^,}]+)`,
    "m",
  ).exec(lock);
  return match ? match[1].trim() : null;
}

function serversDir() {
  return process.env.PW_LSP_SERVERS_DIR || join(root, "target", "lsp-servers");
}

async function download(url) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) throw new Error(`GET ${url}: ${response.status}`);
  if (!response.url.startsWith("https://"))
    throw new Error(`GET ${url}: redirected off HTTPS to ${response.url}`);
  return Buffer.from(await response.arrayBuffer());
}

async function installRustAnalyzer(pins) {
  const target = rustTarget();
  if (!target) throw new Error(`no rust-analyzer build for ${process.platform}/${process.arch}`);
  const sha256 = pins[target];
  if (!sha256) throw new Error(`scripts/lsp-live.toml [download.rust_analyzer] has no ${target}`);
  const dir = join(serversDir(), "rust-analyzer", pins.version);
  const exe = join(dir, "rust-analyzer");
  if (existsSync(exe)) return exe;
  const url = `https://github.com/rust-lang/rust-analyzer/releases/download/${pins.version}/rust-analyzer-${target}.gz`;
  console.log(`== rust_analyzer: GET ${url}`);
  const gz = await download(url);
  const got = createHash("sha256").update(gz).digest("hex");
  if (got !== sha256)
    throw new Error(`rust-analyzer ${pins.version} ${target}: sha256 ${got}, pinned ${sha256}`);
  mkdirSync(dir, { recursive: true });
  // 다 쓴 뒤 이름을 바꾼다 — 끊긴 받기가 「있음」 으로 남지 않게.
  writeFileSync(`${exe}.part`, gunzipSync(gz));
  chmodSync(`${exe}.part`, 0o755);
  renameSync(`${exe}.part`, exe);
  return exe;
}

function installTypescriptLanguageServer(pins) {
  const dir = join(
    serversDir(),
    "typescript-language-server",
    `${pins.version}-ts${pins.typescript}`,
  );
  const exe = join(dir, "node_modules", ".bin", "typescript-language-server");
  const lockFile = join(dir, "pnpm-lock.yaml");
  if (!existsSync(exe) || !existsSync(lockFile)) {
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "package.json"), '{ "private": true }\n');
    const packages = [
      `typescript-language-server@${pins.version}`,
      `typescript@${pins.typescript}`,
    ];
    console.log(`== typescript_language_server: pnpm add ${packages.join(" ")} (in ${dir})`);
    // 저장소의 pnpm 작업 공간에 섞이지 않게(--ignore-workspace), 설치가 패키지 코드를 돌리지 않게(--ignore-scripts).
    const result = spawnSync(
      "pnpm",
      ["add", "--ignore-workspace", "--ignore-scripts", "--dir", dir, ...packages],
      { encoding: "utf8", stdio: ["ignore", "inherit", "inherit"] },
    );
    if (result.error) throw result.error;
    if (result.status !== 0) throw new Error(`pnpm add failed (exit ${result.status})`);
  }
  const lock = readFileSync(lockFile, "utf8");
  for (const [name, version, pinned] of [
    ["typescript-language-server", pins.version, pins.integrity],
    ["typescript", pins.typescript, pins["typescript-integrity"]],
  ]) {
    const got = lockIntegrity(lock, name, version);
    if (got !== pinned) throw new Error(`${name}@${version}: integrity ${got}, pinned ${pinned}`);
  }
  if (!existsSync(exe)) throw new Error(`${exe} is missing after the install`);
  return exe;
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

async function runTarget(server, config, { ci, install }) {
  const target = TARGETS[server];
  let program = process.env[target.env];
  if (!program && install) program = await target.install(config.download[server]);
  if (!program) {
    const message = `${server}: ${target.env} is not set — not run`;
    return ci
      ? { ok: false, message: `${message} (the CI job must set it or pass --install)` }
      : {
          ok: true,
          skipped: true,
          message: `${message}; skipped, not counted as passed (--install fetches the pinned server)`,
        };
  }
  const env = { ...process.env, [target.env]: program };
  const expr = filterExpr(target.filter);
  console.log(
    `\n== ${server}: cargo nextest run -p pw-lsp -E '${expr}' (${target.env}=${program})`,
  );
  const list = cargo(
    ["nextest", "list", "-p", "pw-lsp", "-E", expr, "--message-format", "json"],
    env,
    { capture: true },
  );
  if (list.status !== 0) {
    process.stderr.write(list.stderr);
    return { ok: false, message: `${server}: cargo nextest list failed (exit ${list.status})` };
  }
  const selected = selectedTests(JSON.parse(list.stdout));
  for (const name of selected) console.log(`   selected: ${name}`);
  const run = cargo(
    ["nextest", "run", "-p", "pw-lsp", "-E", expr, "--no-tests=fail", "--color", "never"],
    env,
    { capture: true },
  );
  process.stdout.write(run.stdout);
  process.stderr.write(run.stderr);
  return judge({
    server,
    selected,
    run: runCount(run.stderr + run.stdout),
    floor: config.floors[server],
    exitCode: run.status,
  });
}

async function main(argv) {
  const ci = argv.includes("--ci");
  const install = argv.includes("--install");
  const asked = argv.filter((a) => !a.startsWith("--"));
  const config = readConfig(readFileSync(join(root, "scripts", "lsp-live.toml"), "utf8"));
  const servers = asked.length > 0 ? asked : Object.keys(TARGETS);
  for (const server of servers) {
    if (!TARGETS[server])
      throw new Error(`unknown server ${server} (${Object.keys(TARGETS).join(", ")})`);
  }
  const results = [];
  for (const server of servers) {
    try {
      results.push(await runTarget(server, config, { ci, install }));
    } catch (e) {
      if (e instanceof Interrupted) throw e;
      results.push({ ok: false, message: `${server}: ${e.message}` });
    }
  }
  console.log("\n== lsp-live summary");
  for (const r of results)
    console.log(`${r.ok ? (r.skipped ? "SKIP" : "PASS") : "FAIL"}  ${r.message}`);
  const skipped = results.filter((r) => r.skipped).length;
  if (skipped > 0)
    console.log(
      `${skipped} of ${results.length} not run — set the server paths or pass --install (CONTRIBUTING.md 「명령」)`,
    );
  return exitCode(results);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    process.exitCode = await main(process.argv.slice(2));
  } catch (e) {
    console.error(`lsp-live: ${e.message}`);
    process.exitCode = e instanceof Interrupted ? 130 : 2;
  }
}
