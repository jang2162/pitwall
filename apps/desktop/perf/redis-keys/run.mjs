// Redis 키 트리 측정 하네스를 헤드리스 브라우저에서 돌린다(redis-keys-bench.tsx 머리 주석) — 앱 창을 띄우지 않는다.
//
//   pnpm --filter @pitwall/desktop exec vite build perf/redis-keys --config perf/redis-keys/vite.config.ts --outDir <폴더> --emptyOutDir --base ./
//   PLAYWRIGHT=<playwright-core 폴더> node apps/desktop/perf/redis-keys/run.mjs <폴더> [회수=5] [webkit|chromium] [브라우저 실행 파일]
//
// 회마다 페이지를 새로 연다(앞 회의 JIT · 캐시가 이어지지 않게). playwright-core 는 저장소 의존성이 아니다(측정 도구) — 설치된 것의
// 폴더를 `PLAYWRIGHT` 로 준다. 결과는 회마다 한 줄 JSON 이고, 마지막 줄이 중앙값과 판정이다(첫 화면 500 ms — 창 쪽만, Host 의 첫
// 묶음은 live 시험이 잰다 —, 스크롤은 갱신을 놓치지 않은 프레임 95% 이상 · 간격 중앙값 17 ms 이하 — docs/design/06-roadmap.md
// 「스크롤 60fps」).

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
      timeout: 600_000,
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
const scroll = (pick) => ({
  withinVsync: median(ok.map((r) => pick(r).interval.withinVsync)),
  vsyncMs: median(ok.map((r) => pick(r).interval.vsyncMs)),
  workP95: median(ok.map((r) => pick(r).work.p95)),
});
const summary = {
  engine,
  userAgent: ok[0]?.userAgent,
  runs: results.length,
  valid: ok.length,
  firstFrameMs: median(ok.map((r) => r.firstFrameMs)),
  allKeysMs: median(ok.map((r) => r.allKeysMs)),
  expandMs: median(ok.map((r) => r.expandMs)),
  fling: scroll((r) => r.fling),
  streaming: scroll((r) => r.streaming),
};
const pass = (s) => s.withinVsync >= 0.95 && s.vsyncMs <= 17;
summary.verdict = {
  firstFrame: summary.firstFrameMs !== null && summary.firstFrameMs <= 500,
  fling: pass(summary.fling),
  streaming: pass(summary.streaming),
  reachedTheEnd: ok.every((r) => r.lastKey === "k:999999"),
};
console.log(JSON.stringify({ summary }));
