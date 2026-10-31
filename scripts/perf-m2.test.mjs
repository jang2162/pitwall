// scripts/perf-m2.mjs 를 가짜 mise · ioreg 로 돌려 종료 코드 · 복사 이름 · 덮어쓰기 · 잠금 확인을 본다(앱은 띄우지 않는다).
// 실행: node --test scripts/perf-m2.test.mjs   (`mise run test` 에 든다)

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

const script = resolve(import.meta.dirname, "perf-m2.mjs");

// 가짜 mise · ioreg 는 node 스크립트다 — 셸 스크립트(#!/bin/sh + chmod)는 Windows 에서 실행되지 않는다.
// 명령은 JSON 배열로 넘긴다(perf-m2.mjs 의 명령 변수). node 경로의 공백도 안전하다.
// 가짜 mise: `--out <폴더>` 에 result.json · summary.md 를 쓰고, 단계(git · graph-project)별 종료 코드를 환경 변수로 받는다.
const fakeMise = `
import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
const args = process.argv.slice(2);
const kind = args.join(" ").includes("--project") ? "graph" : "git";
const out = args[args.indexOf("--out") + 1];
mkdirSync(out, { recursive: true });
writeFileSync(join(out, "result.json"), JSON.stringify({ kind }) + "\\n");
writeFileSync(join(out, "summary.md"), "summary " + kind + "\\n");
appendFileSync(process.env.FAKE_LOG, kind + " " + args.join(" ") + "\\n");
process.exit(Number((kind === "git" ? process.env.FAKE_GIT : process.env.FAKE_GRAPH) ?? 0));
`;
const unlockedPlist = `console.log("<key>IOConsoleUsers</key>");`;
const lockedPlist = `console.log("<key>CGSSessionScreenIsLocked</key><true/>");`;
// 첫 호출은 잠기지 않음, 둘째 호출부터 잠김(스크립트 옆의 seen 파일로 구분).
const flipPlist = `
import { existsSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
const seen = join(dirname(process.argv[1]), "seen");
if (existsSync(seen)) console.log("<key>CGSSessionScreenIsLocked</key><true/>");
else writeFileSync(seen, "");
`;

function setup() {
  const dir = mkdtempSync(join(tmpdir(), "perf-m2-test-"));
  const put = (name, body) => {
    const p = join(dir, name);
    writeFileSync(p, body);
    return JSON.stringify([process.execPath, p]);
  };
  return {
    dir,
    reports: join(dir, "reports"),
    log: join(dir, "log"),
    mise: put("mise.mjs", fakeMise),
    unlocked: put("ioreg-ok.mjs", unlockedPlist),
    locked: put("ioreg-locked.mjs", lockedPlist),
    flip: put("ioreg-flip.mjs", flipPlist),
  };
}

function run(t, env, args = []) {
  return spawnSync("node", [script, ...args], {
    cwd: t.dir,
    encoding: "utf8",
    env: {
      ...process.env,
      PERF_M2_MISE: t.mise,
      PERF_M2_IOREG: t.unlocked,
      PERF_M2_REPORTS: t.reports,
      FAKE_LOG: t.log,
      ...env,
    },
  });
}

const copied = (t) => readdirSync(t.reports).sort();

test("exits 0 and copies both results when both steps succeed", () => {
  const t = setup();
  const r = run(t, { PERF_M2_STAMP: "120000" });
  assert.equal(r.status, 0, r.stderr);
  assert.equal(copied(t).length, 4);
  assert.ok(copied(t).every((n) => n.endsWith("-120000.json") || n.endsWith("-120000.summary.md")));
});

test("passes exit 3 through after copying both results", () => {
  const t = setup();
  const r = run(t, { FAKE_GIT: "3", PERF_M2_STAMP: "120000" });
  assert.equal(r.status, 3, r.stderr);
  assert.equal(copied(t).length, 4, "the graph project step still ran");
  assert.match(r.stderr, /git: too few valid runs/);
});

test("stops at the first failing step with its exit code and name", () => {
  const t = setup();
  const r = run(t, { FAKE_GIT: "1" });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /git failed \(exit 1\)/);
  assert.equal(readdirSync(t.dir).includes("reports"), false, "nothing was copied");
  assert.equal(
    readFileSync(t.log, "utf8").split("\n").filter(Boolean).length,
    1,
    "the graph step did not run",
  );
});

test("a second run the same day does not overwrite the first", () => {
  const t = setup();
  assert.equal(run(t, { PERF_M2_STAMP: "120000" }).status, 0);
  assert.equal(run(t, { PERF_M2_STAMP: "120001" }).status, 0);
  assert.equal(copied(t).length, 8);
});

test("refuses to overwrite a copy with the same name", () => {
  const t = setup();
  assert.equal(run(t, { PERF_M2_STAMP: "120000" }).status, 0);
  const again = run(t, { PERF_M2_STAMP: "120000" });
  assert.equal(again.status, 1);
  assert.match(again.stderr, /cannot copy/);
});

test("checks the screen lock before every step", () => {
  const t = setup();
  const r = run(t, { PERF_M2_IOREG: t.locked });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /screen is locked/);
  assert.equal(readdirSync(t.dir).includes("log"), false, "no step was started");
});

test("a lock that appears between the steps stops the second step", () => {
  const t = setup();
  const r = run(t, { PERF_M2_IOREG: t.flip });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /not starting graph-project/);
  assert.equal(readFileSync(t.log, "utf8").split("\n").filter(Boolean).length, 1);
});

test("refuses to start on a non-darwin platform and says so", () => {
  const t = setup();
  const env = { ...process.env, PERF_M2_PLATFORM: "win32", PERF_M2_MISE: t.mise, FAKE_LOG: t.log };
  delete env.PERF_M2_IOREG;
  const r = spawnSync("node", [script], { cwd: t.dir, encoding: "utf8", env });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /macOS only/);
  assert.equal(readdirSync(t.dir).includes("log"), false, "no step was started");
});

test("--dry-run starts nothing and creates no folder", () => {
  const t = setup();
  const r = run(t, { PERF_M2_IOREG: t.locked }, ["--dry-run", "--short"]);
  assert.equal(r.status, 0);
  assert.match(r.stdout, /--runs 1 --warmup 0/);
  assert.equal(readdirSync(t.dir).includes("reports"), false);
});
