// scripts/dap-live.mjs 의 설정 읽기 · 필터 · 설치 결과 읽기를 실 어댑터 없이 본다(판정은 lsp-live.mjs 의 `judge` 그대로 —
// scripts/lsp-live.test.mjs 가 본다). 실행: node --test scripts/dap-live.test.mjs   (`mise run test` 에 든다)

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";

import { TARGETS, filterExpr, installedPath, readConfig } from "./dap-live.mjs";
import { judge } from "./lsp-live.mjs";

const configText = readFileSync(resolve(import.meta.dirname, "dap-live.toml"), "utf8");

test("the config has a floor and an installer package for every adapter and nothing else", () => {
  const config = readConfig(configText);
  assert.deepEqual(Object.keys(config.floors).sort(), Object.keys(TARGETS).sort());
  assert.deepEqual(Object.keys(config.install).sort(), Object.keys(TARGETS).sort());
  // 하한(17-m6-plan.md M6-19): 실제로 도는 시험 수 — 낮추면 시험이 조용히 빠질 수 있다.
  assert.deepEqual(config.floors, { codelldb: 2, js_debug: 2, debugpy: 2, java_debug: 3 });
  assert.equal(
    config.install.java_debug.share,
    "java-debug-adapter/com.microsoft.java.debug.plugin.jar",
  );
  assert.throws(
    () => readConfig(configText.replace("codelldb = 2", "codelldb = 0")),
    /positive \[floors\] codelldb/,
  );
  assert.throws(() => readConfig(`${configText}\n[floors.extra]\n`), /unknown adapter extra/);
  assert.throws(
    () => readConfig(configText.replace('bin = "debugpy-adapter"', 'share = "x"\nbin = "y"')),
    /exactly one of bin and share/,
  );
});

test("the filter picks the live tests of one adapter by name prefix", () => {
  assert.equal(filterExpr("js_debug"), "test(/(^|::)live_dap_js_debug_/)");
});

test("the install result gives the bin or the share and must be pinned", () => {
  const out = JSON.stringify({
    name: "java-debug-adapter",
    version: "0.59.0",
    checksum: "pinned",
    bins: {},
    shares: { "java-debug-adapter/com.microsoft.java.debug.plugin.jar": "/t/a.jar" },
  });
  assert.equal(
    installedPath(out, { share: "java-debug-adapter/com.microsoft.java.debug.plugin.jar" }),
    "/t/a.jar",
  );
  assert.throws(() => installedPath(out, { bin: "x" }), /gave no x/);
  assert.throws(
    () =>
      installedPath(out.replace('"pinned"', '"none"'), {
        share: "java-debug-adapter/com.microsoft.java.debug.plugin.jar",
      }),
    /without a pinned hash/,
  );
});

test("zero selected tests or a run below the floor fails", () => {
  const base = { server: "debugpy", floor: 2, exitCode: 0, required: null };
  assert.equal(judge({ ...base, selected: [], run: 0 }).ok, false);
  assert.equal(judge({ ...base, selected: ["a"], run: 1 }).ok, false);
  assert.equal(judge({ ...base, selected: ["a", "b"], run: 2 }).ok, true);
});
