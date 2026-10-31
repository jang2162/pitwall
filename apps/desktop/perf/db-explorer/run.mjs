// DB 탐색기 측정 하네스를 헤드리스 브라우저에서 돌린다(db-explorer-bench.tsx 머리 주석) — 앱 창을 띄우지 않는다.
//
//   pnpm --filter @pitwall/desktop exec vite build perf/db-explorer --config perf/db-explorer/vite.config.ts --outDir <폴더> --emptyOutDir --base ./
//   PLAYWRIGHT=<playwright-core 폴더> node apps/desktop/perf/db-explorer/run.mjs <폴더> [회수=5] [webkit|chromium] [브라우저 실행 파일]
//
// 회마다 페이지를 새로 연다(앞 회의 JIT · 캐시가 이어지지 않게). playwright-core 는 저장소 의존성이 아니다(측정 도구) — 설치된 것의
// 폴더를 `PLAYWRIGHT` 로 준다. 결과는 회마다 한 줄 JSON 이고, 마지막 줄이 중앙값과 판정이다(첫 화면 500 ms — 결정 M3-D17).

import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { extname, join, normalize } from "node:path";

const [dir, runsArg, engine = "webkit", executablePath] = process.argv.slice(2);
if (!dir || !process.env.PLAYWRIGHT) {
  console.error(
    "usage: PLAYWRIGHT=<playwright-core dir> node run.mjs <dir> [runs] [webkit|chromium] [executable]",
  );
  process.exit(2);
}
const runs = Number(runsArg ?? 5);
const pw = createRequire(import.meta.url)(process.env.PLAYWRIGHT);

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

const browser = await pw[engine].launch({
  headless: true,
  ...(executablePath ? { executablePath } : {}),
});
const results = [];
try {
  for (let run = 1; run <= runs; run++) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
    page.on("console", (m) => m.type() === "error" && console.error("console:", m.text()));
    await page.goto(`http://127.0.0.1:${port}/index.html`);
    await page.waitForFunction(() => /^(done|error) /.test(document.title), null, {
      timeout: 300_000,
    });
    const title = await page.title();
    const r = title.startsWith("done ") ? JSON.parse(title.slice(5)) : { error: title };
    results.push(r);
    console.log(JSON.stringify({ run, ...r }));
    await page.close();
  }
} finally {
  await browser.close();
  server.close();
}

const ok = results.filter((r) => !r.error);
const median = (xs) => {
  const s = [...xs].sort((a, b) => a - b);
  return s.length ? s[Math.floor((s.length - 1) / 2)] : null;
};
const summary = {
  engine,
  userAgent: ok[0]?.userAgent,
  runs: results.length,
  valid: ok.length,
  tablesMs: median(ok.map((r) => r.tablesMs)),
  chainMs: median(ok.map((r) => r.chainMs)),
  maxTablesMs: ok.length ? Math.max(...ok.map((r) => r.tablesMs)) : null,
  domRows: median(ok.map((r) => r.domRows)),
};
summary.verdict = {
  firstScreen: summary.tablesMs !== null && summary.tablesMs <= 500,
  chain: summary.chainMs !== null && summary.chainMs <= 500,
  virtualized: ok.every((r) => r.domRows < 100),
};
console.log(JSON.stringify({ summary }));
