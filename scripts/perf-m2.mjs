// M2-22 실기 측정을 한 번에 돌리고 결과를 보고서 폴더로 옮긴다(앱을 앞으로 띄우므로 사람이 기기를 비웠을 때만).
//
//   node scripts/perf-m2.mjs [--short] [--dry-run]     (`mise run perf-m2 -- --short`)
//
// 차례: ① git 시나리오(`mise run perf-git`) ② 콜드 스타트 · 유휴 메모리를 10만 커밋 저장소 프로젝트로(`mise run perf --
// --project target/perf-git/graph`). `--short` 는 둘 다 `--runs 1 --warmup 0`(끝까지 도는지만 본다 — 옮긴 파일 이름에 `-short`).
// 끝나면 각 `result.json` · `summary.md` 를 `docs/superpowers/reports/<날짜>-m2-perf-<종류>[-short].json|.summary.md` 로
// 복사한다(커밋은 사람이). 측정 방법은 CONTRIBUTING.md 「성능 기준선」, 절차는 docs/ops/m2-realcheck.md.
// 화면이 잠겨 있으면 단계를 시작하지 않는다(앱을 띄우기 직전마다 본다) — 이 스크립트는 OS 키 입력을 보내지 않는다.
// 같은 날 다시 돌려도 앞 결과를 덮지 않는다: 출력 폴더와 복사본 이름에 시작 시각이 붙는다.
// 종료 코드: 0 — 두 단계 모두 0. 3 — 결과는 복사했지만 어느 단계든 유효한 회가 적었다(pw-perf 의 3). 그 밖 — 그 단계의 코드
// 그대로(신호로 끝났으면 1)이고 뒤 단계는 돌지 않는다. 시험(scripts/perf-m2.test.mjs)을 위해 환경 변수로 하위 명령을 바꾼다:
// PERF_M2_MISE(mise) · PERF_M2_IOREG(화면 잠금을 읽는 명령) · PERF_M2_REPORTS(복사 폴더) · PERF_M2_STAMP(시각).

import { spawnSync } from "node:child_process";
import { constants, copyFileSync, existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const short = process.argv.includes("--short");
const dry = process.argv.includes("--dry-run");
const now = new Date();
const pad = (n) => String(n).padStart(2, "0");
const date = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
const stamp =
  process.env.PERF_M2_STAMP ??
  `${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
const suffix = short ? "-short" : "";
const reports = process.env.PERF_M2_REPORTS ?? "docs/superpowers/reports";
const mise = process.env.PERF_M2_MISE ?? "mise";
const ioreg = (process.env.PERF_M2_IOREG ?? "ioreg -n Root -d1 -a").split(" ");
const extra = short ? ["--runs", "1", "--warmup", "0"] : [];

const stepOf = (kind, args) => {
  const out = `target/perf/m2-${kind}${suffix}-${date}-${stamp}`;
  return { kind, out, args: [...args, "--out", out, ...extra] };
};
const steps = [
  stepOf("git", ["run", "perf-git", "--"]),
  stepOf("graph-project", ["run", "perf", "--", "--project", "target/perf-git/graph"]),
];

// 화면이 잠겼으면 true. 읽지 못하면 안 잠겼다고 보지 않고 멈춘다.
const locked = () => {
  const r = spawnSync(ioreg[0], ioreg.slice(1), { encoding: "utf8" });
  if (r.status !== 0) {
    console.error(
      `perf-m2: cannot read the screen lock state (${ioreg.join(" ")} exited ${r.status})`,
    );
    process.exit(1);
  }
  return /CGSSessionScreenIsLocked<\/key>\s*<true\/>/.test(r.stdout ?? "");
};

let worst = 0;
for (const step of steps) {
  console.log(`perf-m2: ${mise} ${step.args.join(" ")}`);
  if (dry) continue;
  if (locked()) {
    console.error(
      `perf-m2: the screen is locked — not starting ${step.kind}; unlock it and leave the device alone`,
    );
    process.exit(1);
  }
  const run = spawnSync(mise, step.args, { stdio: "inherit" });
  // 3 = 결과는 썼지만 유효한 회가 적다. 결과는 옮기고, 끝에서 3 으로 끝낸다.
  if (run.status !== 0 && run.status !== 3) {
    console.error(
      `perf-m2: ${step.kind} failed (exit ${run.status ?? `signal ${run.signal}`}); results already written stay in ${step.out}`,
    );
    process.exit(run.status ?? 1);
  }
  mkdirSync(reports, { recursive: true });
  for (const [from, ext] of [
    ["result.json", "json"],
    ["summary.md", "summary.md"],
  ]) {
    const src = join(step.out, from);
    if (!existsSync(src)) {
      console.error(`perf-m2: ${step.kind}: ${src} was not written`);
      process.exit(1);
    }
    const dst = join(reports, `${date}-m2-perf-${step.kind}${suffix}-${stamp}.${ext}`);
    try {
      copyFileSync(src, dst, constants.COPYFILE_EXCL);
    } catch (e) {
      console.error(`perf-m2: ${step.kind}: cannot copy ${src} to ${dst}: ${e.message}`);
      process.exit(1);
    }
    console.log(`perf-m2: ${src} → ${dst}`);
  }
  if (run.status === 3) {
    worst = 3;
    console.error(
      `perf-m2: ${step.kind}: too few valid runs (exit 3) — see the warning in its summary`,
    );
  }
}
process.exitCode = worst;
