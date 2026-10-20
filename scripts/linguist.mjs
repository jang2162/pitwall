// GitHub Linguist 데이터(crates/pw-lang/linguist/)를 고정 커밋에서 받는다. 고정 커밋은 crates/pw-lang/linguist/VERSION 이다.
//
//   node scripts/linguist.mjs data            고정 커밋의 YAML · LICENSE 를 crates/pw-lang/linguist/ 에 받아 덮어쓴다
//   node scripts/linguist.mjs samples-test    고정 커밋의 samples/ 를 target/linguist-samples/<커밋>/ 에 받고(있으면 건너뛴다)
//                                             pw-lang 의 일치율 시험을 돌린다(`mise run lang-samples`)
//
// 버전을 올리는 순서는 CONTRIBUTING.md 「Linguist 데이터」 절.
// samples 는 git 부분 클론(blob 은 필요할 때만)으로 받는다 — 커밋 해시로 내용이 정해지므로 받은 것이 그 커밋의 것인지 따로
// 검사하지 않는다. GitHub 의 tarball 은 바이트가 늘 같다는 보장이 없어 쓰지 않는다.

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const dataDir = join(root, "crates", "pw-lang", "linguist");
const REPO = "https://github.com/github-linguist/linguist";
// 저장소에 넣는 파일. 바꾸면 build.rs 와 licenses/vendored.md 도 함께 고친다.
const FILES = [
  "lib/linguist/languages.yml",
  "lib/linguist/heuristics.yml",
  "lib/linguist/vendor.yml",
  "lib/linguist/documentation.yml",
  "lib/linguist/generic.yml",
  "lib/linguist/popular.yml",
  "LICENSE",
];

function version() {
  const [tag, commit] = readFileSync(join(dataDir, "VERSION"), "utf8").trim().split(/\s+/);
  if (!/^[0-9a-f]{40}$/.test(commit ?? "")) {
    throw new Error(
      `crates/pw-lang/linguist/VERSION: expected "<tag> <40-hex commit>", got ${tag} ${commit}`,
    );
  }
  return { tag, commit };
}

function git(args, cwd) {
  const r = spawnSync("git", args, { cwd, stdio: ["ignore", "inherit", "inherit"] });
  if (r.status !== 0) throw new Error(`git ${args.join(" ")} failed (${r.status})`);
}

async function data() {
  const { tag, commit } = version();
  for (const file of FILES) {
    const url = `https://raw.githubusercontent.com/github-linguist/linguist/${commit}/${file}`;
    const res = await fetch(url);
    if (!res.ok) throw new Error(`${url}: ${res.status}`);
    const name = file.split("/").pop();
    writeFileSync(join(dataDir, name), Buffer.from(await res.arrayBuffer()));
    console.log(`${name} ← ${tag} (${commit.slice(0, 12)})`);
  }
}

/** 고정 커밋의 samples/ 폴더 경로. 없으면 받는다. 받다가 멈추면 다음 실행이 처음부터 다시 받는다. */
function samples() {
  const { commit } = version();
  const dest = join(root, "target", "linguist-samples", commit);
  if (existsSync(join(dest, "samples"))) return join(dest, "samples");
  const tmp = `${dest}.partial`;
  rmSync(tmp, { recursive: true, force: true });
  mkdirSync(tmp, { recursive: true });
  git(["init", "-q"], tmp);
  git(["remote", "add", "origin", REPO], tmp);
  git(["fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", commit], tmp);
  git(["sparse-checkout", "set", "--no-cone", "/samples/"], tmp);
  git(["checkout", "-q", "FETCH_HEAD"], tmp);
  rmSync(join(tmp, ".git"), { recursive: true, force: true });
  rmSync(dest, { recursive: true, force: true });
  renameSync(tmp, dest);
  return join(dest, "samples");
}

function samplesTest() {
  const dir = samples();
  console.log(`linguist samples: ${dir}`);
  const r = spawnSync(
    "cargo",
    [
      "nextest",
      "run",
      "-p",
      "pw-lang",
      "--run-ignored",
      "only",
      "--no-tests=fail",
      "--no-capture",
      "-E",
      "test(=linguist_samples_match_rate)",
    ],
    { cwd: root, stdio: "inherit", env: { ...process.env, PITWALL_LINGUIST_SAMPLES: dir } },
  );
  process.exit(r.status ?? 1);
}

const command = process.argv[2];
if (command === "data") {
  await data();
} else if (command === "samples-test") {
  samplesTest();
} else {
  console.error("usage: node scripts/linguist.mjs data | samples-test");
  process.exit(2);
}
