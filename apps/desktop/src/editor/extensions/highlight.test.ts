// @vitest-environment happy-dom
import { ChangeSet, EditorState, Text } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { Judgment, SyntaxToken } from "@pitwall/rpc-client";
import { afterEach, expect, it, vi } from "vitest";

import defaultTheme from "../../../../../extensions/theme-default/themes/default.json";
import { parseTheme, SYNTAX_TOKENS } from "../../theme/tokens";
import type { DocumentChange } from "../documents";
import { PLAIN_TEXT } from "../language";
import { createSyntaxSessions, type SyntaxSessions } from "../syntax-sessions";
import { type FakeSyntaxOptions, fakeSyntaxServer, miniHighlight } from "../syntax-testing";
import { type EditorFeatureContext, editorExtensions, reconfigureEditor } from ".";
import { highlight, highlightStyles, tokenClass } from "./highlight";

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

const judgment: Judgment = {
  binary: false,
  encoding: "UTF-8",
  bom: null,
  lineEnding: "lf",
  longLines: false,
  sizeClass: "editable",
  editable: true,
  highlight: true,
};

const baseContext: Omit<EditorFeatureContext, "syntax"> = {
  panelId: "editor-1",
  path: "/p/a.mini",
  judgment,
  readOnly: false,
  language: "Mini",
  appearance: "light",
  onCursor: () => {},
  history: { undo: () => false, redo: () => false },
};

/**
 * 뷰 하나와 그 문서의 바뀜 구독. 문서 모델(../live-text.ts)처럼 뷰를 먼저 바꾸고 바뀜을 알린다.
 */
function setup(
  doc: string,
  options: FakeSyntaxOptions = {},
  context: Partial<EditorFeatureContext> = {},
) {
  const server = fakeSyntaxServer(options);
  const listeners = new Set<(change: DocumentChange) => void>();
  let text = Text.of(doc.split("\n"));
  const sessions = createSyntaxSessions(server.source, {
    onChange(_path, listener) {
      listeners.add(listener);
      listener({ kind: "reset", text });
      return () => listeners.delete(listener);
    },
  });
  // 뷰가 세션에 묻는 횟수(세션이 서버를 부르지 않고 답하는 것까지 센다).
  let asked = 0;
  const counted: SyntaxSessions = {
    acquire(path, language) {
      const handle = sessions.acquire(path, language);
      return {
        get version() {
          return handle.version;
        },
        highlights: (range) => (asked++, handle.highlights(range)),
        release: () => handle.release(),
      };
    },
  };
  const ctx: EditorFeatureContext = { ...baseContext, syntax: counted, ...context };
  const view = new EditorView({
    parent: document.body,
    state: EditorState.create({ doc, extensions: editorExtensions(ctx) }),
    dispatchTransactions(trs, v) {
      for (const tr of trs) {
        v.update([tr]);
        if (!tr.docChanged) continue;
        const before = text;
        text = tr.changes.apply(before);
        for (const l of listeners)
          l({ kind: "edit", changes: tr.changes as ChangeSet, before, after: text });
      }
    },
  });
  return { server, view, ctx, asked: () => asked };
}

/** 뷰에 칠한 강조: `[from, to, 토큰 클래스]`. */
function painted(view: EditorView, from = 0, to = view.state.doc.length) {
  const out: [number, number, string][] = [];
  for (const source of view.state.facet(EditorView.decorations)) {
    const set = typeof source === "function" ? source(view) : source;
    set.between(from, to, (a, b, d) => {
      const cls = (d.spec as { class?: string }).class;
      if (cls?.startsWith("pw-syntax-")) out.push([Math.max(a, from), Math.min(b, to), cls]);
    });
  }
  return out.filter(([a, b]) => a < b).sort((x, y) => x[0] - y[0]);
}

/** 지금 텍스트의 정답(가짜 서버의 강조기). */
function truth(view: EditorView, from = 0, to = view.state.doc.length) {
  return miniHighlight(view.state.doc.toString())
    .filter((s) => s.to > from && s.from < to)
    .map((s): [number, number, string] => [
      Math.max(s.from, from),
      Math.min(s.to, to),
      tokenClass(s.token),
    ]);
}

it("paints each span with its token's class, colored by the theme's CSS variable", async () => {
  const { view } = setup('fn main() { let s = "x"; } // done');
  await vi.waitFor(() => expect(painted(view)).toEqual(truth(view)));
  expect(painted(view).map(([, , c]) => c)).toEqual([
    "pw-syntax-keyword",
    "pw-syntax-keyword",
    "pw-syntax-string",
    "pw-syntax-comment",
  ]);
  const keyword = view.contentDOM.querySelector(".pw-syntax-keyword");
  expect(keyword?.textContent).toBe("fn");
  // 색은 테마 토큰의 CSS 변수다 — 테마가 바뀌면 다시 묻지 않고 색이 바뀐다.
  for (const token of SYNTAX_TOKENS)
    expect(highlightStyles[`.${tokenClass(token)}`]).toMatchObject({
      color: `var(--pw-syntax-${token.replaceAll(".", "-")})`,
    });
  view.destroy();
});

it("gives keywords, strings and comments different colors in both variants of the default theme", () => {
  const theme = parseTheme(defaultTheme);
  for (const appearance of ["light", "dark"] as const) {
    const values = theme.variants[appearance]!;
    const colors = (["keyword", "string", "comment"] as SyntaxToken[]).map((t) =>
      values[`syntax.${t}`].toLowerCase(),
    );
    expect(new Set(colors).size, appearance).toBe(3);
  }
  // 외관이 바뀌면 색이 따른다.
  expect(theme.variants.light!["syntax.keyword"]).not.toBe(theme.variants.dark!["syntax.keyword"]);
});

it("keeps the plugin, its session and its colors through a theme change, without asking again", async () => {
  const { view, server, ctx } = setup("fn a");
  await vi.waitFor(() => expect(painted(view)).toHaveLength(1));
  const asked = server.called("highlights").length;
  view.dispatch(reconfigureEditor({ ...ctx, appearance: "dark" }));
  await new Promise((r) => setTimeout(r, 10));
  expect(painted(view)).toEqual(truth(view));
  expect(server.called("highlights")).toHaveLength(asked);
  expect(server.called("open")).toHaveLength(1);
  view.destroy();
});

it("keeps the old highlight, moved by the edit, until the new answer arrives", async () => {
  vi.useFakeTimers();
  const { view } = setup("fn a // c", {
    delay: (m, leg) => (m === "highlights" && leg === "response" ? 100 : 0),
  });
  await vi.advanceTimersByTimeAsync(100);
  expect(painted(view)).toEqual([
    [0, 2, "pw-syntax-keyword"],
    [5, 9, "pw-syntax-comment"],
  ]);
  view.dispatch({ changes: { from: 0, insert: "let " } });
  // 응답 전: 옛 강조가 4글자 뒤로 옮겨져 남는다.
  expect(painted(view)).toEqual([
    [4, 6, "pw-syntax-keyword"],
    [9, 13, "pw-syntax-comment"],
  ]);
  await vi.advanceTimersByTimeAsync(100);
  expect(painted(view)).toEqual(truth(view));
  expect(painted(view)[0]).toEqual([0, 3, "pw-syntax-keyword"]);
  view.destroy();
});

it("asks only for the range on screen, and again when scrolling past what it has", async () => {
  const lines = Array.from({ length: 5000 }, (_, i) => `let v${i} = "${i}"; // ${i}`);
  const { view, server } = setup(lines.join("\n"));
  await vi.waitFor(() => expect(server.called("highlights").length).toBeGreaterThan(0));
  const [first] = server.called("highlights") as { range: { from: number; to: number } }[];
  expect(first!.range).toEqual({ from: view.viewport.from, to: view.viewport.to });
  expect(first!.range.to).toBeLessThan(view.state.doc.length);

  const end = view.state.doc.length;
  view.dispatch({
    effects: EditorView.scrollIntoView(end, { y: "end" }),
    selection: { anchor: end },
  });
  await vi.waitFor(() =>
    expect((server.called("highlights") as { range: { to: number } }[]).at(-1)!.range.to).toBe(
      view.viewport.to,
    ),
  );
  expect(view.viewport.to).toBeGreaterThan(first!.range.to);
  expect(painted(view, view.viewport.from, view.viewport.to)).toEqual(
    truth(view, view.viewport.from, view.viewport.to),
  );
  view.destroy();
});

it("stays plain text without a language, without a session, or when the judgment turns highlighting off", () => {
  const sessions = { acquire: vi.fn() } as unknown as SyntaxSessions;
  const on = { ...baseContext, syntax: sessions };
  expect(highlight.extension(on)).not.toEqual([]);
  expect(highlight.extension({ ...on, language: null })).toEqual([]);
  expect(highlight.extension({ ...on, language: PLAIN_TEXT })).toEqual([]);
  expect(highlight.extension({ ...on, syntax: undefined })).toEqual([]);
  expect(highlight.extension({ ...on, judgment: { ...judgment, highlight: false } })).toEqual([]);
});

it("stops asking and clears the colors for a language without a grammar", async () => {
  const { view, server, ctx, asked } = setup("fn a");
  await vi.waitFor(() => expect(painted(view)).toHaveLength(1));
  view.dispatch(reconfigureEditor({ ...ctx, language: "Nope" }));
  await vi.waitFor(() => expect(server.called("open")).toHaveLength(2));
  await new Promise((r) => setTimeout(r, 10));
  expect(painted(view)).toEqual([]);
  // 앞 언어의 문서는 닫았다.
  expect(server.called("close")).toHaveLength(1);
  const before = asked();
  view.dispatch({ changes: { from: 0, insert: "x" } });
  await new Promise((r) => setTimeout(r, 10));
  expect(asked()).toBe(before);
  view.destroy();
  expect(server.called("close")).toHaveLength(1);
});

it("releases its session when the view goes away", async () => {
  const { view, server } = setup("fn a");
  await vi.waitFor(() => expect(painted(view)).toHaveLength(1));
  view.destroy();
  expect(server.called("close")).toHaveLength(1);
});

/** 시드가 있는 의사 난수(재현되게). */
function random(seed: number) {
  return () => {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    return seed / 0x80000000;
  };
}

/** 범위 안 글자마다의 토큰 클래스(없으면 빈 문자열). */
function classes(spans: [number, number, string][], from: number, to: number): string[] {
  const out = Array.from({ length: to - from }, () => "");
  for (const [a, b, cls] of spans) for (let i = a; i < b; i++) out[i - from] = cls;
  return out;
}

// M1-17 완료 기준: 초당 10자로 30초 타이핑하는 동안 틀린 강조가 1초 넘게 남지 않는다. RPC 의 요청 · 응답이 각각 0~150 ms 늦게,
// 서로 뒤섞여 도착한다(실제 IPC 는 수 ms 다 — 일부러 비관적으로 잡았다). 친 글자에는 따옴표 · 주석 기호가 있어 그 줄 뒤쪽의 강조가
// 통째로 바뀐다. 20 ms 마다 화면 범위의 글자마다 칠한 토큰을 정답과 견주고, 글자 하나가 틀린 채 남은 가장 긴 시간을 잰다(글자는
// 편집을 따라 옮겨 가며 같은 글자로 센다). 화면 어딘가가 틀린 상태가 이어진 가장 긴 시간도 함께 적는다 — 주석 · 문자열 끝에서
// 계속 치는 동안에는 방금 친 글자가 늘 한 왕복만큼 늦게 칠해지므로 이 값은 그 구간을 치는 시간만큼 길다.
// 가짜 타이머로 31초를 20 ms 씩 걷는다 — 결과는 시드로 정해지지만 실제로 걸리는 시간은 기기 부하를 탄다(혼자 1~3초).
const TYPING_TIMEOUT = 30_000;

async function typeForThirtySeconds(maxDelay: number) {
  vi.useFakeTimers();
  const next = random(17);
  const lines = Array.from({ length: 200 }, (_, i) => `let v${i} = "${i}"; // ${i}`);
  const { view, server } = setup(lines.join("\n"), {
    delay: () => Math.floor(next() * (maxDelay + 1)),
  });
  const script = 'fn f() { let s = "a // b"; return s; } // "q"\n';
  let cursor = view.state.doc.line(20).to;
  view.dispatch({ changes: { from: cursor, insert: "\n" } });
  cursor += 1;
  // 글자마다 id. 이 시험은 커서 자리에 끼우기만 하므로 끼운 자리에 새 id 를 넣으면 된다.
  let seq = 0;
  const ids = Array.from({ length: view.state.doc.length }, () => seq++);
  const wrongSince = new Map<number, number>();
  let longest = 0;
  let screenSince: number | undefined;
  let screenLongest = 0;
  let typed = 0;
  const step = 20;
  const end = 31_000;
  for (let t = 0; t <= end; t += step) {
    if (t % 100 === 0 && t < 30_000) {
      const ch = script[typed % script.length]!;
      view.dispatch({ changes: { from: cursor, insert: ch }, selection: { anchor: cursor + 1 } });
      ids.splice(cursor, 0, seq++);
      cursor += 1;
      typed += 1;
    }
    await vi.advanceTimersByTimeAsync(step);
    const { from, to } = view.viewport;
    const got = classes(painted(view, from, to), from, to);
    const want = classes(truth(view, from, to), from, to);
    let anyWrong = false;
    for (let i = 0; i < got.length; i++) {
      const id = ids[from + i]!;
      if (got[i] !== want[i]) {
        anyWrong = true;
        if (!wrongSince.has(id)) wrongSince.set(id, t);
      } else if (wrongSince.has(id)) {
        longest = Math.max(longest, t - wrongSince.get(id)!);
        wrongSince.delete(id);
      }
    }
    if (anyWrong) screenSince ??= t;
    else if (screenSince !== undefined) {
      screenLongest = Math.max(screenLongest, t - screenSince);
      screenSince = undefined;
    }
  }
  for (const since of wrongSince.values()) longest = Math.max(longest, end - since);
  if (screenSince !== undefined) screenLongest = Math.max(screenLongest, end - screenSince);
  console.info(
    `[highlight] delay 0-${maxDelay} ms per leg, typed ${typed} chars: longest wrong character ${longest} ms, longest wrong screen ${screenLongest} ms, ${server.called("highlights").length} requests`,
  );
  view.destroy();
  return { typed, longest, screenLongest, endedWrong: screenSince !== undefined };
}

it(
  "never leaves a wrong highlight for more than a second while typing ten characters a second for thirty seconds",
  async () => {
    const slow = await typeForThirtySeconds(150);
    expect(slow.typed).toBe(300);
    expect(slow.longest).toBeLessThan(1000);
    // 다 친 뒤에는 화면이 정답이다.
    expect(slow.endedWrong).toBe(false);
  },
  TYPING_TIMEOUT,
);

it(
  "keeps the whole screen right within a second while typing over a local connection",
  async () => {
    const local = await typeForThirtySeconds(10);
    expect(local.longest).toBeLessThan(1000);
    expect(local.screenLongest).toBeLessThan(1000);
    expect(local.endedWrong).toBe(false);
  },
  TYPING_TIMEOUT,
);
