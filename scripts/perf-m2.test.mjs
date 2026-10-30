// scripts/perf-m2.mjs 를 가짜 mise · ioreg 로 돌려 종료 코드 · 복사 이름 · 덮어쓰기 · 잠금 확인을 본다(앱은 띄우지 않는다).
// 실행: node --test scripts/perf-m2.test.mjs   (`mise run test` 에 든다)

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

const script = resolve(import.meta.dirname, "perf-m2.mjs");

// 가짜 mise: `--out <폴더>` 에 result.json · summary.md 를 쓰고, 단계(git · graph-project)별 종료 코드를 환경 변수로 받는다.
const fakeMise = `#!/bin/sh
case "$*" in *"--project"*) kind=graph;; *) kind=git;; esac
out=""; prev=""
for a in "$@"; do [ "$prev" = "--out" ] && out="$a"; prev="$a"; done
mkdir -p "$out"; echo "{\\"kind\\":\\"$kind\\"}" > "$out/result.json"; echo "summary $kind" > "$out/summary.md"
echo "$kind $*" >> "$FAKE_LOG"
if [ "$kind" = git ]; then exit "\${FAKE_GIT:-0}"; else exit "\${FAKE_GRAPH:-0}"; fi
`;
const unlockedPlist = `#!/bin/sh\necho '<key>IOConsoleUsers</key>'\n`;
const lockedPlist = `#!/bin/sh\necho '<key>CGSSessionScreenIsLocked</key><true/>'\n`;

function setup() {
  const dir = mkdtempSync(join(tmpdir(), "perf-m2-test-"));
  const put = (name, body) => {
    const p = join(dir, name);
    writeFileSync(p, body);
    chmodSync(p, 0o755);
    return p;
  };
  return {
    dir,
    reports: join(dir, "reports"),
    log: join(dir, "log"),
    mise: put("mise", fakeMise),
    unlocked: put("ioreg-ok", unlockedPlist),
    locked: put("ioreg-locked", lockedPlist),
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
  // 첫 호출은 잠기지 않음, 둘째 호출부터 잠김.
  const flip = join(t.dir, "ioreg-flip");
  writeFileSync(
    flip,
    `#!/bin/sh\nif [ -e "${t.dir}/seen" ]; then echo '<key>CGSSessionScreenIsLocked</key><true/>'; else touch "${t.dir}/seen"; fi\n`,
  );
  chmodSync(flip, 0o755);
  const r = run(t, { PERF_M2_IOREG: flip });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /not starting graph-project/);
  assert.equal(readFileSync(t.log, "utf8").split("\n").filter(Boolean).length, 1);
});

test("--dry-run starts nothing and creates no folder", () => {
  const t = setup();
  const r = run(t, { PERF_M2_IOREG: t.locked }, ["--dry-run", "--short"]);
  assert.equal(r.status, 0);
  assert.match(r.stdout, /--runs 1 --warmup 0/);
  assert.equal(readdirSync(t.dir).includes("reports"), false);
});
