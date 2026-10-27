// diff 뷰의 실브라우저 검사(13-m2-plan M2-15 리뷰 F1 · F3): happy-dom 은 높이를 재지 않아 이 둘은 진짜 브라우저에서만 보인다.
//
// - `colors`: 앱과 같은 diff 뷰(`buildDiffView`)를 라이트 · 다크 × 나란히 · 통합으로 그리고, 칠한 줄(`[data-diff]` · 통합의 뺀 줄
//   블록)마다 계산된 배경(`getComputedStyle`)이 그 줄의 토큰 색(`diff.line.added` · `deleted` · `changed`)인지, 바뀐 글자가
//   `diff.char.*` 색인지 본다. 토큰 색은 같은 페이지에 `var(--pw-…)` 를 배경으로 둔 상자의 계산값이다.
// - `scroll`: 3,000줄 · 묶음 93개 diff 에서 보기를 바꿀 때 화면 맨 위 줄이 남는지(나란히 → 통합, 통합 → 나란히 각 10곳). DiffView 와
//   같은 `restoreTopLine` 으로 맞추고 그 상한 프레임만큼 기다린다. 문서 끝 근처(그 줄을 맨 위에 둘 수 없음)는 끝까지 굴렸으면 맞다.
//
// 만들기: `pnpm --filter @pitwall/desktop exec vite build perf/diff-check --config vite.config.ts --outDir <폴더> --emptyOutDir --base ./`
// 그 폴더를 `127.0.0.1` 정적 서버로 열고 `index.html` 을 헤드리스 브라우저로 연다(perf/run-headless.cjs). 끝나면 결과가
// `window.__diffCheck` 와 페이지 제목(`done ok` · `done FAIL`)에 남는다. 이 검사는 `mise run test` 에 들지 않는다 — 저장소에 브라우저
// · playwright 의존성이 없다. diff 색 · 줄 맞춤을 고쳤으면 돌린다.

import "../bench.css";

import defaultTheme from "../../../../extensions/theme-default/themes/default.json";
import {
  buildDiffView,
  type DiffMode,
  RESTORE_FRAMES,
  restoreTopLine,
} from "../../src/git/diff/diff-view";
import { applyTokens, parseTheme, tokenCssVar } from "../../src/theme/tokens";
import { content } from "./content";

const frame = () => new Promise<number>((r) => requestAnimationFrame(r));

/** 토큰의 계산된 색(그 변수를 배경으로 둔 상자). */
function tokenColor(token: string): string {
  const box = document.createElement("div");
  box.style.background = `var(${tokenCssVar(token)})`;
  document.body.append(box);
  const color = getComputedStyle(box).backgroundColor;
  box.remove();
  return color;
}

async function colors(parent: HTMLElement) {
  const theme = parseTheme(defaultTheme);
  const failures: string[] = [];
  const counts: Record<string, number> = {};
  for (const variant of ["light", "dark"] as const) {
    applyTokens(document.documentElement.style, theme.variants[variant]!);
    const want = {
      added: tokenColor("diff.line.added"),
      deleted: tokenColor("diff.line.deleted"),
      changed: tokenColor("diff.line.changed"),
      charChanged: tokenColor("diff.char.changed"),
      charAdded: tokenColor("diff.char.added"),
      charDeleted: tokenColor("diff.char.deleted"),
    };
    for (const mode of ["split", "unified"] as const) {
      parent.replaceChildren();
      const built = buildDiffView({ content: content(), mode, parent, phrases: {} });
      await frame();
      await frame();
      const check = (el: Element, expected: string, what: string) => {
        const got = getComputedStyle(el).backgroundColor;
        const key = `${variant}/${mode}/${what}`;
        counts[key] = (counts[key] ?? 0) + 1;
        if (got !== expected) failures.push(`${key}: ${got} != ${expected} (${el.textContent})`);
      };
      for (const el of parent.querySelectorAll("[data-diff]")) {
        const cls = el.className;
        if (cls.includes("pw-diff-changed")) check(el, want.changed, "changed");
        else if (cls.includes("pw-diff-added")) check(el, want.added, "added");
        else check(el, want.deleted, "deleted");
      }
      for (const el of parent.querySelectorAll(".pw-diff-char-changed"))
        check(el, want.charChanged, "char-changed");
      for (const el of parent.querySelectorAll(".pw-diff-char-added"))
        check(el, want.charAdded, "char-added");
      for (const el of parent.querySelectorAll(".pw-diff-char-deleted"))
        check(el, want.charDeleted, "char-deleted");
      // 칠하지 않는 줄(merge 의 덩어리에 함께 든 문맥 줄 포함)은 배경이 없다.
      for (const el of parent.querySelectorAll(".cm-line:not([data-diff])"))
        if (getComputedStyle(el).backgroundColor !== "rgba(0, 0, 0, 0)")
          failures.push(`${variant}/${mode}/context painted: ${el.textContent}`);
      built.destroy();
    }
  }
  return { failures, counts };
}

async function scroll(parent: HTMLElement) {
  applyTokens(document.documentElement.style, parseTheme(defaultTheme).variants.light!);
  const c = content();
  const probe = buildDiffView({ content: c, mode: "split", parent, phrases: {} });
  const groups = probe.controller.model.groups;
  probe.destroy();
  const targets = [5, 20, 47, 66, 88].flatMap((k) => [groups[k]!.newFrom - 1, groups[k]!.newTo]);
  const results: {
    from: DiffMode;
    to: DiffMode;
    target: number;
    before: number;
    after: number;
    atEnd: boolean;
  }[] = [];
  for (const [from, to] of [
    ["split", "unified"],
    ["unified", "split"],
  ] as const) {
    for (const target of targets) {
      parent.replaceChildren();
      let b = buildDiffView({ content: c, mode: from, parent, phrases: {} });
      restoreTopLine(b.controller, parent, target);
      for (let i = 0; i <= RESTORE_FRAMES; i++) await frame();
      const before = b.controller.topLine();
      b.destroy();
      // 보기 전환과 같은 길(DiffView 가 부르는 `restoreTopLine`)로 잇고, 그 상한만큼 기다린다.
      b = buildDiffView({ content: c, mode: to, parent, phrases: {} });
      restoreTopLine(b.controller, parent, before);
      for (let i = 0; i <= RESTORE_FRAMES; i++) await frame();
      const scroller = parent.querySelector<HTMLElement>(
        to === "split" ? ".cm-mergeView" : ".cm-scroller",
      )!;
      // 문서 끝 근처는 그 줄을 맨 위에 둘 수 없다(스크롤 끝) — 그때는 끝까지 굴렸으면 맞다.
      const atEnd = scroller.scrollTop >= scroller.scrollHeight - scroller.clientHeight - 1;
      results.push({ from, to, target, before, after: b.controller.topLine(), atEnd });
      b.destroy();
    }
  }
  return { failures: results.filter((r) => r.before !== r.after && !r.atEnd), results };
}

async function main() {
  const parent = document.getElementById("root")!;
  const c = await colors(parent);
  const s = await scroll(parent);
  const ok = c.failures.length === 0 && s.failures.length === 0;
  const result = { ok, colors: c, scroll: s, userAgent: navigator.userAgent };
  (window as unknown as { __diffCheck: unknown }).__diffCheck = result;
  document.title = ok ? "done ok" : "done FAIL";
}

main().catch((e: unknown) => {
  (window as unknown as { __diffCheck: unknown }).__diffCheck = { ok: false, error: String(e) };
  document.title = "done FAIL";
});
