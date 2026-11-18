// java-debug 를 127.0.0.1 에만 묶는 패치를 만들고 검사한다(06-roadmap.md 결정 D71, 17-m6-plan.md M6-19). 순서와 규칙은
// CONTRIBUTING.md 「도구 registry」 의 java-debug 문단. 출처 · 컴파일 인자는 crates/pw-tools/patches/java-debug/upstream.toml.
//
//   mise run java-debug-patch -- build
//       상류 고정 커밋의 JavaDebugServer.java 를 받아 해시를 확인하고 loopback.patch 를 적용해 JavaDebugServer.java 를 쓰고, 고정
//       JDK 의 javac 로 classes/ 를 다시 만들고, registry 고정 표의 vsix 안 jar 를 설치기와 같은 코드(`registry_tool patch-jar`)로
//       바꿔 registry.toml 의 [[patched]] 행을 쓴 뒤 `check` 를 돈다.
//   mise run java-debug-patch -- check
//       `check-source` + 커밋된 소스를 다시 컴파일한 클래스가 classes/ 와 바이트 단위로 같은가 + 다시 묶은 jar 의 해시가
//       [[patched]] 행과 같은가. JDK(그 태스크가 고정)와 네트워크(vsix — target/java-debug-patch/ 에 두고 다시 받지 않는다)가 든다.
//   node scripts/java-debug-patch.mjs check-source
//       네트워크 · JDK 없이: 패치가 바인드 주소 한 줄만 바꾸고, 커밋된 소스를 거꾸로 되돌리면 원본 해시가 나오는가(`mise run lint`).
//
// 상류 소스 전체를 Tycho 로 빌드하지 않는 까닭: 그 판의 target 이 날마다 바뀌고 지워지는 저장소(jdtls snapshots/latest, Eclipse
// I-builds)를 가리켜 재현되지 않는다. 바꾸는 파일 하나만 컴파일해 고정 해시의 상류 jar 에 끼운다.
//
// 받은 vsix 는 미신뢰다: target/ 아래에 두고 실행하지 않는다. 클래스 경로로만 쓴다.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const CRATE = join(root, "crates", "pw-tools");
const DIR = join(CRATE, "patches", "java-debug");
const TOML = join(CRATE, "registry.toml");
const WORK = join(root, "target", "java-debug-patch");
const PATCH_NAME = "java-debug-loopback";
const UA = { "User-Agent": "pitwall-java-debug-patch" };

function fail(message) {
  console.error(`java-debug-patch: ${message}`);
  process.exit(1);
}

const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { cwd: root, encoding: "utf8", maxBuffer: 64 << 20, ...opts });
  if (r.error) fail(`${cmd}: ${r.error.message}`);
  if (r.status !== 0) fail(`${cmd} ${args.join(" ")} failed:\n${r.stderr ?? ""}${r.stdout ?? ""}`);
  return r.stdout;
}

function tool(...args) {
  return run("cargo", ["run", "-q", "-p", "pw-tools", "--example", "registry_tool", "--", ...args]);
}

/** upstream.toml 의 `키 = "글"` · `키 = ["글", …]` 만 읽는다(이 파일의 꼴). */
export function parseUpstream(text) {
  const out = {};
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const m = line.match(/^([a-z_0-9]+)\s*=\s*(.+)$/);
    if (!m) throw new Error(`upstream.toml: cannot read ${line}`);
    out[m[1]] = JSON.parse(m[2]);
  }
  return out;
}

function upstream() {
  return parseUpstream(readFileSync(join(DIR, "upstream.toml"), "utf8"));
}

/**
 * 한 덩어리짜리 unified diff 를 읽는다. 바뀐 줄이 정확히 하나(`-` 하나 · `+` 하나)이고 그 줄이 서버 소켓을 여는 줄이며, 새 줄은
 * 옛 줄의 `new ServerSocket(0, 1)` 에 loopback 주소 인자만 더한 것인지 본다(결정 D71 — 패치는 바인드 주소 한 줄만).
 */
export function parsePatch(text) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines.at(-1) === "") lines.pop();
  if (!lines[0]?.startsWith("--- ") || !lines[1]?.startsWith("+++ "))
    throw new Error("the patch has no ---/+++ header");
  const hunks = lines.filter((l) => l.startsWith("@@"));
  if (hunks.length !== 1) throw new Error(`the patch has ${hunks.length} hunks, not 1`);
  const body = lines.slice(3);
  const removed = body.filter((l) => l.startsWith("-"));
  const added = body.filter((l) => l.startsWith("+"));
  if (removed.length !== 1 || added.length !== 1)
    throw new Error(
      `the patch changes ${removed.length} - / ${added.length} + lines, not one line`,
    );
  for (const l of body) {
    if (!/^[ +-]/.test(l)) throw new Error(`the patch has an unexpected line: ${l}`);
  }
  const before = removed[0].slice(1);
  const after = added[0].slice(1);
  const expected = before.replace(
    "new ServerSocket(0, 1)",
    "new ServerSocket(0, 1, java.net.InetAddress.getLoopbackAddress())",
  );
  if (before === expected || after !== expected)
    throw new Error("the changed line is not the loopback bind of `new ServerSocket(0, 1)`");
  return {
    before,
    after,
    context: body.map((l) => ({ kind: l[0], text: l.slice(1) })),
  };
}

/** 패치를 앞으로(원본 → 고친 것) 또는 거꾸로 적용한다. 문맥이 맞는 자리가 정확히 하나여야 한다. */
export function applyPatch(source, patch, reverse = false) {
  const lines = source.split("\n");
  const from = patch.context.filter((l) => l.kind !== (reverse ? "-" : "+")).map((l) => l.text);
  const to = patch.context.filter((l) => l.kind !== (reverse ? "+" : "-")).map((l) => l.text);
  const at = [];
  for (let i = 0; i + from.length <= lines.length; i++) {
    if (from.every((l, k) => lines[i + k] === l)) at.push(i);
  }
  if (at.length !== 1) throw new Error(`the patch context matches ${at.length} places, not 1`);
  lines.splice(at[0], from.length, ...to);
  return lines.join("\n");
}

/** 네트워크 · JDK 없이 보는 것. 문제 목록을 돌려준다. */
export function checkSource({ upstream: up, patchText, patched }) {
  const problems = [];
  let patch;
  try {
    patch = parsePatch(patchText);
  } catch (e) {
    return [e.message];
  }
  let original;
  try {
    original = applyPatch(patched, patch, true);
  } catch (e) {
    return [`JavaDebugServer.java: ${e.message}`];
  }
  if (sha256(original) !== up.source_sha256)
    problems.push(
      `JavaDebugServer.java with the patch reversed has SHA-256 ${sha256(original)}, upstream.toml says ${up.source_sha256} — the source differs from upstream beyond the patch`,
    );
  return problems;
}

function sourceProblems() {
  const up = upstream();
  return checkSource({
    upstream: up,
    patchText: readFileSync(join(DIR, "loopback.patch"), "utf8"),
    patched: readFileSync(join(DIR, "JavaDebugServer.java"), "utf8"),
  });
}

async function download(url) {
  const { followHttps } = await import("./tools-registry-bump.mjs");
  const res = await followHttps(url, { headers: UA }).catch((e) => fail(e.message));
  if (!res.ok) fail(`${url} answered ${res.status}`);
  return Buffer.from(await res.arrayBuffer());
}

/** registry.toml 에서 그 패키지 · 판의 산출물 행(대상과 상관없이 같은 vsix)의 주소와 해시. */
function vsixRow(up) {
  const text = readFileSync(TOML, "utf8");
  for (const block of text.split(/^\[\[artifact\]\]\s*$/m).slice(1)) {
    const get = (k) => block.match(new RegExp(`^${k}\\s*=\\s*"([^"]*)"`, "m"))?.[1];
    if (get("package") === up.package && get("version") === up.version)
      return { url: get("url"), sha256: get("sha256") };
  }
  fail(`registry.toml has no artifact for ${up.package} ${up.version}`);
}

/** 고정 해시의 vsix 를 받아(이미 있으면 그것) 그 안의 jar 와 클래스 경로 jar 를 꺼낸다. */
async function upstreamJars(up) {
  const row = vsixRow(up);
  mkdirSync(WORK, { recursive: true });
  const vsix = join(WORK, `${row.sha256}.vsix`);
  if (!existsSync(vsix) || sha256(readFileSync(vsix)) !== row.sha256) {
    console.error(`downloading ${row.url}`);
    const bytes = await download(row.url);
    if (sha256(bytes) !== row.sha256)
      fail(`${row.url} has SHA-256 ${sha256(bytes)}, registry.toml pins ${row.sha256}`);
    writeFileSync(vsix, bytes);
  }
  const jar = join(WORK, "upstream.jar");
  tool("zip-entry", vsix, up.vsix_entry, jar);
  const classpath = [jar];
  up.classpath_in_jar.forEach((entry, i) => {
    const out = join(WORK, `classpath-${i}.jar`);
    tool("zip-entry", jar, entry, out);
    classpath.push(out);
  });
  return { jar, classpath };
}

function javac(up, classpath, outDir) {
  const v = spawnSync("javac", ["-version"], { encoding: "utf8" });
  if (v.error) fail(`javac: ${v.error.message} — run through \`mise run java-debug-patch\``);
  const shown = `${v.stdout ?? ""}${v.stderr ?? ""}`.trim();
  if (shown !== `javac ${up.javac_version}`)
    fail(
      `javac is "${shown}", upstream.toml pins javac ${up.javac_version} — run through \`mise run java-debug-patch\``,
    );
  const sep = process.platform === "win32" ? ";" : ":";
  run("javac", [
    ...up.javac_args,
    "-sourcepath",
    "",
    "-cp",
    classpath.join(sep),
    "-d",
    outDir,
    join(DIR, "JavaDebugServer.java"),
  ]);
  const pkg = join(outDir, ...up.class_prefix.split("/").filter(Boolean));
  return Object.fromEntries(
    readdirSync(pkg)
      .filter((n) => n.endsWith(".class"))
      .sort()
      .map((n) => [n, readFileSync(join(pkg, n))]),
  );
}

function committedClasses() {
  const dir = join(DIR, "classes");
  return Object.fromEntries(
    readdirSync(dir)
      .filter((n) => n.endsWith(".class"))
      .sort()
      .map((n) => [n, readFileSync(join(dir, n))]),
  );
}

/** registry.toml 의 [[patched]] 행을 그 하나로 바꾼다. */
export function withPatchedRow(text, row) {
  const kept = text.replace(/\n*^\[\[patched\]\]\n(?:^(?!\[).*\n?)*/gm, "\n").replace(/\n+$/, "\n");
  const q = JSON.stringify;
  return `${kept}
[[patched]]
package = ${q(row.package)}
version = ${q(row.version)}
file = ${q(row.file)}
patch = ${q(row.patch)}
sha256 = ${q(row.sha256)}
patched_sha256 = ${q(row.patched_sha256)}
`;
}

/** registry.toml 의 [[patched]] 행. */
export function patchedRows(text) {
  return text
    .split(/^\[\[patched\]\]\s*$/m)
    .slice(1)
    .map((block) => {
      const get = (k) => block.match(new RegExp(`^${k}\\s*=\\s*"([^"]*)"`, "m"))?.[1];
      return {
        package: get("package"),
        version: get("version"),
        file: get("file"),
        patch: get("patch"),
        sha256: get("sha256"),
        patched_sha256: get("patched_sha256"),
      };
    });
}

async function build() {
  const up = upstream();
  const url = `https://raw.githubusercontent.com/microsoft/java-debug/${up.commit}/${up.source}`;
  console.error(`downloading ${url}`);
  const original = (await download(url)).toString("utf8");
  if (sha256(original) !== up.source_sha256)
    fail(
      `${up.source} at ${up.commit} has SHA-256 ${sha256(original)}, upstream.toml says ${up.source_sha256}`,
    );
  const patch = parsePatch(readFileSync(join(DIR, "loopback.patch"), "utf8"));
  writeFileSync(join(DIR, "JavaDebugServer.java"), applyPatch(original, patch));

  const { jar, classpath } = await upstreamJars(up);
  const out = mkdtempSync(join(tmpdir(), "pw-java-debug-"));
  try {
    const classes = javac(up, classpath, out);
    const dir = join(DIR, "classes");
    rmSync(dir, { recursive: true, force: true });
    mkdirSync(dir, { recursive: true });
    for (const [name, bytes] of Object.entries(classes)) writeFileSync(join(dir, name), bytes);
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
  // classes/ 가 바뀌었으면 cargo 가 registry_tool 을 다시 빌드한다(include_bytes).
  const hashes = JSON.parse(tool("patch-jar", PATCH_NAME, jar, join(WORK, "patched.jar")));
  writeFileSync(
    TOML,
    withPatchedRow(readFileSync(TOML, "utf8"), {
      package: up.package,
      version: up.version,
      file: up.vsix_entry,
      patch: PATCH_NAME,
      ...hashes,
    }),
  );
  console.log(`patched ${up.vsix_entry}: ${hashes.sha256} -> ${hashes.patched_sha256}`);
  await check();
}

async function check() {
  const problems = sourceProblems();
  const up = upstream();
  const { jar, classpath } = await upstreamJars(up);
  const out = mkdtempSync(join(tmpdir(), "pw-java-debug-"));
  try {
    const built = javac(up, classpath, out);
    const committed = committedClasses();
    const names = (o) => Object.keys(o).join(", ");
    if (names(built) !== names(committed))
      problems.push(`javac makes ${names(built)} but classes/ has ${names(committed)}`);
    for (const [name, bytes] of Object.entries(built)) {
      if (committed[name] && !committed[name].equals(bytes))
        problems.push(`classes/${name} differs from what javac makes of JavaDebugServer.java`);
    }
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
  const hashes = JSON.parse(tool("patch-jar", PATCH_NAME, jar, join(WORK, "patched.jar")));
  const row = patchedRows(readFileSync(TOML, "utf8")).find(
    (r) => r.package === up.package && r.version === up.version,
  );
  if (!row) problems.push(`registry.toml has no [[patched]] row for ${up.package} ${up.version}`);
  else {
    if (row.file !== up.vsix_entry)
      problems.push(`[[patched]] file is ${row.file}, not ${up.vsix_entry}`);
    if (row.sha256 !== hashes.sha256)
      problems.push(`[[patched]] sha256 is ${row.sha256}, the upstream jar has ${hashes.sha256}`);
    if (row.patched_sha256 !== hashes.patched_sha256)
      problems.push(
        `[[patched]] patched_sha256 is ${row.patched_sha256}, the rebuilt jar has ${hashes.patched_sha256}`,
      );
  }
  report(problems);
  console.log(`java-debug patch: classes and ${hashes.patched_sha256} rebuilt from source`);
}

function report(problems) {
  if (problems.length > 0) {
    for (const p of problems) console.error(`java-debug-patch: ${p}`);
    process.exit(1);
  }
}

const direct = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
const [cmd] = direct ? process.argv.slice(2) : ["--imported"];
if (cmd === "--imported") {
  // 시험(scripts/java-debug-patch.test.mjs)이 불러 쓴다.
} else if (cmd === "build") {
  await build();
} else if (cmd === "check") {
  await check();
} else if (cmd === "check-source") {
  report(sourceProblems());
  console.log("java-debug patch: one loopback line over the pinned upstream source");
} else {
  console.error("usage: node scripts/java-debug-patch.mjs build | check | check-source");
  process.exit(2);
}
