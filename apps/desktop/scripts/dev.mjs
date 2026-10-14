// 데스크톱 앱을 개발 모드로 띄운다(`mise run dev`).
//
// 개발 서버 포트를 워크트리마다 다르게 정한다(docs/guideline/base/08-shared-resources.md).
// 저장소 루트의 절대 경로를 해시해 20000–29999 에서 고르므로 같은 워크트리는 늘 같은 포트를 받는다.
// 두 워크트리가 같은 포트를 받으면 vite 의 strictPort 가 시작을 실패시킨다 — 그때는
// PITWALL_DEV_PORT 를 직접 지정한다.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const appDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(appDir, "../..");

function worktreePort(root) {
  const hash = createHash("sha256").update(root).digest();
  return 20000 + (hash.readUInt32BE(0) % 10000);
}

const port = Number(process.env.PITWALL_DEV_PORT ?? worktreePort(repoRoot));
const devUrl = `http://127.0.0.1:${port}`;
console.log(`[pitwall dev] ${repoRoot} → ${devUrl}`);

// 실제 devUrl 은 여기서만 넘긴다. tauri.conf.json 의 devUrl(포트 0)은 쓸 수 없는 자리표시다 — 지우면 debug 빌드의
// generate_context! 가 ../dist 를 요구해 프론트엔드를 빌드하기 전에는 cargo build·clippy 가 실패하고,
// 실제 포트를 적어 두면 그 값이 다시 모든 워크트리의 공용 포트가 된다.
const result = spawnSync(
  "pnpm",
  [
    "exec",
    "tauri",
    "dev",
    "--config",
    JSON.stringify({ build: { devUrl } }),
    ...process.argv.slice(2),
  ],
  {
    cwd: appDir,
    stdio: "inherit",
    env: { ...process.env, PITWALL_DEV_PORT: String(port) },
    shell: process.platform === "win32",
  },
);
process.exit(result.status ?? 1);
