// 내장 언어 확장(extensions/lang-<id>/)의 tree-sitter 문법 WASM · 강조 쿼리를 출처표(scripts/grammars.toml)대로 만든다.
//
//   node scripts/grammars.mjs build [id…]   고정 커밋의 소스를 target/grammars/src/ 에 받아 WASM · 쿼리를 확장 폴더에 쓰고
//                                          출처표의 해시 줄을 고친다(`mise run grammars`). id 를 주면 그 문법만
//   node scripts/grammars.mjs verify [id…]  같은 입력으로 target/grammars/verify/ 에 다시 만들어 출처표의 해시와 견준다
//                                          (재현성 확인 — 확장 폴더는 건드리지 않는다)
//   node scripts/grammars.mjs check         커밋된 WASM · 쿼리의 해시가 출처표와 맞는지, 표에 없는 WASM 이 없는지, 고지가
//                                          licenses/vendored.md 에 있는지 본다(`mise run lint`, 네트워크 · 도구 없이 돈다)
//
// build · verify 는 PATH 의 `tree-sitter`(mise.toml 이 고정한다)와 git, generate 가 필요한 문법은 node 를 쓴다. tree-sitter CLI 는
// WASM 을 만들 때 wasi-sdk · binaryen 을 자기 캐시에 받는다(버전은 CLI 가 고정한다). 순서와 규칙은 CONTRIBUTING.md 「tree-sitter 문법」 절.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const TABLE = join(root, "scripts", "grammars.toml");
const VENDORED = join(root, "licenses", "vendored.md");
const WORK = join(root, "target", "grammars");

/**
 * 출처표를 읽는다. 이 파일에 쓰는 TOML 만 받는 좁은 파서다: 주석, `[[grammar]]`, `키 = "문자열"` · `true` · `false` ·
 * 문자열 배열(여러 줄 가능). 문자열에 이스케이프는 쓰지 않는다.
 */
function readTable() {
  const grammars = [];
  let current = null;
  let pending = null;
  for (const [i, raw] of readFileSync(TABLE, "utf8").split("\n").entries()) {
    const line = raw.replace(/\s+#.*$/, "").trim();
    const where = `scripts/grammars.toml:${i + 1}`;
    if (pending) {
      pending.text += ` ${line}`;
      if (line.endsWith("]")) {
        current[pending.key] = parseValue(pending.text, where);
        pending = null;
      }
      continue;
    }
    if (line === "" || line.startsWith("#")) continue;
    if (line === "[[grammar]]") {
      current = {};
      grammars.push(current);
      continue;
    }
    const m = /^([a-z_0-9]+)\s*=\s*(.*)$/.exec(line);
    if (!m || !current) throw new Error(`${where}: cannot read \`${raw}\``);
    if (m[2].startsWith("[") && !m[2].endsWith("]")) {
      pending = { key: m[1], text: m[2] };
    } else {
      current[m[1]] = parseValue(m[2], where);
    }
  }
  for (const g of grammars) {
    for (const key of ["id", "extension", "repo", "tag", "commit", "license"]) {
      if (typeof g[key] !== "string" || g[key] === "") {
        throw new Error(`scripts/grammars.toml: grammar ${g.id ?? "?"} needs \`${key}\``);
      }
    }
    if (!/^[0-9a-f]{40}$/.test(g.commit)) {
      throw new Error(`scripts/grammars.toml: grammar ${g.id}: commit must be 40 hex digits`);
    }
    g.path ??= ".";
    g.generate ??= false;
    g.highlights ??= [];
  }
  return grammars;
}

function parseValue(text, where) {
  if (text === "true" || text === "false") return text === "true";
  if (text.startsWith('"')) {
    if (!/^"[^"\\]*"$/.test(text)) throw new Error(`${where}: unsupported string ${text}`);
    return text.slice(1, -1);
  }
  if (text.startsWith("[") && text.endsWith("]")) {
    const inner = text.slice(1, -1).trim().replace(/,$/, "");
    return inner === "" ? [] : inner.split(",").map((s) => parseValue(s.trim(), where));
  }
  throw new Error(`${where}: unsupported value ${text}`);
}

/** 해시 줄만 고쳐 쓴다(주석 · 순서 유지). */
function writeHashes(hashes) {
  let id = null;
  const lines = readFileSync(TABLE, "utf8")
    .split("\n")
    .map((line) => {
      if (line.trim() === "[[grammar]]") id = null;
      const idMatch = /^id = "([^"]*)"/.exec(line);
      if (idMatch) id = idMatch[1];
      const key = /^(wasm_sha256|highlights_sha256) = /.exec(line)?.[1];
      if (key && id && hashes.has(id)) return `${key} = "${hashes.get(id)[key]}"`;
      return line;
    });
  writeFileSync(TABLE, lines.join("\n"));
}

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function run(cmd, args, cwd) {
  const r = spawnSync(cmd, args, { cwd, stdio: ["ignore", "inherit", "inherit"] });
  if (r.error) throw new Error(`${cmd}: ${r.error.message}`);
  if (r.status !== 0) throw new Error(`${cmd} ${args.join(" ")} failed (${r.status}) in ${cwd}`);
}

/** 고정 커밋의 체크아웃. 다 받은 뒤에 표식(`.pitwall-commit`)을 쓰고, 표식이 맞으면 다시 받지 않는다. */
function checkout(grammar) {
  const name = grammar.repo.split("/").pop();
  const dir = join(WORK, "src", `${name}-${grammar.commit.slice(0, 12)}`);
  const marker = join(dir, ".pitwall-commit");
  if (existsSync(marker) && readFileSync(marker, "utf8") === grammar.commit) return dir;
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  run("git", ["init", "-q"], dir);
  run("git", ["fetch", "-q", "--depth", "1", grammar.repo, grammar.commit], dir);
  run("git", ["checkout", "-q", "FETCH_HEAD"], dir);
  writeFileSync(marker, grammar.commit);
  return dir;
}

/** `<다른 id>:<경로>` 는 그 문법 저장소의 파일, 아니면 이 문법 저장소의 파일. */
function queryFile(spec, grammar, byId) {
  const [other, path] = spec.includes(":") ? spec.split(":", 2) : [null, spec];
  const source = other ? byId.get(other) : grammar;
  if (!source) throw new Error(`grammar ${grammar.id}: unknown grammar \`${other}\` in highlights`);
  return { source, path, file: join(checkout(source), path) };
}

/** `out` 아래에 확장 폴더와 같은 모양으로 쓴다. 쓴 파일의 해시를 돌려준다. */
function buildOne(grammar, byId, out) {
  const src = checkout(grammar);
  const dir = join(src, grammar.path);
  if (grammar.generate && !existsSync(join(dir, "src", "parser.c"))) {
    run("tree-sitter", ["generate"], dir);
  }
  const ext = join(out, "extensions", grammar.extension);
  const wasm = join(ext, "grammars", `${grammar.id}.wasm`);
  mkdirSync(dirname(wasm), { recursive: true });
  run("tree-sitter", ["build", "--wasm", "-o", wasm, "."], dir);

  const parts = [
    "; 생성물 — scripts/grammars.mjs 가 아래 원본 쿼리를 이어 붙였다. 손으로 고치지 않는다(출처표 scripts/grammars.toml).",
  ];
  const licenses = new Map([[grammar.repo, grammar]]);
  for (const spec of grammar.highlights) {
    const q = queryFile(spec, grammar, byId);
    licenses.set(q.source.repo, q.source);
    parts.push(`\n; ---- ${q.source.repo} @ ${q.source.commit} — ${q.path}\n`);
    parts.push(readFileSync(q.file, "utf8").trimEnd());
  }
  const highlights = join(ext, "queries", grammar.id, "highlights.scm");
  mkdirSync(dirname(highlights), { recursive: true });
  writeFileSync(highlights, `${parts.join("\n")}\n`);

  // 문법 저장소의 LICENSE 는 LICENSE 로, 쿼리를 빌려 온 다른 저장소의 것은 LICENSE-<id> 로.
  for (const source of licenses.values()) {
    const name = source === grammar ? "LICENSE" : `LICENSE-${source.id}`;
    copyFileSync(join(checkout(source), "LICENSE"), join(ext, name));
  }
  return { wasm_sha256: sha256(wasm), highlights_sha256: sha256(highlights) };
}

function select(grammars, ids) {
  if (ids.length === 0) return grammars;
  const picked = grammars.filter((g) => ids.includes(g.id));
  const unknown = ids.filter((id) => !grammars.some((g) => g.id === id));
  if (unknown.length > 0) throw new Error(`unknown grammar: ${unknown.join(", ")}`);
  return picked;
}

function build(ids) {
  const grammars = readTable();
  const byId = new Map(grammars.map((g) => [g.id, g]));
  const hashes = new Map();
  for (const g of select(grammars, ids)) {
    console.log(`${g.id} ← ${g.repo} ${g.tag} (${g.commit.slice(0, 12)})`);
    hashes.set(g.id, buildOne(g, byId, root));
  }
  writeHashes(hashes);
}

function verify(ids) {
  const grammars = readTable();
  const byId = new Map(grammars.map((g) => [g.id, g]));
  const out = join(WORK, "verify");
  rmSync(out, { recursive: true, force: true });
  const differ = [];
  for (const g of select(grammars, ids)) {
    const h = buildOne(g, byId, out);
    for (const key of ["wasm_sha256", "highlights_sha256"]) {
      if (h[key] !== g[key]) differ.push(`${g.id}: ${key} ${h[key]} (table: ${g[key] || "empty"})`);
    }
  }
  if (differ.length > 0) {
    console.error(`rebuilt files differ from scripts/grammars.toml (output in ${out}):`);
    for (const d of differ) console.error(`  ${d}`);
    process.exit(1);
  }
  console.log("rebuilt files match scripts/grammars.toml");
}

function check() {
  const grammars = readTable();
  const problems = [];
  const vendored = readFileSync(VENDORED, "utf8");
  const listed = new Set();
  for (const g of grammars) {
    const ext = join(root, "extensions", g.extension);
    const files = [
      ["wasm_sha256", join(ext, "grammars", `${g.id}.wasm`)],
      ["highlights_sha256", join(ext, "queries", g.id, "highlights.scm")],
    ];
    for (const [key, file] of files) {
      listed.add(file);
      if (!existsSync(file)) {
        problems.push(`${g.id}: ${file} is missing — run \`mise run grammars\``);
      } else if (sha256(file) !== g[key]) {
        problems.push(
          `${g.id}: ${file} does not match ${key} in scripts/grammars.toml — regenerate it with \`mise run grammars\` instead of editing it`,
        );
      }
    }
    if (!existsSync(join(ext, "LICENSE"))) problems.push(`${g.id}: ${ext}/LICENSE is missing`);
    if (!vendored.includes(g.commit)) {
      problems.push(`${g.id}: licenses/vendored.md does not mention commit ${g.commit}`);
    }
  }
  // `grammars/` 밖에 둔 WASM 도 내장 확장에 묶여 매니페스트의 `grammar` 가 가리킬 수 있다 — 확장 폴더 전체를 훑는다.
  for (const file of wasmFiles(join(root, "extensions"))) {
    if (!listed.has(file)) problems.push(`${file} is not listed in scripts/grammars.toml`);
  }
  if (problems.length > 0) {
    for (const p of problems) console.error(p);
    process.exit(1);
  }
  console.log(`grammars: ${grammars.length} WASM + highlights match scripts/grammars.toml`);
}

/** `dir` 아래(하위 폴더 포함)의 `.wasm` 파일들. 내장 확장으로 묶이지 않는 `node_modules` 는 건너뛴다. */
function wasmFiles(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory() && entry.name !== "node_modules") out.push(...wasmFiles(path));
    else if (entry.isFile() && entry.name.endsWith(".wasm")) out.push(path);
  }
  return out;
}

const [command, ...args] = process.argv.slice(2);
switch (command) {
  case "build":
    build(args);
    break;
  case "verify":
    verify(args);
    break;
  case "check":
    check();
    break;
  default:
    console.error("usage: node scripts/grammars.mjs build [id…] | verify [id…] | check");
    process.exit(2);
}
