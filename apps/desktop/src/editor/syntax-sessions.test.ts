import { ChangeSet, type ChangeSpec, Text } from "@codemirror/state";
import { afterEach, expect, it, vi } from "vitest";

import type { DocumentChange } from "./documents";
import { EDITABLE_MAX, LONG_LINE_CHARS } from "./limits";
import {
  createSyntaxSessions,
  type HighlightAnswer,
  MAPPED_VERSIONS,
  RETRY_BASE_MS,
  RETRY_MAX_MS,
  RETRY_NOTICE_AFTER,
  toTextChanges,
} from "./syntax-sessions";
import { fakeSyntaxServer, miniHighlight } from "./syntax-testing";

afterEach(() => {
  vi.useRealTimers();
});

/** 문서 모델의 바뀜 구독만 흉내 낸다. */
function fakeDocuments(initial: string) {
  let text = Text.of(initial.split("\n"));
  const listeners = new Set<(change: DocumentChange) => void>();
  return {
    onChange(_path: string, listener: (change: DocumentChange) => void) {
      listeners.add(listener);
      listener({ kind: "reset", text });
      return () => listeners.delete(listener);
    },
    edit(spec: ChangeSpec) {
      const before = text;
      const changes = ChangeSet.of(spec, before.length);
      text = changes.apply(before);
      for (const l of listeners) l({ kind: "edit", changes, before, after: text });
    },
    reset(next: string) {
      text = Text.of(next.split("\n"));
      for (const l of listeners) l({ kind: "reset", text });
    },
    get text() {
      return text.toString();
    },
  };
}

const all = { from: 0, to: 1_000_000 };

it("opens one service document for every view of a document and closes it with the last one", async () => {
  const server = fakeSyntaxServer();
  const sessions = createSyntaxSessions(server.source, fakeDocuments("fn a"));
  const one = sessions.acquire("/p/a.rs", "Mini");
  const two = sessions.acquire("/p/a.rs", "Mini");
  expect(await one.highlights(all)).toEqual({
    kind: "spans",
    version: 0,
    spans: [{ from: 0, to: 2, token: "keyword" }],
    exact: true,
  });
  expect(server.called("open")).toHaveLength(1);
  one.release();
  one.release();
  await Promise.resolve();
  expect(server.called("close")).toHaveLength(0);
  two.release();
  const [opened] = server.called("open") as { docId: string }[];
  await vi.waitFor(() => expect(server.called("close")).toEqual([{ docId: opened!.docId }]));
});

it("sends edits made while opening after the open answer, in version order", async () => {
  vi.useFakeTimers();
  const server = fakeSyntaxServer({
    delay: (m, leg) => (m === "open" && leg === "response" ? 50 : 0),
  });
  const documents = fakeDocuments("let x");
  const sessions = createSyntaxSessions(server.source, documents);
  const handle = sessions.acquire("/p/a.rs", "Mini");
  documents.edit({ from: 0, insert: "fn " });
  documents.edit({ from: 8, insert: ' "s"' });
  documents.edit({ from: 0, to: 3 });
  expect(server.called("edit")).toHaveLength(0);
  const answer = handle.highlights(all);
  await vi.advanceTimersByTimeAsync(50);
  expect((server.called("edit") as { version: number }[]).map((e) => e.version)).toEqual([1, 2, 3]);
  expect(await answer).toEqual({
    kind: "spans",
    version: 3,
    spans: miniHighlight('let x "s"'),
    exact: true,
  });
  expect([...server.docs.values()][0]!.text).toBe(documents.text);
});

it("moves an answer computed on an older version to the current text, and drops a far older one", async () => {
  vi.useFakeTimers();
  const server = fakeSyntaxServer({
    delay: (m, leg) => (m === "highlights" && leg === "response" ? 30 : 0),
  });
  const documents = fakeDocuments("fn a");
  const handle = createSyntaxSessions(server.source, documents).acquire("/p/a.rs", "Mini");
  await vi.advanceTimersByTimeAsync(0);
  const answer = handle.highlights(all);
  await vi.advanceTimersByTimeAsync(0);
  // 서버는 버전 0 으로 계산했는데 응답이 오기 전에 고쳤다 — 위치만 옮긴 구간이다.
  documents.edit({ from: 0, insert: "//" });
  await vi.advanceTimersByTimeAsync(30);
  expect(await answer).toEqual({
    kind: "spans",
    version: 1,
    spans: [{ from: 2, to: 4, token: "keyword" }],
    exact: false,
  });
  const next = handle.highlights(all);
  await vi.advanceTimersByTimeAsync(30);
  expect(await next).toEqual({
    kind: "spans",
    version: 1,
    spans: [{ from: 0, to: 6, token: "comment" }],
    exact: true,
  });

  // 들고 있는 바뀜보다 늦은 결과는 옮길 수 없다.
  const old = handle.highlights(all);
  await vi.advanceTimersByTimeAsync(0);
  for (let i = 0; i <= MAPPED_VERSIONS; i++) documents.edit({ from: 0, insert: "x" });
  await vi.advanceTimersByTimeAsync(30);
  expect(await old).toEqual({ kind: "stale" });
});

it("reopens with a new document id and the current text when the service lost the document", async () => {
  const server = fakeSyntaxServer();
  const documents = fakeDocuments("fn a");
  const handle = createSyntaxSessions(server.source, documents).acquire("/p/a.rs", "Mini");
  await handle.highlights(all);
  const [first] = server.called("open") as { docId: string }[];
  server.forget(first!.docId);
  documents.edit({ from: 4, insert: ' "x"' });
  // 편집이 not found 로 끝나면 다시 연다.
  await vi.waitFor(() => expect(server.called("open")).toHaveLength(2));
  const [, second] = server.called("open") as { docId: string; text: string }[];
  expect(second!.docId).not.toBe(first!.docId);
  expect(second!.text).toBe('fn a "x"');
  await vi.waitFor(async () =>
    expect(await handle.highlights(all)).toEqual({
      kind: "spans",
      version: 0,
      spans: miniHighlight('fn a "x"'),
      exact: true,
    }),
  );
});

it("reopens when an answer's version disagrees though nothing changed", async () => {
  const server = fakeSyntaxServer();
  const documents = fakeDocuments("fn a");
  const handle = createSyntaxSessions(server.source, documents).acquire("/p/a.rs", "Mini");
  await handle.highlights(all);
  // 서버만 한 편집을 더 받았다(어긋남).
  [...server.docs.values()][0]!.version = 7;
  expect(await handle.highlights(all)).toEqual({ kind: "stale" });
  expect(server.called("open")).toHaveLength(2);
  expect(await handle.highlights(all)).toMatchObject({ kind: "spans", version: 0 });
});

it("asks again instead of taking the empty answer of a parse that a newer edit stopped", async () => {
  vi.useFakeTimers();
  const server = fakeSyntaxServer({ parse: 40 });
  const documents = fakeDocuments("fn a");
  const handle = createSyntaxSessions(server.source, documents).acquire("/p/a.rs", "Mini");
  const answer = handle.highlights(all);
  await vi.advanceTimersByTimeAsync(10);
  documents.edit({ from: 4, insert: " x" });
  await vi.advanceTimersByTimeAsync(40);
  expect(await answer).toEqual({ kind: "stale" });
  const next = handle.highlights(all);
  await vi.advanceTimersByTimeAsync(40);
  expect(await next).toEqual({
    kind: "spans",
    version: 1,
    spans: miniHighlight("fn a x"),
    exact: true,
  });
});

it("backs off and reopens after a failure, doubling the wait up to a cap, and logs once when failures go on", async () => {
  vi.useFakeTimers();
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
  const server = fakeSyntaxServer();
  const real = server.source.highlights;
  let failing = 10;
  const asked: number[] = [];
  server.source.highlights = (params) => {
    asked.push(Date.now());
    return failing-- > 0 ? Promise.reject(new Error("rpc transport closed")) : real(params);
  };
  const documents = fakeDocuments("fn a");
  const handle = createSyntaxSessions(server.source, documents).acquire("/p/a.rs", "Mini");
  // 뷰처럼 답을 받자마자 다시 묻는다. 타이머는 다음 것까지만 넘긴다.
  let answer: HighlightAnswer = { kind: "stale" };
  for (let i = 0; answer.kind === "stale" && i < 20; i++) {
    let settled: HighlightAnswer | undefined;
    void handle.highlights(all).then((a) => (settled = a));
    // 물러선 동안 고친 것도 다시 열 때 실린다.
    if (i === 3) documents.edit({ from: 4, insert: ' "s"' });
    while (!settled) await vi.advanceTimersToNextTimerAsync();
    answer = settled;
  }
  expect(answer).toEqual({
    kind: "spans",
    version: 0,
    spans: miniHighlight('fn a "s"'),
    exact: true,
  });
  const waits = asked.slice(1).map((t, i) => t - asked[i]!);
  expect(waits).toEqual([1, 2, 4, 8, 16, 32, 64, 120, 120, 120].map((n) => n * RETRY_BASE_MS));
  expect(Math.max(...waits)).toBe(RETRY_MAX_MS);
  // 시도마다 앞 문서를 닫고 지금 텍스트로 새로 연다 — 남은 문서는 하나다.
  expect(server.called("open")).toHaveLength(11);
  expect(server.docs.size).toBe(1);
  expect(warn).toHaveBeenCalledTimes(1);
  expect(String(warn.mock.calls[0]![0])).toContain(`${RETRY_NOTICE_AFTER} times`);

  // 응답을 받으면 처음으로 돌아간다.
  failing = 1;
  documents.edit({ from: 0, insert: "x" });
  expect(await handle.highlights(all)).toEqual({ kind: "stale" });
  const retried = handle.highlights(all);
  await vi.advanceTimersByTimeAsync(RETRY_BASE_MS);
  expect(await retried).toMatchObject({ kind: "spans", exact: true });
  expect(warn).toHaveBeenCalledTimes(1);
  warn.mockRestore();
});

it("reopens with the current text after an edit fails instead of turning highlighting off", async () => {
  vi.useFakeTimers();
  const server = fakeSyntaxServer();
  const real = server.source.edit;
  let failing = 1;
  server.source.edit = (params) =>
    failing-- > 0 ? Promise.reject(new Error("rpc transport closed")) : real(params);
  const documents = fakeDocuments("fn a");
  const handle = createSyntaxSessions(server.source, documents).acquire("/p/a.rs", "Mini");
  await vi.advanceTimersByTimeAsync(0);
  documents.edit({ from: 4, insert: " // c" });
  const answer = handle.highlights(all);
  await vi.advanceTimersByTimeAsync(RETRY_BASE_MS);
  expect(await answer).toMatchObject({ kind: "stale" });
  expect(await handle.highlights(all)).toEqual({
    kind: "spans",
    version: 0,
    spans: miniHighlight("fn a // c"),
    exact: true,
  });
  expect(server.called("open")).toHaveLength(2);
});

it("keeps the session it opened for the new text when the views are made again after the document is read again", async () => {
  const server = fakeSyntaxServer();
  const documents = fakeDocuments("fn a");
  const sessions = createSyntaxSessions(server.source, documents);
  const old = sessions.acquire("/p/a.rs", "Mini");
  await old.highlights(all);
  documents.reset("// b");
  // 화면은 다시 읽은 문서의 뷰를 새로 만든다 — 같은 커밋에서 옛 뷰가 놓고 새 뷰가 잡는다.
  old.release();
  const fresh = sessions.acquire("/p/a.rs", "Mini");
  expect(await fresh.highlights(all)).toEqual({
    kind: "spans",
    version: 0,
    spans: [{ from: 0, to: 4, token: "comment" }],
    exact: true,
  });
  expect(server.called("open")).toHaveLength(2);
  expect(server.docs.size).toBe(1);
  fresh.release();
  await vi.waitFor(() => expect(server.docs.size).toBe(0));
});

it("reopens with the new text when the document is read again", async () => {
  const server = fakeSyntaxServer();
  const documents = fakeDocuments("fn a");
  const handle = createSyntaxSessions(server.source, documents).acquire("/p/a.rs", "Mini");
  await handle.highlights(all);
  documents.reset("// b");
  expect(await handle.highlights(all)).toEqual({
    kind: "spans",
    version: 0,
    spans: [{ from: 0, to: 4, token: "comment" }],
    exact: true,
  });
  expect(server.called("close")).toHaveLength(1);
});

it("answers plain for a language without a grammar, and for text over the size limit without asking", async () => {
  const server = fakeSyntaxServer();
  const plainLanguage = createSyntaxSessions(server.source, fakeDocuments("x")).acquire(
    "/p/a",
    "Nope",
  );
  expect(await plainLanguage.highlights(all)).toEqual({ kind: "plain" });

  const big = createSyntaxSessions(server.source, fakeDocuments("a".repeat(EDITABLE_MAX + 1)));
  expect(await big.acquire("/p/big", "Mini").highlights(all)).toEqual({ kind: "plain" });
  // 바이트로 넘는다: 한글 한 자는 3바이트.
  const wide = createSyntaxSessions(
    server.source,
    fakeDocuments(`${"가\n".repeat(Math.floor(EDITABLE_MAX / 4) + 1)}`),
  );
  expect(await wide.acquire("/p/wide", "Mini").highlights(all)).toEqual({ kind: "plain" });
  const long = createSyntaxSessions(
    server.source,
    fakeDocuments(`a\n${"b".repeat(LONG_LINE_CHARS + 1)}`),
  );
  expect(await long.acquire("/p/long", "Mini").highlights(all)).toEqual({ kind: "plain" });
  expect(server.called("open")).toHaveLength(1);

  // 기준 경계: 꼭 그만큼은 강조한다.
  const edge = createSyntaxSessions(
    server.source,
    fakeDocuments(`fn\n${"b".repeat(LONG_LINE_CHARS)}`),
  );
  expect(await edge.acquire("/p/edge", "Mini").highlights(all)).toMatchObject({ kind: "spans" });
});

it("goes plain when the service turns the document off", async () => {
  const server = fakeSyntaxServer();
  const documents = fakeDocuments("fn a");
  const handle = createSyntaxSessions(server.source, documents).acquire("/p/a.rs", "Mini");
  await handle.highlights(all);
  server.turnOff([...server.docs.keys()][0]!, "timedOut");
  expect(await handle.highlights(all)).toEqual({ kind: "plain" });
  documents.edit({ from: 0, insert: "x" });
  expect(await handle.highlights(all)).toEqual({ kind: "plain" });
  expect(server.called("edit")).toHaveLength(0);
});

it("turns CodeMirror changes into changes the service applies one after another", () => {
  const before = "0123456789abcdef";
  const changes = ChangeSet.of(
    [
      { from: 1, to: 3, insert: "XY\nZ" },
      { from: 5, insert: "!" },
      { from: 8, to: 12 },
      { from: 16, insert: "end" },
    ],
    before.length,
  );
  let text = before;
  for (const { from, to, insert } of toTextChanges(changes))
    text = text.slice(0, from) + insert + text.slice(to);
  expect(text).toBe(changes.apply(Text.of([before])).toString());
});
