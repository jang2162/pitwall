// Git 로그 측정 하네스를 헤드리스 WebKit 에서 돌린다(git-log-bench.tsx 머리 주석) — 앱 창을 띄우지 않는다.
//
//   cargo run --release -p pw-git --features test-support --example synth_history -- <저장소> 100000 --commit-graph
//   cargo run --release -p pw-git --example log_timing -- <저장소> 1 --dump <폴더>/log-100k.json
//   pnpm --filter @pitwall/desktop exec vite build perf/git-log --config perf/git-log/vite.config.ts --outDir <폴더> --base ./
//   PLAYWRIGHT=<playwright-core 폴더> node apps/desktop/perf/git-log/run.mjs <폴더> [회수=5] [WebKit 실행 파일]
//
// `PAGE=scenario.html` 이면 하네스 대신 앱 안 측정 시나리오의 스모크(scenario-bench.tsx)를 연다. 덤프는 작은 저장소(`synth_history <저장소>
// 2000 --perf-fixture`)로도 된다 — `?data=` 없이 `log-100k.json` 을 읽으니 그 이름으로 둔다.
//
// `vite build` 는 `--emptyOutDir` 없이 — 덤프가 같은 폴더에 있다. 회마다 페이지를 새로 연다(앞 회의 JIT · 캐시가 이어지지 않게).
// playwright-core 는 저장소 의존성이 아니다(측정 도구) — 설치된 것의 폴더를 `PLAYWRIGHT` 로 준다. 결과는 회마다 한 줄 JSON.

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { createRequire } from "node:module";

const [dir, runsArg, executablePath] = process.argv.slice(2);
if (!dir || !process.env.PLAYWRIGHT) {
  console.error(
    "usage: PLAYWRIGHT=<playwright-core dir> node run.mjs <dir> [runs] [webkit executable]",
  );
  process.exit(2);
}
const runs = Number(runsArg ?? 5);
const { webkit } = createRequire(import.meta.url)(process.env.PLAYWRIGHT);

const TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
};
const server = createServer(async (req, res) => {
  const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(
    /^(\.\.[/\\])+/,
    "",
  );
  try {
    const body = await readFile(join(dir, path === "/" ? "index.html" : path));
    res.writeHead(200, { "content-type": TYPES[extname(path)] ?? "application/octet-stream" });
    res.end(body);
  } catch {
    res.writeHead(404);
    res.end();
  }
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const { port } = server.address();

const browser = await webkit.launch({
  headless: true,
  ...(executablePath ? { executablePath } : {}),
});
try {
  for (let run = 1; run <= runs; run++) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
    await page.goto(`http://127.0.0.1:${port}/${process.env.PAGE ?? "index.html"}`);
    await page.waitForFunction(() => /^(done|error) /.test(document.title), null, {
      timeout: 300_000,
    });
    const title = await page.title();
    console.log(
      JSON.stringify({
        run,
        ...(title.startsWith("done ") ? JSON.parse(title.slice(5)) : { error: title }),
      }),
    );
    await page.close();
  }
} finally {
  await browser.close();
  server.close();
}
