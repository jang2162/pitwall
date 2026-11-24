// scripts/relay-build.mjs 의 판정(`file` 결과 읽기 · 타깃 표 · 인자)을 빌드 없이 본다.
// 실행: node --test scripts/relay-build.test.mjs   (`mise run test` 에 든다)

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";

import { TARGETS, buildCommand, checkFile, hostTarget, parseArgs } from "./relay-build.mjs";

const STATIC_X86 =
  "ELF 64-bit LSB executable, x86-64, version 1 (SYSV), statically linked, BuildID[sha1]=ab, stripped";
const STATIC_PIE_ARM =
  "ELF 64-bit LSB pie executable, ARM aarch64, version 1 (SYSV), static-pie linked, BuildID[sha1]=cd, stripped";
const DYNAMIC_X86 =
  "ELF 64-bit LSB pie executable, x86-64, version 1 (SYSV), dynamically linked, interpreter /lib64/ld-linux-x86-64.so.2, stripped";
const UNIVERSAL =
  "Mach-O universal binary with 2 architectures: [x86_64:Mach-O 64-bit executable x86_64] [arm64:Mach-O 64-bit executable arm64]";

test("linux binaries must be static and of their arch", () => {
  assert.equal(checkFile("x86_64-unknown-linux-musl", STATIC_X86), null);
  assert.equal(checkFile("aarch64-unknown-linux-musl", STATIC_PIE_ARM), null);
  assert.match(checkFile("x86_64-unknown-linux-musl", DYNAMIC_X86), /not statically linked/);
  assert.match(checkFile("aarch64-unknown-linux-musl", STATIC_X86), /not aarch64/);
  assert.match(checkFile("x86_64-unknown-linux-musl", UNIVERSAL), /not an ELF/);
  // 정적이라고 적혀도 로더가 있으면 거절.
  assert.match(
    checkFile("x86_64-unknown-linux-musl", `${STATIC_X86}, interpreter /lib/ld-musl-x86_64.so.1`),
    /dynamic loader/,
  );
});

test("macOS binaries must be one arch, not universal", () => {
  assert.equal(checkFile("aarch64-apple-darwin", "Mach-O 64-bit executable arm64"), null);
  assert.equal(checkFile("x86_64-apple-darwin", "Mach-O 64-bit executable x86_64"), null);
  assert.match(checkFile("aarch64-apple-darwin", UNIVERSAL), /universal/);
  assert.match(checkFile("x86_64-apple-darwin", "Mach-O 64-bit executable arm64"), /not x86_64/);
  assert.match(checkFile("aarch64-apple-darwin", STATIC_X86), /not a Mach-O/);
});

test("the target table matches pw_release::relay::PLATFORMS", () => {
  const source = readFileSync(
    resolve(import.meta.dirname, "../crates/pw-release/src/relay.rs"),
    "utf8",
  );
  const rust = [...source.matchAll(/asset: "([^"]+)",\s*rust_target: "([^"]+)"/g)].map((m) => [
    m[2],
    m[1],
  ]);
  assert.equal(rust.length, 4);
  assert.deepEqual(
    rust.sort(),
    Object.entries(TARGETS)
      .map(([target, spec]) => [target, spec.platform])
      .sort(),
  );
});

test("the host target and arguments", () => {
  assert.equal(hostTarget("darwin", "arm64"), "aarch64-apple-darwin");
  assert.equal(hostTarget("darwin", "x64"), "x86_64-apple-darwin");
  assert.equal(hostTarget("linux", "x64"), "x86_64-unknown-linux-musl");
  assert.equal(hostTarget("linux", "arm64"), "aarch64-unknown-linux-musl");
  assert.throws(() => hostTarget("win32", "x64"), /no relay target/);
  const host = "x86_64-unknown-linux-musl";
  assert.deepEqual(parseArgs([], host).targets, [host]);
  const options = parseArgs(
    ["--target", "aarch64-unknown-linux-musl", "--complete", "--out", "/tmp/x"],
    host,
  );
  assert.deepEqual(options.targets, ["aarch64-unknown-linux-musl"]);
  assert.equal(options.complete, true);
  assert.equal(options.out, "/tmp/x");
  assert.throws(() => parseArgs(["--target", "x86_64-unknown-linux-gnu"], host), /unknown target/);
  assert.throws(() => parseArgs(["--bogus"], host), /unknown argument/);
});

test("linux builds pack relative relocations, macOS builds do not get the linux linker flags", () => {
  // x86_64 musl 은 static-pie 라 RELR 없이는 `.rela.dyn` 이 4 MB 넘게 붙어 크기 상한(M7-P13)을 넘었다(CI run 45299304602).
  for (const target of ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"]) {
    const { args, env } = buildCommand(target);
    assert.deepEqual(args.slice(-3), ["--", "-C", "link-arg=-Wl,-z,pack-relative-relocs"], target);
    assert.equal(args[0], "rustc");
    assert.equal(args[args.indexOf("--target") + 1], target);
    assert.equal(env[`CC_${target.replaceAll("-", "_")}`], "musl-gcc");
  }
  for (const target of ["aarch64-apple-darwin", "x86_64-apple-darwin"]) {
    const { args, env } = buildCommand(target);
    assert.ok(!args.includes("--"), target);
    assert.deepEqual(env, {});
  }
});
