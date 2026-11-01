// scripts/lsp-live.mjs 의 판정(하한 · 0개 · 돈 수와 고른 수) · 설정 읽기 · 받기 확인을 실 서버 없이 본다.
// 실행: node --test scripts/lsp-live.test.mjs   (`mise run test` 에 든다)

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";

import {
  TARGETS,
  capabilityTest,
  exitCode,
  filterExpr,
  judge,
  lockIntegrity,
  parseToml,
  readConfig,
  runCount,
  rustTarget,
  selectedTests,
} from "./lsp-live.mjs";

const configText = readFileSync(resolve(import.meta.dirname, "lsp-live.toml"), "utf8");

test("the config has a floor, capabilities and a pinned download for every server and nothing else", () => {
  const config = readConfig(configText);
  assert.deepEqual(Object.keys(config.floors).sort(), Object.keys(TARGETS).sort());
  assert.deepEqual(Object.keys(config.capabilities).sort(), Object.keys(TARGETS).sort());
  for (const name of Object.keys(TARGETS)) {
    assert.ok(config.floors[name] >= 1, `${name}: M4-01 sets every floor to at least 1`);
    assert.ok(config.capabilities[name].length > 0, `${name}: expected capabilities`);
  }
  const ra = config.download.rust_analyzer;
  for (const target of [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
  ])
    assert.match(ra[target], /^[0-9a-f]{64}$/, target);
  const ts = config.download.typescript_language_server;
  assert.match(ts.integrity, /^sha512-/);
  assert.match(ts["typescript-integrity"], /^sha512-/);
  assert.throws(
    () => readConfig("[floors]\nrust_analyzer = 1\n"),
    /has no \[capabilities\] rust_analyzer/,
  );
  assert.throws(
    () => readConfig(configText.replace(/^typescript_language_server = 1$/m, "")),
    /has no \[floors\] typescript_language_server/,
  );
  assert.throws(
    () => readConfig(configText.replace(/^\[floors\]$/m, "[floors]\nmystery = 1")),
    /unknown server mystery/,
  );
});

test("the TOML subset reads tables, numbers, strings, arrays and comments", () => {
  const parsed = parseToml(
    '# top\n[a]\nn = 3 # three\ns = "x # not a comment"\nlist = ["p", "q"]\nempty = []\n[a.b]\nk-1 = "v"\n',
  );
  assert.deepEqual(parsed, {
    a: { n: 3, s: "x # not a comment", list: ["p", "q"], empty: [], b: { "k-1": "v" } },
  });
  assert.throws(() => parseToml("[a]\nn = 1\nn = 2"), /twice/);
  assert.throws(() => parseToml("[a]\nn = true"), /unsupported value/);
  assert.throws(() => parseToml("[a]\nlist = [1]"), /must be strings/);
  assert.throws(() => parseToml("just words"), /expected/);
});

const listed = {
  "rust-suites": {
    "pw-lsp::live_lsp": {
      testcases: {
        live_lsp_rust_analyzer_initialize_and_shutdown: {
          ignored: false,
          "filter-match": { status: "matches" },
        },
      },
    },
    "pw-lsp": {
      testcases: {
        "live::tests::names_follow_the_filter_and_env_rules": {
          ignored: false,
          "filter-match": { status: "mismatch", reason: "expression" },
        },
      },
    },
  },
};

test("the filter only matches names that start with live_lsp_<server>_", () => {
  const expr = filterExpr("live_lsp_rust_analyzer");
  assert.equal(expr, "test(/(^|::)live_lsp_rust_analyzer_/)");
  const pattern = new RegExp(expr.slice("test(/".length, -"/)".length));
  assert.ok(pattern.test("live_lsp_rust_analyzer_initialize_and_shutdown"));
  assert.ok(pattern.test("navigation::tests::live_lsp_rust_analyzer_definition"));
  assert.ok(!pattern.test("tests::the_live_lsp_rust_analyzer_name"));
  assert.ok(!pattern.test("live_lsp_rust_analyzerx_y"));
  assert.deepEqual(selectedTests(listed), [
    "pw-lsp::live_lsp live_lsp_rust_analyzer_initialize_and_shutdown",
  ]);
});

test("a run below the floor, with no tests, failed or miscounted fails", () => {
  const selected = ["pw-lsp::live_lsp live_lsp_rust_analyzer_initialize_and_shutdown"];
  const base = { server: "rust_analyzer", selected, run: 1, exitCode: 0 };
  assert.equal(judge({ ...base, floor: 1 }).ok, true);
  // 하한을 실행 개수 + 1 로 올리면 실패한다.
  const above = judge({ ...base, floor: 2 });
  assert.equal(above.ok, false);
  assert.match(above.message, /below the floor/);
  // 필터가 0개를 고르면 하한이 0 이어도 실패한다.
  const none = judge({ ...base, selected: [], run: 0, floor: 0, exitCode: 4 });
  assert.equal(none.ok, false);
  assert.match(none.message, /selected no tests/);
  assert.match(judge({ ...base, floor: 1, exitCode: 100 }).message, /nextest failed/);
  assert.match(judge({ ...base, floor: 1, run: null }).message, /no Summary line/);
  assert.match(judge({ ...base, floor: 0, run: 2 }).message, /ran 2 but the filter selected 1/);
});

test("the capability check must be among the selected tests", () => {
  const required = capabilityTest("rust_analyzer");
  assert.equal(required, "live_lsp_rust_analyzer_initialize_and_shutdown");
  const with_ = [`pw-lsp::live_lsp ${required}`, "pw-lsp::live_lsp live_lsp_rust_analyzer_hover"];
  const base = { server: "rust_analyzer", run: 2, floor: 2, exitCode: 0, required };
  assert.equal(judge({ ...base, selected: with_ }).ok, true);
  // 하한은 넘지만 능력 판정 시험이 없다(지워졌거나 이름이 바뀜) — 실패.
  const without = [
    "pw-lsp::live_lsp live_lsp_rust_analyzer_hover",
    "pw-lsp::live_lsp live_lsp_rust_analyzer_definition",
  ];
  const missing = judge({ ...base, selected: without });
  assert.equal(missing.ok, false);
  assert.match(missing.message, /capability check/);
  // 이름 조각 끝이 정확히 같아야 한다.
  const lookalike = [
    `pw-lsp::live_lsp ${required}_twice`,
    "pw-lsp::live_lsp live_lsp_rust_analyzer_hover",
  ];
  assert.equal(judge({ ...base, selected: lookalike }).ok, false);
});

test("run counts, exit codes, targets and lockfile integrity", () => {
  assert.equal(runCount("     Summary [   0.355s] 1 test run: 1 passed, 33 skipped"), 1);
  assert.equal(exitCode([{ ok: true, skipped: true }]), 3);
  assert.equal(exitCode([{ ok: true }, { ok: false }]), 1);
  assert.equal(rustTarget("darwin", "arm64"), "aarch64-apple-darwin");
  assert.equal(rustTarget("linux", "x64"), "x86_64-unknown-linux-gnu");
  assert.equal(rustTarget("win32", "x64"), null);
  const lock = [
    "packages:",
    "",
    "  typescript-language-server@6.0.2:",
    "    resolution: {integrity: sha512-AAA==}",
    "    engines: {node: '>=22'}",
    "",
    "  typescript@6.0.3:",
    "    resolution: {integrity: sha512-BBB==}",
  ].join("\n");
  assert.equal(lockIntegrity(lock, "typescript-language-server", "6.0.2"), "sha512-AAA==");
  assert.equal(lockIntegrity(lock, "typescript", "6.0.3"), "sha512-BBB==");
  assert.equal(lockIntegrity(lock, "typescript", "6.0.2"), null);
});
