// Mason registry 고정 판을 올린다(15-m4-plan.md M4-03 · 결정 M4-D16 · D17). 순서와 규칙은 CONTRIBUTING.md 「도구 registry」 절.
//
//   node scripts/tools-registry-bump.mjs bump [<태그>|latest] [--download-all]
//       mason-org/mason-registry 의 그 릴리스(없으면 latest)의 `checksums.txt` · `registry.json.zip` 을 받아 해시를 확인하고
//       ① crates/pw-tools/registry.toml 의 [registry](판 · 주소 · 두 해시)
//       ② crates/pw-tools/snapshot/bundled.json(기본 번들 패키지의 정의만 — 내장 스냅숏)
//       ③ crates/pw-tools/tests/fixtures/registry.json.zip(고정 판 전체 — 파싱 표 시험의 입력)
//       ④ registry.toml 의 [[artifact]](기본 번들 패키지의 (판, 대상, 파일)별 SHA-256 · 크기)
//       를 다시 쓰고 `check` 를 돌린다. 번들 목록(`bundled`)은 지금 registry.toml 의 것을 쓴다.
//   node scripts/tools-registry-bump.mjs check
//       registry.toml 이 스냅숏의 번들 패키지를 모두 덮는지(`mise run lint` 와 같다 — 네트워크 없이 돈다).
//
// 해시를 얻는 곳(행의 `hash_from`): GitHub 릴리스 자산은 API 의 `digest`(GitHub 이 올릴 때 계산 — 없으면 받는다), PyPI 휠은
// PyPI JSON 의 `digests.sha256`, 그 밖은 배포처가 함께 올린 `<주소>.sha256` 이 있으면 그것(크기는 HEAD), 없으면 받아서 계산한다.
// `--download-all` 이면 모두 받아서 계산한다(kotlin-lsp 는 대상마다 약 360 MB). npm 타르볼은 늘 받아 `dist.integrity` 와 대조한다.
// 패키지를 어떻게 푸는지(대상 · 표현식 · 덮어쓰기)는 Rust 쪽(`registry_tool plan`)이 정한다 — 같은 규칙을 JS 로 다시 쓰지 않는다.
//
// 받은 데이터는 미신뢰다: 산출물은 target/tools-registry/<태그>/ 에 두고 실행하지 않는다.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const CRATE = join(root, "crates", "pw-tools");
const TOML = join(CRATE, "registry.toml");
const SNAPSHOT = join(CRATE, "snapshot", "bundled.json");
const FIXTURE = join(CRATE, "tests", "fixtures", "registry.json.zip");
const WORK = join(root, "target", "tools-registry");
const REPO = "mason-org/mason-registry";
const UA = { "User-Agent": "pitwall-tools-registry-bump" };
const TARGETS = [
  "darwin_arm64",
  "darwin_x64",
  "linux_x64_gnu",
  "linux_arm64_gnu",
  "linux_x64_musl",
  "win_x64",
];

function fail(message) {
  console.error(`tools-registry-bump: ${message}`);
  process.exit(1);
}

function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { cwd: root, encoding: "utf8", maxBuffer: 64 << 20, ...opts });
  if (r.status !== 0) fail(`${cmd} ${args.join(" ")} failed:\n${r.stderr ?? ""}`);
  return r.stdout;
}

function tool(...args) {
  return run("cargo", ["run", "-q", "-p", "pw-tools", "--example", "registry_tool", "--", ...args]);
}

const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

/** 리다이렉트를 따라가는 횟수의 상한(crates/pw-tools/src/fetch.rs 와 같다). */
export const MAX_REDIRECTS = 5;

/**
 * `fetch` 를 리다이렉트 홉마다 검사하며 따라간다 — 모든 홉이 https 여야 하고 `MAX_REDIRECTS` 번까지(Rust 의 받기와 같은 규칙).
 * `redirect: "follow"` 뒤 마지막 주소만 보면 중간의 http 홉을 놓친다(리뷰 F6). 어기면 던진다.
 */
export async function followHttps(url, init = {}, fetchImpl = fetch) {
  let current = url;
  for (let hop = 0; ; hop++) {
    if (!current.startsWith("https://")) throw new Error(`refusing a non-https URL: ${current}`);
    const res = await fetchImpl(current, { ...init, redirect: "manual" });
    if (res.status < 300 || res.status >= 400 || res.status === 304) return res;
    const location = res.headers.get("location");
    if (!location) return res;
    if (hop >= MAX_REDIRECTS) throw new Error(`more than ${MAX_REDIRECTS} redirects from ${url}`);
    current = new URL(location, current).href;
  }
}

async function get(url, { json = false, method = "GET", headers = {} } = {}) {
  const res = await followHttps(url, { method, headers: { ...UA, ...headers } }).catch((e) =>
    fail(e.message),
  );
  if (!res.ok) return { ok: false, status: res.status };
  if (method === "HEAD") return { ok: true, headers: res.headers };
  if (json) return { ok: true, value: await res.json() };
  return { ok: true, bytes: Buffer.from(await res.arrayBuffer()) };
}

async function mustGet(url, opts) {
  const r = await get(url, opts);
  if (!r.ok) fail(`${url} answered ${r.status}`);
  return r;
}

/** 받아서 해시 · 크기를 잰다(메모리에 담지 않고 흘린다). */
async function hashByDownload(url) {
  const res = await followHttps(url, { headers: UA }).catch((e) => fail(e.message));
  if (!res.ok) fail(`${url} answered ${res.status}`);
  const hash = createHash("sha256");
  let size = 0;
  for await (const chunk of res.body) {
    hash.update(chunk);
    size += chunk.length;
  }
  return { sha256: hash.digest("hex"), size, hash_from: "download" };
}

/** 지금 registry.toml 에서 `bundled` 목록을 읽는다. */
function bundledList() {
  const m = readFileSync(TOML, "utf8").match(/^bundled\s*=\s*\[([^\]]*)\]/m);
  if (!m) fail("registry.toml has no `bundled = [...]`");
  return [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
}

const githubReleases = new Map();
async function githubAsset(url) {
  const m = url.match(
    /^https:\/\/github\.com\/([^/]+)\/([^/]+)\/releases\/download\/([^/]+)\/([^/]+)$/,
  );
  if (!m) return null;
  const [, owner, repo, tag, name] = m;
  const key = `${owner}/${repo}@${tag}`;
  if (!githubReleases.has(key)) {
    const headers = { Accept: "application/vnd.github+json" };
    if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
    const r = await mustGet(
      `https://api.github.com/repos/${owner}/${repo}/releases/tags/${encodeURIComponent(tag)}`,
      { json: true, headers },
    );
    githubReleases.set(key, r.value.assets);
  }
  const asset = githubReleases.get(key).find((a) => a.name === decodeURIComponent(name));
  if (!asset) fail(`${url}: no such release asset`);
  const digest =
    typeof asset.digest === "string" ? asset.digest.match(/^sha256:([0-9a-f]{64})$/) : null;
  return digest ? { sha256: digest[1], size: asset.size, hash_from: "github-api" } : null;
}

async function vendorSha(url) {
  const r = await get(`${url}.sha256`);
  if (!r.ok) return null;
  const m = r.bytes
    .toString("utf8")
    .trim()
    .match(/^([0-9a-fA-F]{64})(\s|$)/);
  if (!m) return null;
  const head = await mustGet(url, { method: "HEAD" });
  const size = Number(head.headers.get("content-length"));
  if (!Number.isSafeInteger(size) || size <= 0) return null;
  return { sha256: m[1].toLowerCase(), size, hash_from: "vendor-sha256" };
}

const hashed = new Map();
async function hashFile(url, downloadAll) {
  if (!hashed.has(url)) hashed.set(url, await hashFileOnce(url, downloadAll));
  return hashed.get(url);
}

async function hashFileOnce(url, downloadAll) {
  if (!downloadAll) {
    const viaGithub = await githubAsset(url);
    if (viaGithub) return viaGithub;
    if (!url.startsWith("https://github.com/")) {
      const viaVendor = await vendorSha(url);
      if (viaVendor) return viaVendor;
    }
  }
  console.error(`  downloading ${url}`);
  return hashByDownload(url);
}

// ---- npm ----

async function npmArtifacts(pkg, version, specs) {
  const rows = [];
  for (const spec of specs) {
    const name = spec.name;
    const encoded = name.startsWith("@")
      ? `@${encodeURIComponent(name.slice(1))}`
      : encodeURIComponent(name);
    const meta = (
      await mustGet(`https://registry.npmjs.org/${encoded}/${spec.version ?? "latest"}`, {
        json: true,
      })
    ).value;
    if (meta.dependencies && Object.keys(meta.dependencies).length > 0) {
      fail(
        `${name}@${meta.version} has runtime dependencies — they would install unpinned (decision M4-D17)`,
      );
    }
    const { tarball, integrity } = meta.dist;
    const data = (await mustGet(tarball)).bytes;
    const sri = `sha512-${createHash("sha512").update(data).digest("base64")}`;
    if (sri !== integrity)
      fail(`${tarball}: integrity ${sri} does not match the registry's ${integrity}`);
    rows.push({
      package: pkg,
      version,
      target: "any",
      name: tarball.split("/").pop(),
      url: tarball,
      sha256: sha256(data),
      size: data.length,
      hash_from: "download",
      integrity,
      dist: name,
      dist_version: meta.version,
    });
  }
  return rows;
}

// ---- PyPI ----

const normalize = (name) => name.toLowerCase().replace(/[-_.]+/g, "-");

/** 휠 태그가 그 대상에서 설치되는가(pip 이 고르는 것 중 하나). `any` 는 모든 대상. */
function wheelTarget(filename) {
  const tags = filename
    .replace(/\.whl$/, "")
    .split("-")
    .slice(-3);
  const [, abi, plat] = tags;
  const plats = plat.split(".");
  if (plats.includes("any")) return ["any"];
  const out = new Set();
  for (const p of plats) {
    if (p.startsWith("macosx") && (p.endsWith("arm64") || p.endsWith("universal2")))
      out.add("darwin_arm64");
    if (p.startsWith("macosx") && (p.endsWith("x86_64") || p.endsWith("universal2")))
      out.add("darwin_x64");
    if (/^(manylinux|linux)/.test(p) && p.endsWith("x86_64")) out.add("linux_x64_gnu");
    if (/^(manylinux|linux)/.test(p) && p.endsWith("aarch64")) out.add("linux_arm64_gnu");
    if (p.startsWith("musllinux") && p.endsWith("x86_64")) out.add("linux_x64_musl");
    if (p === "win_amd64") out.add("win_x64");
  }
  // abi3 · none 이 아닌 CPython 전용 ABI 는 고르지 않는다(번들 패키지는 쓰지 않는다 — 생기면 여기서 멈춘다).
  if (abi !== "none" && abi !== "abi3" && out.size > 0)
    fail(`${filename}: CPython-specific wheel, not handled`);
  return [...out];
}

function parseVersion(v) {
  if (!/^\d+(\.\d+)*$/.test(v)) return null; // 프리릴리스 · 로컬 판은 고르지 않는다
  return v.split(".").map(Number);
}

function cmp(a, b) {
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const d = (a[i] ?? 0) - (b[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

function satisfies(version, spec) {
  const v = parseVersion(version);
  if (!v) return false;
  return spec
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean)
    .every((s) => {
      const m = s.match(/^(>=|<=|==|!=|>|<)\s*([\d.]+)$/);
      if (!m) fail(`version specifier ${s} is not handled`);
      const c = cmp(v, parseVersion(m[2]));
      return { ">=": c >= 0, "<=": c <= 0, "==": c === 0, "!=": c !== 0, ">": c > 0, "<": c < 0 }[
        m[1]
      ];
    });
}

/** `requires_dist` 한 줄 → { name, spec } (꼬리표가 `extra ==` 뿐이면 버린다). */
function parseRequirement(line) {
  const [req, marker] = line.split(";").map((s) => s.trim());
  if (marker) {
    if (/^extra\s*==/.test(marker)) return null;
    // 설치에 쓰는 Python 은 3.10 이상이다(basedpyright 의 `requires_python` — 결정 M4-D3). `python_version` 하나만 보는 꼬리표는
    // 3.10 과 3.14 에서 같게 갈릴 때만 판정한다 — 둘이 다르면 판에 따라 의존이 달라지므로 멈춘다.
    const pv = marker.match(/^python_version\s*(>=|<=|==|!=|>|<)\s*["']([\d.]+)["']$/);
    if (!pv) fail(`requirement marker not handled: ${line}`);
    const at = (v) => satisfies(v, `${pv[1]}${pv[2]}`);
    if (at("3.10") !== at("3.14"))
      fail(`requirement marker depends on the Python version: ${line}`);
    if (!at("3.10")) return null;
  }
  const m = req.match(/^([A-Za-z0-9][A-Za-z0-9._-]*)\s*(?:\[[^\]]*\])?\s*\(?([^)]*)\)?$/);
  if (!m) fail(`requirement not handled: ${line}`);
  return { name: m[1], spec: m[2].trim() };
}

async function pypiRelease(name, spec) {
  const meta = (
    await mustGet(`https://pypi.org/pypi/${encodeURIComponent(name)}/json`, { json: true })
  ).value;
  if (spec.startsWith("==")) {
    const version = spec.slice(2);
    if (!meta.releases[version]) fail(`${name}==${version} is not on PyPI`);
    return {
      version,
      info: (
        await mustGet(`https://pypi.org/pypi/${encodeURIComponent(name)}/${version}/json`, {
          json: true,
        })
      ).value,
    };
  }
  const versions = Object.keys(meta.releases)
    .filter(
      (v) =>
        satisfies(v, spec) &&
        meta.releases[v].some((f) => !f.yanked && f.packagetype === "bdist_wheel"),
    )
    .sort((a, b) => cmp(parseVersion(b), parseVersion(a)));
  if (versions.length === 0) fail(`no wheel of ${name} satisfies ${spec || "(any)"}`);
  const version = versions[0];
  return {
    version,
    info: (
      await mustGet(`https://pypi.org/pypi/${encodeURIComponent(name)}/${version}/json`, {
        json: true,
      })
    ).value,
  };
}

async function pypiArtifacts(pkg, version, specs) {
  const rows = [];
  const seen = new Set();
  const queue = specs.map((s) => ({ name: s.name, spec: s.version ? `==${s.version}` : "" }));
  while (queue.length > 0) {
    const { name, spec } = queue.shift();
    if (seen.has(normalize(name))) continue;
    seen.add(normalize(name));
    const { version: distVersion, info } = await pypiRelease(name, spec);
    const covered = new Set();
    for (const file of info.urls) {
      if (file.packagetype !== "bdist_wheel" || file.yanked) continue;
      for (const target of wheelTarget(file.filename)) {
        covered.add(target);
        rows.push({
          package: pkg,
          version,
          target,
          name: file.filename,
          url: file.url,
          sha256: file.digests.sha256,
          size: file.size,
          hash_from: "pypi",
          dist: info.info.name,
          dist_version: distVersion,
        });
      }
    }
    for (const t of TARGETS) {
      if (!covered.has("any") && !covered.has(t))
        fail(`${name} ${distVersion} has no wheel for ${t}`);
    }
    for (const line of info.info.requires_dist ?? []) {
      const req = parseRequirement(line);
      if (req) queue.push(req);
    }
  }
  return rows;
}

// ---- registry.toml ----

const q = (s) => JSON.stringify(s);

function writeToml(registry, bundled, artifacts) {
  const order = (a) =>
    [a.package, a.target === "any" ? "" : a.target, a.dist ?? "", a.name].join("\0");
  artifacts.sort((a, b) => (order(a) < order(b) ? -1 : order(a) > order(b) ? 1 : 0));
  let out = `# Pitwall 이 고정한 Mason registry 판과 기본 번들 패키지의 고정 해시(15-m4-plan.md 결정 M4-D16 · D17).
# 손으로 고치지 않는다 — node scripts/tools-registry-bump.mjs bump <태그> 가 다시 쓴다(CONTRIBUTING.md 「도구 registry」).
# \`mise run lint\` 가 번들 패키지의 (판, 대상, 파일)마다 해시가 있는지 본다(crates/pw-tools/src/pinned.rs 의 check).

# 기본 번들 패키지(결정 M4-D1 ~ D5). 내장 스냅숏에 묶이고 아래에 해시를 고정한다. 바꾸면 bump 를 다시 돌린다.
bundled = [${bundled.map(q).join(", ")}]

[registry]
version = ${q(registry.version)}
url = ${q(registry.url)}
sha256 = ${q(registry.sha256)}
json_sha256 = ${q(registry.json_sha256)}
`;
  for (const a of artifacts) {
    out += `\n[[artifact]]\n`;
    for (const key of [
      "package",
      "version",
      "target",
      "name",
      "url",
      "sha256",
      "size",
      "hash_from",
      "integrity",
      "dist",
      "dist_version",
    ]) {
      if (a[key] === undefined || a[key] === null) continue;
      out += `${key} = ${typeof a[key] === "number" ? a[key] : q(a[key])}\n`;
    }
  }
  writeFileSync(TOML, out);
  // 커밋 훅의 포매터(oxfmt)와 같은 꼴로 둔다 — 안 그러면 다음 bump 마다 형식만 바뀐 diff 가 남는다.
  run("pnpm", ["exec", "oxfmt", TOML]);
}

async function bump(tagArg, downloadAll) {
  let tag = tagArg;
  if (!tag || tag === "latest") {
    tag = (await mustGet(`https://api.github.com/repos/${REPO}/releases/latest`, { json: true }))
      .value.tag_name;
  }
  if (!/^\d{4}-\d{2}-\d{2}(-[a-z0-9]+)+$/.test(tag)) fail(`unexpected tag ${tag}`);
  const base = `https://github.com/${REPO}/releases/download/${tag}`;
  const dir = join(WORK, tag);
  mkdirSync(dir, { recursive: true });
  const sums = (await mustGet(`${base}/checksums.txt`)).bytes.toString("utf8");
  const sumOf = (name) => {
    const line = sums.split("\n").find((l) => l.trim().split(/\s+\*?/)[1] === name);
    const hash = line?.trim().split(/\s+/)[0]?.toLowerCase();
    if (!hash || !/^[0-9a-f]{64}$/.test(hash)) fail(`checksums.txt has no ${name}`);
    return hash;
  };
  const zipSha = sumOf("registry.json.zip");
  const jsonSha = sumOf("registry.json");
  const zip = (await mustGet(`${base}/registry.json.zip`)).bytes;
  if (sha256(zip) !== zipSha)
    fail(`registry.json.zip SHA-256 is ${sha256(zip)}, checksums.txt says ${zipSha}`);
  const zipPath = join(dir, "registry.json.zip");
  const jsonPath = join(dir, "registry.json");
  writeFileSync(zipPath, zip);
  tool("unzip", zipPath, jsonPath);
  const json = readFileSync(jsonPath);
  if (sha256(json) !== jsonSha)
    fail(`registry.json SHA-256 is ${sha256(json)}, checksums.txt says ${jsonSha}`);

  const bundled = bundledList();
  const all = JSON.parse(json.toString("utf8"));
  const packages = bundled.map(
    (name) =>
      all.find((p) => p.name === name) ?? fail(`bundled package ${name} is not in registry ${tag}`),
  );
  packages.sort((a, b) => (a.name < b.name ? -1 : 1));
  writeFileSync(SNAPSHOT, `${JSON.stringify({ registry: tag, packages }, null, 2)}\n`);
  mkdirSync(join(CRATE, "tests", "fixtures"), { recursive: true });
  copyFileSync(zipPath, FIXTURE);

  const plan = JSON.parse(tool("plan", SNAPSHOT));
  const artifacts = [];
  const done = new Set();
  for (const row of plan.rows) {
    if (row.unsupported) {
      console.error(
        `${row.package} ${row.target}: unsupported (${row.unsupported.reason}) — no pin`,
      );
      continue;
    }
    const r = row.resolved;
    const src = r.source;
    if (src.kind === "files") {
      for (const f of src.files) {
        console.error(`${r.name} ${row.target}: ${f.name}`);
        artifacts.push({
          package: r.name,
          version: r.version,
          target: row.target,
          name: f.name,
          url: f.url,
          ...(await hashFile(f.url, downloadAll)),
        });
      }
    } else if (!done.has(r.name)) {
      // npm · PyPI 는 대상마다 같은 패키지 목록이다 — 한 번만 푼다.
      done.add(r.name);
      const specs = [src.package, ...src.extra];
      console.error(
        `${r.name}: ${src.kind} ${specs.map((s) => `${s.name}@${s.version ?? "latest"}`).join(" ")}`,
      );
      artifacts.push(
        ...(src.kind === "npm"
          ? await npmArtifacts(r.name, r.version, specs)
          : await pypiArtifacts(r.name, r.version, specs)),
      );
    }
  }
  writeToml(
    { version: tag, url: `${base}/registry.json.zip`, sha256: zipSha, json_sha256: jsonSha },
    bundled,
    artifacts,
  );
  console.log(
    `registry ${tag}: ${packages.length} bundled packages, ${artifacts.length} pinned files`,
  );
  check();
}

function check() {
  const r = spawnSync(
    "cargo",
    ["run", "-q", "-p", "pw-tools", "--example", "registry_tool", "--", "check"],
    { cwd: root, stdio: "inherit" },
  );
  if (r.status !== 0) process.exit(r.status ?? 1);
}

const direct = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
const [cmd, ...rest] = direct ? process.argv.slice(2) : ["--imported"];
if (cmd === "--imported") {
  // 시험(scripts/tools-registry-bump.test.mjs)이 불러 쓴다.
} else if (cmd === "bump") {
  await bump(
    rest.find((a) => !a.startsWith("--")),
    rest.includes("--download-all"),
  );
} else if (cmd === "check") {
  check();
} else {
  console.error(
    "usage: node scripts/tools-registry-bump.mjs bump [<tag>|latest] [--download-all] | check",
  );
  process.exit(2);
}
