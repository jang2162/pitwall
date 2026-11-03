// 실 언어 서버 시험(live_lsp_*)을 서버마다 따로 돌리고 실행 개수를 하한(scripts/lsp-live.toml 의 [floors])과 견준다
// (15-m4-plan.md M4-01 ⑧ · M4-18 — M3 의 scripts/db-live.mjs 와 같은 판정).
//
//   node scripts/lsp-live.mjs [--ci] [--install] [server …]
//     server: rust_analyzer · typescript_language_server · basedpyright · ruff · jdtls · kotlin_lsp(없으면 전부)
//
// CI 의 `lsp-live` 잡이 서버마다 한 단계씩(`--ci --install`), `mise run test-lsp` 가 전부를 한 번에 부른다. 서버마다:
//   1. 실행 파일 경로: PW_TEST_LSP_<서버>, 없으면 PW_TEST_TOOLS_DIR/bin/<실행 파일>(설치기의 실행 링크 폴더). 둘 다 없고 `--install`
//      이면 Pitwall 의 설치기로 받는다 — `cargo run -q -p pw-tools --example tools_install -- <패키지> <$DATA> <$STATE>`(앱과 같은
//      고정 registry 판 · 고정 해시 · 풀기 검사, `$DATA` · `$STATE` 는 PW_LSP_SERVERS_DIR — 기본 target/lsp-servers/ — 아래). 아니면
//      로컬은 「건너뜀」 으로 알리고 통과로 세지 않는다(모두 건너뛰었으면 종료 코드 3). `--ci` 인데 경로가 없으면 실패다.
//   2. `cargo nextest list` 로 필터가 고른 시험을 세고, `cargo nextest run --no-tests=fail` 로 돌린다 — 필터가 0개를 고르면 실패다.
//   3. 돈 수(nextest 의 Summary 줄)가 고른 수와 같은지, 하한 이상인지 본다.
// 시험을 다는 법은 crates/pw-lsp/src/live.rs 머리 주석.

import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { exitCode, runCount, selectedTests } from "./db-live.mjs";

export { exitCode, runCount, selectedTests };

const root = fileURLToPath(new URL("..", import.meta.url));

// 서버 → (실행 파일 경로 변수, 실행 파일 이름, nextest 이름 필터). 받을 패키지는 lsp-live.toml 의 [install.<서버>]. 새 서버는 여기와
// lsp-live.toml, crates/pw-lsp 의 live_cfg.rs · src/live.rs 에 한 줄씩.
export const TARGETS = {
  rust_analyzer: { env: "PW_TEST_LSP_RUST_ANALYZER", exe: "rust-analyzer" },
  typescript_language_server: {
    env: "PW_TEST_LSP_TYPESCRIPT_LANGUAGE_SERVER",
    exe: "typescript-language-server",
  },
  basedpyright: { env: "PW_TEST_LSP_BASEDPYRIGHT", exe: "basedpyright-langserver" },
  ruff: { env: "PW_TEST_LSP_RUFF", exe: "ruff" },
  jdtls: { env: "PW_TEST_LSP_JDTLS", exe: "jdtls" },
  kotlin_lsp: { env: "PW_TEST_LSP_KOTLIN_LSP", exe: "kotlin-lsp" },
};

/**
 * lsp-live.toml 이 쓰는 TOML 의 부분집합: `[절]` · `[절.하위]` 머리, `키 = 정수 | "글" | ["글", …]`(배열은 여러 줄 · 끝 쉼표 가능), `#` 주석. 키는 글자 ·
 * 숫자 · `_` · `-`. 모르는 꼴 · 같은 키 두 번은 오류다. `{ 절: { 키: 값 } }`(하위 절은 중첩)을 돌려준다.
 */
export function parseToml(text, file = "lsp-live.toml") {
  const out = {};
  let table = out;
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];
    let line = stripComment(raw).trim();
    if (line === "") continue;
    const where = `${file} line ${i + 1}`;
    // 여러 줄 배열: `키 = [` 로 열리고 `]` 로 끝나는 줄까지 이어 붙인다.
    if (/=\s*\[[^\]]*$/.test(line)) {
      while (!line.endsWith("]")) {
        i++;
        if (i >= lines.length) throw new Error(`${where}: array is not closed`);
        line += ` ${stripComment(lines[i]).trim()}`;
      }
    }
    const head = /^\[([A-Za-z0-9_.-]+)\]$/.exec(line);
    if (head) {
      table = out;
      for (const part of head[1].split(".")) {
        table[part] ??= {};
        table = table[part];
      }
      continue;
    }
    const pair = /^([A-Za-z0-9_-]+)\s*=\s*(.+)$/.exec(line);
    if (!pair) throw new Error(`${where}: expected \`key = value\` or \`[table]\`, got ${raw}`);
    const [, key, value] = pair;
    if (Object.hasOwn(table, key)) throw new Error(`${where}: ${key} is listed twice`);
    table[key] = parseValue(value, where);
  }
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
    const items = inner.split(",").map((item) => item.trim());
    if (items.at(-1) === "") items.pop();
    return items.map((s) => {
      if (!/^"[^"]*"$/.test(s)) throw new Error(`${where}: array items must be strings, got ${s}`);
      return s.slice(1, -1);
    });
  }
  throw new Error(`${where}: unsupported value ${value}`);
}

/** 설정 전체를 읽고 서버마다 하한 · 기대 능력 표 · 받을 패키지가 있는지 본다. */
export function readConfig(text) {
  const config = parseToml(text);
  for (const name of Object.keys(TARGETS)) {
    if (!Number.isInteger(config.floors?.[name]))
      throw new Error(`scripts/lsp-live.toml has no [floors] ${name}`);
    if (!Array.isArray(config.capabilities?.[name]))
      throw new Error(`scripts/lsp-live.toml has no [capabilities] ${name}`);
    const install = config.install?.[name];
    if (typeof install?.package !== "string" || typeof install?.bin !== "string")
      throw new Error(`scripts/lsp-live.toml has no [install.${name}] package and bin`);
  }
  for (const name of Object.keys(config.floors ?? {})) {
    if (!TARGETS[name])
      throw new Error(`scripts/lsp-live.toml: unknown server ${name} in [floors]`);
  }
  return config;
}

/** 그 서버의 nextest 필터 이름 조각(`live_lsp_<서버>`). */
export function filterName(server) {
  return `live_lsp_${server}`;
}

/**
 * 그 서버의 nextest 필터 식. 실 서버 시험은 함수 이름이 `live_lsp_<서버>_` 로 시작한다 — 경로 조각(`::`)의 처음에만 맞춘다(이름
 * 가운데에 든 단위 시험은 고르지 않는다).
 */
export function filterExpr(filter) {
  return `test(/(^|::)${filter}_/)`;
}

/**
 * 기대 능력(`[capabilities]`)을 판정하는 시험의 이름 — `crates/pw-lsp/tests/live_lsp.rs`. 판정은 정적 능력만 본다: 동적 등록으로만
 * 알리는 서버(M4-18 의 JVM 서버 일부)를 넣을 때는 그 시험이 `supports()` 를 쓰도록 바꾼다.
 */
export function capabilityTest(server) {
  return `live_lsp_${server}_initialize_and_shutdown`;
}

/** 한 서버의 판정. */
export function judge({ server, selected, run, floor, exitCode: code, required }) {
  const line = `${server}: ${run ?? "?"} run (selected ${selected.length}), floor ${floor}`;
  // 0개를 먼저 본다 — nextest 는 `--no-tests=fail` 로 그때도 실패하므로 종료 코드보다 까닭이 분명하다.
  if (selected.length === 0)
    return { ok: false, message: `${line} — the filter selected no tests` };
  // 기대 능력 판정은 그 시험 안에 있다 — 지우거나 이름을 바꾸면 하한을 넘어도 판정이 조용히 사라지므로 있는지부터 본다(리뷰 M3).
  if (required && !selected.some((name) => name.split(/::|\s/).at(-1) === required))
    return {
      ok: false,
      message: `${line} — the capability check ${required} is not among the selected tests`,
    };
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

function serversDir() {
  return process.env.PW_LSP_SERVERS_DIR || join(root, "target", "lsp-servers");
}

/** 설치기 예제의 cargo 인자(`$DATA` · `$STATE` 는 `dir` 아래). */
export function installArgs(pkg, dir) {
  return [
    "run",
    "-q",
    "-p",
    "pw-tools",
    "--example",
    "tools_install",
    "--",
    pkg,
    join(dir, "data"),
    join(dir, "state"),
  ];
}

/** 설치 결과 JSON(`{name, version, checksum, bins}`)에서 실행 파일 경로. 없으면 오류. */
export function installedProgram(stdout, bin) {
  const out = JSON.parse(stdout);
  const program = out.bins?.[bin];
  if (typeof program !== "string")
    throw new Error(`the installer gave no ${bin} for ${out.name}: ${stdout.trim()}`);
  if (out.checksum !== "pinned")
    throw new Error(
      `${out.name} ${out.version} was installed without a pinned hash (${out.checksum})`,
    );
  return { program, name: out.name, version: out.version };
}

/** 폴더의 바이트(링크는 따라가지 않는다). */
function folderBytes(dir) {
  let total = 0;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) total += folderBytes(path);
    else total += lstatSync(path).size;
  }
  return total;
}

/** Pitwall 의 설치기로 그 서버를 받는다(이미 그 판이면 받지 않는다). 걸린 시간 · 설치 폴더 크기를 찍는다(CI 단계 보고). */
function install(server, spec) {
  const dir = serversDir();
  const started = Date.now();
  console.log(`== ${server}: tools_install ${spec.package} (${dir})`);
  const result = spawnSync("cargo", installArgs(spec.package, dir), {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    stdio: ["ignore", "pipe", "inherit"],
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`tools_install ${spec.package} failed (exit ${result.status})`);
  const { program, name, version } = installedProgram(result.stdout, spec.bin);
  const folder = join(dir, "data", "tools", name);
  const mb = existsSync(folder) ? (folderBytes(folder) / 1e6).toFixed(1) : "?";
  console.log(
    `   ${name} ${version}: ${((Date.now() - started) / 1000).toFixed(1)} s, install folder ${mb} MB — ${program}`,
  );
  return program;
}

/** 시험 프로세스가 쓸 실행 파일 경로: 변수, 없으면 PW_TEST_TOOLS_DIR/bin/<실행 파일>. */
function givenProgram(target) {
  if (process.env[target.env]) return process.env[target.env];
  const tools = process.env.PW_TEST_TOOLS_DIR;
  if (tools && existsSync(join(tools, "bin", target.exe))) return join(tools, "bin", target.exe);
  return null;
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

async function runTarget(server, config, { ci, install: wantInstall }) {
  const target = TARGETS[server];
  let program = givenProgram(target);
  if (!program && wantInstall) program = install(server, config.install[server]);
  if (!program) {
    const message = `${server}: ${target.env} is not set — not run`;
    return ci
      ? { ok: false, message: `${message} (the CI job must set it or pass --install)` }
      : {
          ok: true,
          skipped: true,
          message: `${message}; skipped, not counted as passed (--install fetches it with the Pitwall installer)`,
        };
  }
  const env = { ...process.env, [target.env]: program };
  const expr = filterExpr(filterName(server));
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
    required: capabilityTest(server),
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
