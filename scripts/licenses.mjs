// npm 의존성 라이선스 검사와 THIRD_PARTY_NOTICES.md 생성. `mise run licenses` / `mise run notices` 가 부른다.
//
//   node scripts/licenses.mjs check             npm 라이선스 허용 목록 검사 + about.toml 동기 검사
//   node scripts/licenses.mjs notices           THIRD_PARTY_NOTICES.md 생성
//   node scripts/licenses.mjs notices --check   커밋된 THIRD_PARTY_NOTICES.md 가 최신인지 검사(고치지 않는다)
//
// 허용 목록의 정본은 deny.toml 의 [licenses].allow 다. Rust 는 cargo-deny 가, npm 은 이 스크립트가 같은 목록으로 본다.
// license-checker 대신 pnpm licenses list 를 쓴다. license-checker 는 npm 의 중첩 node_modules 배치를 전제하고
// 관리도 멈췄다. pnpm licenses 는 pnpm 내장이라 의존성을 더 들이지 않고 워크스페이스 전체를 본다.
// ⚠ 지금 OS 에 설치된 패키지만 본다. 다른 OS 전용 바이너리 패키지(@esbuild/win32-x64 등)는 검사하지 않는다.

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const noticesPath = join(root, "THIRD_PARTY_NOTICES.md");

// 따옴표 친 문자열 배열 하나만 읽는 좁은 TOML 파서. deny.toml·about.toml 의 형식(한 줄에 항목 하나)에 기대고 있다.
function readStringArray(file, section, key) {
  const text = readFileSync(join(root, file), "utf8");
  let current = null;
  let inArray = false;
  const values = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.replace(/#.*$/, "").trim();
    if (inArray) {
      if (line.startsWith("]")) return values;
      const m = line.match(/^"([^"]+)",?$/);
      if (m) values.push(m[1]);
      else if (line) throw new Error(`${file}: ${key} 배열의 줄을 읽지 못했다: ${raw}`);
      continue;
    }
    const header = line.match(/^\[([^\]]+)\]$/);
    if (header) current = header[1];
    else if (current === section && new RegExp(`^${key}\\s*=\\s*\\[$`).test(line)) inArray = true;
  }
  throw new Error(`${file}: ${section ?? "(최상위)"} 의 ${key} 배열을 찾지 못했다`);
}

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
  // 패키지가 하나도 없으면 JSON 대신 "No licenses in packages found" 를 출력한다.
  if (!out.startsWith("{")) return [];
  return Object.values(JSON.parse(out)).flat();
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
  const allowList = readStringArray("deny.toml", "licenses", "allow");
  const accepted = readStringArray("about.toml", null, "accepted");
  let failed = false;

  if (JSON.stringify([...allowList].sort()) !== JSON.stringify([...accepted].sort())) {
    console.error(
      "about.toml 의 accepted 가 deny.toml 의 [licenses].allow 와 다르다. 두 목록을 같게 맞춘다.",
    );
    failed = true;
  }

  const allow = new Set(allowList);
  const packages = pnpmLicenses(false);
  for (const p of packages) {
    let ok;
    try {
      ok = spdxAllowed(p.license, allow);
    } catch {
      ok = false;
    }
    if (!ok) {
      console.error(`허용되지 않은 라이선스: ${p.name}@${p.versions.join(",")} — ${p.license}`);
      failed = true;
    }
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

function rustSection() {
  const about = JSON.parse(run("cargo", ["about", "generate", "--format", "json"]));
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

function renderNotices() {
  return [
    "<!-- 생성물이다. 직접 고치지 말고 `mise run notices` 로 다시 만든다(생성기: scripts/licenses.mjs). -->",
    "",
    "# Third-party notices",
    "",
    "Pitwall includes the following third-party software. Development-only dependencies are not listed.",
    "",
    ...rustSection(),
    ...npmSection(),
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
