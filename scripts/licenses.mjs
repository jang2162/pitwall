// npm 의존성 라이선스 검사와 THIRD_PARTY_NOTICES.md 생성. `mise run licenses` / `mise run notices` 가 부른다.
//
//   node scripts/licenses.mjs check             npm 라이선스 허용 목록 검사 + about.toml 동기 검사
//   node scripts/licenses.mjs notices           THIRD_PARTY_NOTICES.md 생성
//   node scripts/licenses.mjs notices --check   커밋된 THIRD_PARTY_NOTICES.md 가 최신인지 검사(고치지 않는다)
//
// 허용 목록의 정본은 deny.toml 의 [licenses].allow 다. Rust 는 cargo-deny 가, npm 은 이 스크립트가 같은 목록으로 본다.
// 크레이트 단위 예외([[licenses.exceptions]])는 Rust 에만 적용한다. npm 의 패키지 단위 예외는
// licenses/npm-exceptions.toml 에 따로 두고, 개발 의존성에만 유효하다(배포 의존성에 나타나면 실패).
// license-checker 대신 pnpm licenses list 를 쓴다. license-checker 는 npm 의 중첩 node_modules 배치를 전제하고
// 관리도 멈췄다. pnpm licenses 는 pnpm 내장이라 의존성을 더 들이지 않고 워크스페이스 전체를 본다.
// ⚠ 지금 OS 에 설치된 패키지만 본다. 다른 OS 전용 바이너리 패키지(@esbuild/win32-x64 등)는 검사하지 않는다.

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const noticesPath = join(root, "THIRD_PARTY_NOTICES.md");
const vendoredPath = join(root, "licenses", "vendored.md");
const npmExceptionsPath = join("licenses", "npm-exceptions.toml");

// deny.toml·about.toml 을 읽는 좁은 TOML 파서. 표 머리(`[a]`, `[[a]]`)와 `키 = 값` 만 알고, 값은 원문 그대로 둔다
// (여러 줄 배열은 `]` 로 시작하는 줄까지 이어 붙인다). 문자열 안의 `#` 은 주석으로 잘못 읽는다 — 두 파일에 그런 값이 없다.
function readToml(file) {
  const text = readFileSync(join(root, file), "utf8");
  const tables = new Map([["", {}]]);
  const arrays = new Map();
  let table = tables.get("");
  let pending = null;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.replace(/#.*$/, "").trim();
    if (pending) {
      pending.value += ` ${line}`;
      if (line.startsWith("]")) {
        table[pending.key] = pending.value;
        pending = null;
      }
      continue;
    }
    if (!line) continue;
    let m;
    if ((m = line.match(/^\[\[([^\]]+)\]\]$/))) {
      table = {};
      if (!arrays.has(m[1])) arrays.set(m[1], []);
      arrays.get(m[1]).push(table);
    } else if ((m = line.match(/^\[([^\]]+)\]$/))) {
      table = {};
      tables.set(m[1], table);
    } else if ((m = line.match(/^([A-Za-z0-9_-]+)\s*=\s*(.*)$/))) {
      if (m[2] === "[") pending = { key: m[1], value: m[2] };
      else table[m[1]] = m[2];
    } else throw new Error(`${file}: 읽지 못한 줄: ${raw}`);
  }
  if (pending) throw new Error(`${file}: ${pending.key} 배열이 닫히지 않았다`);
  return { tables, arrays };
}

// 문자열 배열 값(원문)에서 항목을 꺼낸다. 값이 없으면 실패한다 — 키 이름이 틀려 빈 목록으로 통과하는 것을 막는다.
function strings(value, where) {
  if (value === undefined) throw new Error(`${where} 가 없다`);
  return [...value.matchAll(/"([^"]*)"/g)].map((m) => m[1]);
}

// 「크레이트:라이선스」 문자열 목록. deny.toml 과 about.toml 의 크레이트 단위 예외를 같은 모양으로 비교한다.
function denyExceptions(deny) {
  return (deny.arrays.get("licenses.exceptions") ?? []).flatMap((e) => {
    const [crate] = strings(e.crate, "deny.toml [[licenses.exceptions]] crate");
    return strings(e.allow, `deny.toml ${crate} 예외의 allow`).map((l) => `${crate}:${l}`);
  });
}

function aboutExceptions(about) {
  return [...about.tables]
    .filter(([name, t]) => name !== "" && t.accepted !== undefined)
    .flatMap(([name, t]) =>
      strings(t.accepted, `about.toml [${name}] accepted`).map((l) => `${name}:${l}`),
    );
}

// npm 패키지 단위 예외. 이유가 없는 예외는 받지 않는다 — 다음 사람이 걷어도 되는지 판정할 근거가 없어진다.
function npmExceptions() {
  const file = readToml(npmExceptionsPath);
  return (file.arrays.get("exceptions") ?? []).map((e) => {
    const [name] = strings(e.package, `${npmExceptionsPath} [[exceptions]] package`);
    const allow = strings(e.allow, `${npmExceptionsPath} ${name} 예외의 allow`);
    const [reason] = strings(e.reason, `${npmExceptionsPath} ${name} 예외의 reason`);
    if (!name || allow.length === 0 || !reason?.trim()) {
      throw new Error(
        `${npmExceptionsPath}: ${name || "(이름 없음)"} 예외에 package·allow·reason 이 다 있어야 한다`,
      );
    }
    return { name, allow, reason };
  });
}

const sameSet = (a, b) => JSON.stringify([...a].sort()) === JSON.stringify([...b].sort());

function run(cmd, args) {
  // Windows 의 pnpm 은 pnpm.cmd 라 셸을 거쳐야 찾힌다.
  return execFileSync(cmd, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    shell: process.platform === "win32",
    stdio: ["ignore", "pipe", "inherit"],
  });
}

function pnpmLicenses(prodOnly) {
  const out = run("pnpm", ["licenses", "list", "--json", ...(prodOnly ? ["--prod"] : [])]).trim();
  // 패키지가 하나도 없으면 JSON 대신 이 문구를 출력한다. 빈 결과는 이 문구일 때만 인정한다 — 그 밖에 JSON 이 아닌
  // 출력(경고 줄이 앞에 붙는 등)을 빈 결과로 보면 검사가 0개로 조용히 통과한다.
  if (out === "No licenses in packages found") return [];
  try {
    return Object.values(JSON.parse(out)).flat();
  } catch {
    console.error(`pnpm licenses list 의 출력을 JSON 으로 읽지 못했다:\n${out.slice(0, 500)}`);
    process.exit(1);
  }
}

// SPDX 식 판정. OR 는 하나라도, AND 는 전부 허용돼야 한다. `X WITH Y` 는 그 문자열 그대로 목록에 있어야 한다.
function spdxAllowed(expr, allow) {
  const tokens = expr.match(/\(|\)|[^\s()]+/g) ?? [];
  let i = 0;
  const primary = () => {
    const t = tokens[i++];
    if (t === "(") {
      const v = orExpr();
      if (tokens[i++] !== ")") throw new Error(`괄호가 닫히지 않은 SPDX 식: ${expr}`);
      return v;
    }
    if (t === undefined || t === ")" || t === "AND" || t === "OR")
      throw new Error(`SPDX 식을 읽지 못했다: ${expr}`);
    if (tokens[i] === "WITH") {
      i++;
      return allow.has(`${t} WITH ${tokens[i++]}`);
    }
    return allow.has(t);
  };
  const andExpr = () => {
    let v = primary();
    while (tokens[i] === "AND") {
      i++;
      v = primary() && v;
    }
    return v;
  };
  const orExpr = () => {
    let v = andExpr();
    while (tokens[i] === "OR") {
      i++;
      v = andExpr() || v;
    }
    return v;
  };
  const v = orExpr();
  if (i !== tokens.length) throw new Error(`SPDX 식을 읽지 못했다: ${expr}`);
  return v;
}

function check() {
  const deny = readToml("deny.toml");
  const about = readToml("about.toml");
  const allowList = strings(deny.tables.get("licenses")?.allow, "deny.toml [licenses] allow");
  const accepted = strings(about.tables.get("").accepted, "about.toml accepted");
  let failed = false;

  if (!sameSet(allowList, accepted)) {
    console.error(
      "about.toml 의 accepted 가 deny.toml 의 [licenses].allow 와 다르다. 두 목록을 같게 맞춘다.",
    );
    failed = true;
  }
  if (!sameSet(denyExceptions(deny), aboutExceptions(about))) {
    console.error(
      "about.toml 의 크레이트별 accepted 가 deny.toml 의 [[licenses.exceptions]] 와 다르다. 크레이트와 라이선스를 같게 맞춘다.",
    );
    failed = true;
  }

  const allow = new Set(allowList);
  const packages = pnpmLicenses(false);
  const prodNames = new Set(pnpmLicenses(true).map((p) => p.name));
  const exceptions = npmExceptions();
  const used = new Set();
  for (const exception of exceptions) {
    if (prodNames.has(exception.name)) {
      console.error(
        `${exception.name} 이 배포 의존성에 있다. ${npmExceptionsPath} 의 예외는 개발 의존성에만 유효하다 — 의존성을 바꾸거나 라이선스를 다시 판단한다.`,
      );
      failed = true;
    }
  }
  for (const p of packages) {
    const exception = exceptions.find((e) => e.name === p.name);
    const allowed = (list) => {
      try {
        return spdxAllowed(p.license, list);
      } catch {
        return false;
      }
    };
    if (allowed(allow)) continue;
    if (exception && allowed(new Set([...allow, ...exception.allow]))) {
      used.add(exception.name);
      continue;
    }
    console.error(`허용되지 않은 라이선스: ${p.name}@${p.versions.join(",")} — ${p.license}`);
    failed = true;
  }
  for (const exception of exceptions) {
    if (!used.has(exception.name)) {
      console.warn(
        `경고: 쓰이지 않는 예외 — ${exception.name}. 트리에서 빠졌으면 ${npmExceptionsPath} 에서 걷는다.`,
      );
    }
  }
  // dev 의존성(oxfmt·typescript 등)이 늘 있으므로 0개는 pnpm 출력을 못 읽었다는 뜻이다.
  if (packages.length === 0) {
    console.error(
      "npm 패키지를 하나도 찾지 못했다. pnpm install 이 됐는지, pnpm licenses list 출력이 맞는지 본다.",
    );
    failed = true;
  }
  if (failed) process.exit(1);
  console.log(`npm 라이선스 검사 통과: 패키지 ${packages.length}개`);
}

// 마크다운 코드 펜스. 라이선스 본문에 백틱이 들어 있어도 깨지지 않게 본문보다 긴 펜스를 쓴다.
function fenced(text) {
  const longest = Math.max(0, ...(text.match(/`+/g) ?? []).map((s) => s.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}text\n${text.replace(/\r\n/g, "\n").trimEnd()}\n${fence}`;
}

// cargo-about 의 stderr 에서 「그래프에 없는 크레이트의 설정」 경고만 한 줄로 줄인다. about.toml 의 크레이트별
// 예외가 아직 없는 Tauri 트리를 위한 것이라 M0-04 전까지는 매번 나온다. 나머지 경고·오류는 그대로 보인다.
function cargoAbout() {
  const r = spawnSync("cargo", ["about", "generate", "--format", "json"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    shell: process.platform === "win32",
  });
  if (r.error) throw r.error;
  // cargo-about 의 로그 줄은 파이프로 받아도 ANSI 색 코드가 붙는다. 그것을 지우려면 ESC 를 직접 맞춰야 한다.
  // eslint-disable-next-line no-control-regex
  const blocks = r.stderr.replace(/\x1b\[[0-9;]*m/g, "").split(/\n\s*\n/);
  const absent = blocks.filter((b) =>
    b.startsWith("warning: config found for a crate not present in the graph"),
  );
  const rest = blocks
    .filter((b) => !absent.includes(b))
    .join("\n")
    .trim();
  if (rest) process.stderr.write(`${rest}\n`);
  if (absent.length)
    console.error(
      `cargo-about: 의존성 그래프에 없는 크레이트 설정 ${absent.length}개(about.toml 의 크레이트별 예외)`,
    );
  if (r.status !== 0) {
    console.error(`cargo about generate 실패(exit ${r.status})`);
    process.exit(1);
  }
  return JSON.parse(r.stdout);
}

function rustSection() {
  const about = cargoAbout();
  const lines = ["## Rust crates", ""];
  if (about.licenses.length === 0) return [...lines, "None.", ""];
  const licenses = [...about.licenses].sort(
    (a, b) => a.id.localeCompare(b.id) || a.text.localeCompare(b.text),
  );
  for (const l of licenses) {
    const users = l.used_by
      .map((u) => `${u.crate.name} ${u.crate.version}`)
      .sort((a, b) => a.localeCompare(b));
    lines.push(
      `### ${l.name} (${l.id})`,
      "",
      `Used by: ${users.join(", ")}`,
      "",
      fenced(l.text),
      "",
    );
  }
  return lines;
}

function licenseText(dir) {
  const file = readdirSync(dir)
    .filter((f) => /^(licen[cs]e|copying)([.-].*)?$/i.test(f))
    .sort()[0];
  return file ? readFileSync(join(dir, file), "utf8") : null;
}

function npmSection() {
  const lines = ["## npm packages", ""];
  const entries = [];
  for (const p of pnpmLicenses(true)) {
    p.versions.forEach((version, idx) => {
      const dir = p.paths[idx];
      const manifest = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
      // os·cpu 를 선언한 플랫폼 전용 패키지는 그 OS 에만 설치된다. 넣으면 생성 결과가 OS 마다 달라져
      // 최신 여부 검사가 OS 에 따라 갈린다.
      if (manifest.os || manifest.cpu) return;
      entries.push({
        name: p.name,
        version,
        license: p.license,
        homepage: p.homepage,
        text: licenseText(dir),
      });
    });
  }
  if (entries.length === 0) return [...lines, "None.", ""];
  entries.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
  for (const e of entries) {
    lines.push(`### ${e.name} ${e.version} (${e.license})`, "");
    if (e.homepage) lines.push(`Homepage: ${e.homepage}`, "");
    lines.push(e.text ? fenced(e.text) : "No license file in the package.", "");
  }
  return lines;
}

// 의존성이 아니라 저장소에 복사해 넣은 코드·데이터(DBX 코드 차용, Linguist 데이터 등)의 고지는 손으로 쓴다.
// 첫 `## ` 절부터를 그대로 싣는다. 그 위의 안내문은 이 파일을 고치는 사람을 위한 것이다.
function vendoredSection() {
  const text = readFileSync(vendoredPath, "utf8").replace(/\r\n/g, "\n");
  const at = text.search(/^## /m);
  if (at < 0) throw new Error("licenses/vendored.md 에 `## ` 절이 없다");
  return [text.slice(at).trimEnd(), ""];
}

function renderNotices() {
  return [
    "<!-- 생성물이다. 직접 고치지 말고 `mise run notices` 로 다시 만든다(생성기: scripts/licenses.mjs).",
    "     손으로 쓰는 고지(복사해 넣은 코드)는 licenses/vendored.md 에 쓴다. -->",
    "",
    "# Third-party notices",
    "",
    "Pitwall includes the following third-party software. Development-only dependencies are not listed.",
    "",
    ...rustSection(),
    ...npmSection(),
    ...vendoredSection(),
  ]
    .join("\n")
    .replace(/\n+$/, "\n");
}

function notices(checkOnly) {
  const next = renderNotices();
  if (!checkOnly) {
    writeFileSync(noticesPath, next);
    console.log("THIRD_PARTY_NOTICES.md 를 생성했다");
    return;
  }
  const current = existsSync(noticesPath)
    ? readFileSync(noticesPath, "utf8").replace(/\r\n/g, "\n")
    : "";
  if (current !== next) {
    console.error(
      "THIRD_PARTY_NOTICES.md 가 최신이 아니다. `mise run notices` 로 다시 만들어 커밋한다.",
    );
    process.exit(1);
  }
  console.log("THIRD_PARTY_NOTICES.md 최신");
}

const [cmd, flag] = process.argv.slice(2);
if (cmd === "check") check();
else if (cmd === "notices") notices(flag === "--check");
else {
  console.error("사용법: node scripts/licenses.mjs check | notices [--check]");
  process.exit(2);
}
