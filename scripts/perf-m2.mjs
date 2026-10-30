// M2-22 실기 측정을 한 번에 돌리고 결과를 보고서 폴더로 옮긴다(앱을 앞으로 띄우므로 사람이 기기를 비웠을 때만).
//
//   node scripts/perf-m2.mjs [--short] [--dry-run]     (`mise run perf-m2 -- --short`)
//
// 차례: ① git 시나리오(`mise run perf-git`) ② 콜드 스타트 · 유휴 메모리를 10만 커밋 저장소 프로젝트로(`mise run perf --
// --project target/perf-git/graph`). `--short` 는 둘 다 `--runs 1 --warmup 0`(끝까지 도는지만 본다 — 옮긴 파일 이름에 `-short`).
// 끝나면 각 `result.json` · `summary.md` 를 `docs/superpowers/reports/<날짜>-m2-perf-<종류>[-short].json|.summary.md` 로
// 복사한다(커밋은 사람이). 측정 방법은 CONTRIBUTING.md 「성능 기준선」, 절차는 docs/ops/m2-realcheck.md.
// 화면이 잠겨 있으면 시작하지 않는다 — 이 스크립트는 OS 키 입력을 보내지 않는다.

import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const short = process.argv.includes("--short");
const dry = process.argv.includes("--dry-run");
const date = new Date().toISOString().slice(0, 10);
const suffix = short ? "-short" : "";
const reports = "docs/superpowers/reports";
const extra = short ? ["--runs", "1", "--warmup", "0"] : [];

const steps = [
  {
    kind: "git",
    out: `target/perf/m2-git${suffix}`,
    args: ["run", "perf-git", "--", "--out", `target/perf/m2-git${suffix}`, ...extra],
  },
  {
    kind: "graph-project",
    out: `target/perf/m2-graph-project${suffix}`,
    args: [
      "run",
      "perf",
      "--",
      "--project",
      "target/perf-git/graph",
      "--out",
      `target/perf/m2-graph-project${suffix}`,
      ...extra,
    ],
  },
];

if (!dry) {
  const lock = spawnSync("ioreg", ["-n", "Root", "-d1", "-a"], { encoding: "utf8" }).stdout ?? "";
  if (/CGSSessionScreenIsLocked<\/key>\s*<true\/>/.test(lock)) {
    console.error("perf-m2: the screen is locked — unlock it and leave the device alone");
    process.exit(1);
  }
}

mkdirSync(reports, { recursive: true });
for (const step of steps) {
  console.log(`perf-m2: mise ${step.args.join(" ")}`);
  if (dry) continue;
  const run = spawnSync("mise", step.args, { stdio: "inherit" });
  // 3 = 결과는 썼지만 유효한 회가 적다. 그래도 결과는 옮긴다.
  if (run.status !== 0 && run.status !== 3) {
    console.error(
      `perf-m2: ${step.kind} failed (exit ${run.status}); results already written stay in ${step.out}`,
    );
    process.exit(run.status ?? 1);
  }
  for (const [from, ext] of [
    ["result.json", "json"],
    ["summary.md", "summary.md"],
  ]) {
    const src = join(step.out, from);
    if (!existsSync(src)) {
      console.error(`perf-m2: ${src} was not written`);
      process.exit(1);
    }
    const dst = join(reports, `${date}-m2-perf-${step.kind}${suffix}.${ext}`);
    copyFileSync(src, dst);
    console.log(`perf-m2: ${src} → ${dst}`);
  }
  if (run.status === 3)
    console.error(
      `perf-m2: ${step.kind}: too few valid runs (exit 3) — see the warning in its summary`,
    );
}
