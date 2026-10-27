// perf/* 하네스 · 검사 페이지를 헤드리스 브라우저로 연다(창을 앞에 띄우지 않는다). 저장소는 playwright 에 의존하지 않으므로 그
// 설치본 경로를 넘긴다.
//
//   node apps/desktop/perf/run-headless.cjs <webkit|chromium> <url> <window 전역 이름> <playwright-core 경로> [브라우저 실행 파일]
//
// 페이지 제목이 `done` 으로 시작하면 그 전역을 JSON 으로 찍는다. 제목이 `done FAIL` 이면 1 로 끝난다.
const [engine, url, global, pwPath, exe] = process.argv.slice(2);
const pw = require(pwPath);
(async () => {
  const browser = await pw[engine].launch({ headless: true, ...(exe && { executablePath: exe }) });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  page.on("console", (m) => m.type() === "error" && console.error("console:", m.text()));
  await page.goto(url);
  await page.waitForFunction(() => document.title.startsWith("done"), null, { timeout: 240000 });
  const title = await page.title();
  console.log(JSON.stringify(await page.evaluate(`window.${global}`), null, 1));
  await browser.close();
  process.exit(title === "done FAIL" ? 1 : 0);
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
