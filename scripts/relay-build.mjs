// Relay 개발 빌드 · 배포 빌드(18-m7-plan M7-05 ① ② ④). `mise run relay-build` · CI 의 `relay-live` 잡 · 릴리스 워크플로의 `relay`
// 잡이 같은 이 스크립트를 부른다.
//
//   node scripts/relay-build.mjs [--target <타깃>]… [--out <폴더>] [--complete]
//
// 타깃: x86_64-unknown-linux-musl · aarch64-unknown-linux-musl · aarch64-apple-darwin · x86_64-apple-darwin(macOS 는 아키텍처별 —
// 결정 M7-D4 2026-11-23 바뀜). 없으면 이 기기의 플랫폼 하나(Linux 는 같은 아키텍처의 musl). 타깃마다:
//   1. `cargo rustc --profile relay -p pitwall-relay --target <타깃>`(`buildCommand`) — `relay` 프로필은 워크스페이스 Cargo.toml(크기를
//      줄이는 설정), Linux 는 링크에 RELR 을 더한다.
//   2. `<out>/pitwall-relay-<판>-<플랫폼>` 으로 복사하고 `file` 결과를 적는다. Linux 는 정적 링크와 아키텍처, macOS 는 그
//      아키텍처 하나의 Mach-O(유니버설 아님)인지 본다 — 아니면 실패한다(checkFile).
//   3. `pw-release relay-describe` 로 항목(`<out>/entries/<플랫폼>.json` — 이름 · sha256 · 크기)을 쓴다. **크기가 상한(M7-P13 —
//      `pw_release::relay::MAX_BINARY_BYTES`)을 넘으면 실패한다.** 크기는 빌드마다 출력에 남는다.
// 끝에 `pw-release relay-manifest` 로 `<out>/relay-manifest.json`(서명 없음)을 쓴다. `--complete` 는 네 플랫폼이 다 있어야 한다
// (릴리스). 서명은 이 스크립트가 하지 않는다 — 릴리스의 `sign-updater` 잡만 운영 키로 한다.
//
// 개발 Desktop(debug)은 `<out>`(기본 `target/relay-dist/` — `pw_release::relay::DEV_DIST_DIR`)의 바이너리와 서명 없는 매니페스트를
// 원격에 올린다(결정 M7-D5). release Desktop 은 이 폴더를 보지 않는다.
//
// musl 타깃은 C 코드(tree-sitter 문법 · sqlite · ring)를 musl 로 컴파일해야 한다 — Linux 에서 `musl-tools`(musl-gcc)를 깔고 같은
// 아키텍처 타깃을 빌드한다(CI 는 x86_64 · arm64 runner). macOS 에서 Linux 로 교차 빌드하는 길은 두지 않았다.

import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));

// Rust 타깃 → 자산 이름의 플랫폼 조각(`pw_release::relay::PLATFORMS` 와 같은 표 — 시험이 견준다).
export const TARGETS = {
  "x86_64-unknown-linux-musl": { platform: "linux-x86_64", os: "linux", arch: "x86-64" },
  "aarch64-unknown-linux-musl": { platform: "linux-aarch64", os: "linux", arch: "aarch64" },
  "aarch64-apple-darwin": { platform: "macos-aarch64", os: "macos", arch: "arm64" },
  "x86_64-apple-darwin": { platform: "macos-x86_64", os: "macos", arch: "x86_64" },
};

/** 이 기기의 기본 타깃. */
export function hostTarget(platform, arch) {
  if (platform === "darwin" && arch === "arm64") return "aarch64-apple-darwin";
  if (platform === "darwin" && arch === "x64") return "x86_64-apple-darwin";
  if (platform === "linux" && arch === "x64") return "x86_64-unknown-linux-musl";
  if (platform === "linux" && arch === "arm64") return "aarch64-unknown-linux-musl";
  throw new Error(
    `no relay target for ${platform} ${arch} (Relay runs on macOS and Linux — 18-m7-plan M7-D4)`,
  );
}

export function parseArgs(argv, host) {
  const options = { targets: [], out: join(root, "target", "relay-dist"), complete: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--target") {
      const target = argv[++i] ?? "";
      if (!TARGETS[target])
        throw new Error(`unknown target ${target} (${Object.keys(TARGETS).join(", ")})`);
      options.targets.push(target);
    } else if (arg === "--out") options.out = resolve(argv[++i] ?? "");
    else if (arg === "--complete") options.complete = true;
    else throw new Error(`unknown argument ${arg}`);
  }
  if (options.targets.length === 0) options.targets.push(host);
  return options;
}

/**
 * `file` 의 한 줄이 그 타깃의 배포물인가. Linux: ELF · 그 아키텍처 · 정적 링크(`statically linked` 또는 `static-pie linked` —
 * 동적 로더가 없어야 glibc 판과 무관하다). macOS: 그 아키텍처 하나의 Mach-O 실행 파일(유니버설이면 거절 — 원격에 두 아키텍처를
 * 올리지 않는다). 아니면 까닭 글, 맞으면 null.
 */
export function checkFile(target, output) {
  const spec = TARGETS[target];
  if (spec.os === "linux") {
    if (!output.includes("ELF")) return `not an ELF binary: ${output}`;
    if (!output.includes(spec.arch)) return `not ${spec.arch}: ${output}`;
    if (!/statically linked|static-pie linked/.test(output))
      return `not statically linked: ${output}`;
    if (/interpreter/.test(output)) return `has a dynamic loader: ${output}`;
    return null;
  }
  if (output.includes("universal")) return `a universal binary, not one arch: ${output}`;
  if (!output.startsWith("Mach-O 64-bit executable")) return `not a Mach-O executable: ${output}`;
  if (!output.trim().endsWith(spec.arch)) return `not ${spec.arch}: ${output}`;
  return null;
}

function run(command, args, options = {}) {
  console.log(`$ ${command} ${args.join(" ")}`);
  const result = spawnSync(command, args, {
    cwd: root,
    stdio: options.capture ? ["ignore", "pipe", "inherit"] : "inherit",
    encoding: "utf8",
    env: { ...process.env, ...options.env },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} ${args[0]} failed (exit ${result.status})`);
  return result.stdout ?? "";
}

/**
 * 그 타깃의 빌드 명령(`cargo` 의 인자와 더할 env). `cargo rustc` 로 Relay 바이너리 하나만 짓는다 — 뒤의 `--` 인자는 마지막 링크에만
 * 붙고 의존 크레이트는 `cargo build` 와 같다.
 *
 * Linux(musl): C 컴파일러는 musl-gcc 다(cc 크레이트가 기본으로 고르지 못하는 판이 있어 적는다). 그리고 상대 재배치를 RELR 로 싼다
 * (`-z pack-relative-relocs`). x86_64 musl 은 static-pie(주소 무작위화)라 포인터마다 `.rela.dyn` 에 24 바이트가 붙는데(Relay 에서 약
 * 17만 개 · 4.1 MB), RELR 은 그것을 수십 KB 로 줄인다 — 기능 · 프로필 · static-pie 는 그대로다(잰 값은 Cargo.toml 의 `relay` 프로필
 * 주석). musl 1.2.4 부터 static-pie 시작 코드가 RELR 을 푼다(Rust 의 자체 musl crt), 링커는 binutils 2.38 이상이 받는다.
 * static-pie 가 아닌 타깃(aarch64 musl)에는 쌀 것이 없다.
 */
export function buildCommand(target) {
  const args = ["rustc", "--locked", "--profile", "relay", "-p", "pitwall-relay"];
  args.push("--bin", "pitwall-relay", "--target", target);
  const env = {};
  if (target.endsWith("-linux-musl")) {
    env[`CC_${target.replaceAll("-", "_")}`] = "musl-gcc";
    args.push("--", "-C", "link-arg=-Wl,-z,pack-relative-relocs");
  }
  return { args, env };
}

function cargoBuild(target) {
  const { args, env } = buildCommand(target);
  run("cargo", args, { env });
  return join(root, "target", target, "relay", "pitwall-relay");
}

function version() {
  const text = readFileSync(join(root, "Cargo.toml"), "utf8");
  const match = /\[workspace\.package\][^[]*?\nversion\s*=\s*"([^"]+)"/.exec(text);
  if (!match) throw new Error("no [workspace.package] version in Cargo.toml");
  return match[1];
}

function buildOne(target, out) {
  const spec = TARGETS[target];
  const binary = cargoBuild(target);
  const name = `pitwall-relay-${version()}-${spec.platform}`;
  const dest = join(out, name);
  copyFileSync(binary, dest);
  const described = run("file", ["-b", dest], { capture: true }).trim();
  console.log(`file: ${name}: ${described}`);
  const problem = checkFile(target, described);
  if (problem) throw new Error(`${name}: ${problem}`);
  run("cargo", [
    "run",
    "-q",
    "--locked",
    "-p",
    "pw-release",
    "--",
    "relay-describe",
    "--file",
    dest,
    "--platform",
    spec.platform,
    "--out",
    join(out, "entries", `${spec.platform}.json`),
  ]);
}

function main(argv) {
  const options = parseArgs(argv, hostTarget(process.platform, process.arch));
  mkdirSync(join(options.out, "entries"), { recursive: true });
  // 앞 실행의 항목이 섞이지 않게 고른 타깃의 것만 새로 쓴다 — 다른 플랫폼 항목은 지운다(개발 매니페스트가 낡은 바이너리를 싣지 않게).
  for (const spec of Object.values(TARGETS)) {
    if (!options.targets.some((t) => TARGETS[t] === spec))
      rmSync(join(options.out, "entries", `${spec.platform}.json`), { force: true });
  }
  for (const target of options.targets) buildOne(target, options.out);
  run("cargo", [
    "run",
    "-q",
    "--locked",
    "-p",
    "pw-release",
    "--",
    "relay-manifest",
    "--entries",
    join(options.out, "entries"),
    "--out",
    join(options.out, "relay-manifest.json"),
    "--complete",
    String(options.complete),
  ]);
  console.log(`relay-build: wrote ${join(options.out, "relay-manifest.json")}`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`relay-build: ${e.message}`);
    process.exitCode = 1;
  }
}
