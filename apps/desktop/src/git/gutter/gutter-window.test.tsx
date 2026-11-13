// @vitest-environment happy-dom
// 에디터 거터 · 인라인 diff 팝업 · blame(13-m2-plan M2-17)의 창 시험. 소스는 가짜다(testing.ts) — 기준 텍스트(작업 트리 꼴)와
// blame 범위는 Rust 쪽 시험(crates/pw-git 의 work_tree_form · blame 시험)이 git 과 같음을 본다.

import type { BlameChunk, DiffFile } from "@pitwall/rpc-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { annotations, toggleAnnotations } from "./blame-extension";
import { goToChange, gutterLink, gutterState, togglePopupAt } from "./gutter-extension";
import { BLAME_REVISION_ACTION, HISTORY_ACTION, REVEAL_ACTION } from "./gutter-host";
import { FakeSource, type Harness, openEditor, textBlob, TOP, unifiedFile } from "./testing";
import { withTooltips } from "../../test-support/editor-tooltips";

let open: Harness[] = [];
afterEach(() => {
  for (const h of open) h.destroy();
  open = [];
  document.querySelectorAll(".pw-blame-menu").forEach((m) => m.remove());
});

function start(options: Parameters<typeof openEditor>[0], head?: string): Harness {
  const source = options.source ?? new FakeSource();
  source.files.set("a.txt", textBlob(head ?? options.text));
  const h = openEditor({ ...options, source });
  // happy-dom 은 포커스를 옮기며 selectionchange 를 그 자리에서(동기로) 쏘아 CodeMirror 의 갱신 도중에 다시 갱신하게 한다(브라우저는
  // 미룬다). 포커스는 시험이 `hasFocus` 로 정한다.
  h.view.focus = () => {};
  open.push(h);
  return h;
}

/** 뷰에 포커스가 있다고 한다(인라인 blame 은 포커스가 있을 때만 보인다). */
function focus(h: Harness) {
  vi.spyOn(h.view, "hasFocus", "get").mockReturnValue(true);
}

const hunks = (h: Harness) => h.view.state.field(gutterState).hunks;
const marks = (h: Harness) =>
  [...withTooltips(h.view).querySelectorAll<HTMLElement>(".pw-git-gutter .pw-git-mark")].map(
    (m) => m.dataset.gitChange,
  );

/** 모델이 기준을 받고 뷰가 그 hunk 를 받을 때까지. */
async function ready(h: Harness) {
  await vi.waitFor(() => expect(h.view.plugin(gutterLink)?.doc?.state.kind).toBe("on"));
  await Promise.resolve();
}

function type(h: Harness, line: number, text: string) {
  const at = h.view.state.doc.line(line);
  h.view.dispatch({ changes: { from: at.from, to: at.to, insert: text }, userEvent: "input.type" });
}

function popupButton(h: Harness, name: string): HTMLButtonElement {
  const b = withTooltips(h.view).querySelector<HTMLButtonElement>(`[data-git-popup="${name}"]`);
  if (!b) throw new Error(`no popup button ${name}`);
  return b;
}

/** 줄 `line` 의 거터 표시를 누른 것과 같다(happy-dom 은 배치가 없어 줄의 거터 칸을 좌표로 찾지 못한다 — 누름 자체는 따로 본다). */
function clickMark(h: Harness, line: number) {
  if (!togglePopupAt(h.view, line)) throw new Error(`no mark on line ${line}`);
}

describe("gutter markers", () => {
  it("marks an unsaved edit of a committed line, and an added and a deleted line", async () => {
    const h = start({ text: "one\ntwo\nthree\nfour\n" });
    await ready(h);
    expect(hunks(h)).toEqual([]);
    // 100 ms 기준은 gutter-perf.test.tsx 가 가짜 시계로 본다(벽시계는 부하에 흔들린다).
    type(h, 2, "TWO");
    await vi.waitFor(() => expect(marks(h)).toEqual(["modified"]));
    expect(h.live.dirty).toBe(true);
    // 더한 줄 · 지운 줄.
    h.view.dispatch({ changes: { from: 0, insert: "zero\n" } });
    const four = h.view.state.doc.line(5);
    h.view.dispatch({ changes: { from: four.from, to: four.to + 1 } });
    await vi.waitFor(() =>
      expect(hunks(h)).toEqual([
        { oldFrom: 1, oldTo: 1, newFrom: 1, newTo: 2 },
        { oldFrom: 2, oldTo: 3, newFrom: 3, newTo: 4 },
        { oldFrom: 4, oldTo: 5, newFrom: 5, newTo: 5 },
      ]),
    );
    await vi.waitFor(() => expect(marks(h)).toEqual(["added", "modified", "deleted"]));
  });

  it("opens the inline diff popup when its marker is pressed", async () => {
    // happy-dom 은 배치가 없어 거터가 누른 자리를 맨 위 줄로 읽는다 — 표시를 첫 줄에 둔다.
    const h = start({ text: "A\nb\n" }, "a\nb\n");
    await ready(h);
    await vi.waitFor(() => expect(marks(h)).toEqual(["modified"]));
    const element = withTooltips(h.view).querySelector(".pw-git-mark")!.parentElement!;
    element.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
    expect(withTooltips(h.view).querySelector(".pw-git-popup")).not.toBeNull();
    element.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
    expect(withTooltips(h.view).querySelector(".pw-git-popup")).toBeNull();
  });

  it("reads HEAD in its work tree form at the commit HEAD points to", async () => {
    const h = start({ text: "x\n" });
    await ready(h);
    expect(h.source.blobCalls).toEqual([
      { rev: "a".repeat(40), path: "a.txt", workTreeForm: true },
    ]);
  });

  it("drops the markers when a commit from the terminal moves HEAD to the edited text", async () => {
    const h = start({ text: "one\ntwo\n" }, "one\nold\n");
    await ready(h);
    await vi.waitFor(() => expect(marks(h)).toEqual(["modified"]));
    // `git commit` 이 HEAD 를 옮겼다 — 브랜치 저장소가 새 해시를 알린다.
    h.source.files.set("a.txt", textBlob("one\ntwo\n"));
    h.source.heads = "b".repeat(40);
    h.source.changed();
    await vi.waitFor(() => expect(marks(h)).toEqual([]));
    expect(h.source.blobCalls.at(-1)).toMatchObject({ rev: "b".repeat(40) });
    expect(h.source.blameCalls.at(-1)).toEqual({ path: "a.txt", rev: "b".repeat(40) });
  });

  it("is off for untracked files, files outside the repository, filtered files and over the editable size", async () => {
    const untracked = new FakeSource();
    untracked.letters.set(`${TOP}/a.txt`, "?");
    const a = start({ text: "x\n", source: untracked });
    await vi.waitFor(() =>
      expect(a.view.plugin(gutterLink)?.doc?.state).toEqual({ kind: "off", reason: "untracked" }),
    );
    expect(untracked.blobCalls).toEqual([]);

    const outside = start({ text: "x\n", path: "/elsewhere/a.txt" });
    await vi.waitFor(() =>
      expect(outside.view.plugin(gutterLink)?.doc?.state).toEqual({
        kind: "off",
        reason: "outside",
      }),
    );

    // 외부 필터 드라이버: 내용 없이 `filter` — 끄고 위 막대에 까닭을 적는다.
    const filtered = new FakeSource();
    filtered.files.set("a.txt", { ...textBlob(""), content: null, judgment: null, filter: "lfs" });
    const f = openEditor({ text: "pointer\n", source: filtered });
    open.push(f);
    await vi.waitFor(() =>
      expect(withTooltips(f.view).querySelector(".pw-git-off")?.textContent).toContain(
        'git-gutter-off-filter {"driver":"lfs"}',
      ),
    );
    expect(f.view.state.field(gutterState).hunks).toEqual([]);

    // 편집 등급(≤ 5 MiB)을 넘는 문서는 기능 자체가 없다.
    const big = start({ text: "x\n", sizeClass: "readOnly" });
    expect(big.view.state.field(gutterState, false)).toBeUndefined();
    expect(big.view.state.field(annotations, false)).toBeUndefined();
  });

  it("compares a staged new file with an empty HEAD", async () => {
    const source = new FakeSource();
    source.letters.set(`${TOP}/a.txt`, "A");
    const h = openEditor({ text: "new\nfile\n", source });
    open.push(h);
    await ready(h);
    await vi.waitFor(() =>
      expect(hunks(h)).toEqual([{ oldFrom: 1, oldTo: 1, newFrom: 1, newTo: 3 }]),
    );
  });

  it("moves to the next and previous change", async () => {
    const h = start({ text: "1\nX\n3\n4\n5\nY\n7\n" }, "1\n2\n3\n4\n5\n6\n7\n");
    await ready(h);
    await vi.waitFor(() => expect(hunks(h)).toHaveLength(2));
    const line = () => h.view.state.doc.lineAt(h.view.state.selection.main.head).number;
    expect(goToChange(h.view, 1)).toBe(true);
    expect(line()).toBe(2);
    expect(goToChange(h.view, 1)).toBe(true);
    expect(line()).toBe(6);
    expect(goToChange(h.view, 1)).toBe(false);
    expect(goToChange(h.view, -1)).toBe(true);
    expect(line()).toBe(2);
  });
});

/** HEAD 텍스트에 편집 둘(`first` 줄과 `second` 줄)을 한 문서. 하나를 되돌리면 나머지만 남아야 한다. */
interface RevertCase {
  name: string;
  head: string;
  lineEnding: "lf" | "crlf" | "mixed";
  /** 문서(디스크 바이트 — 줄바꿈 포함). */
  text: string;
  /** 되돌릴 hunk 의 표시가 있는 줄. */
  line: number;
  /** 되돌린 뒤 저장할 바이트. */
  expected: string;
}

const REVERT_CASES: RevertCase[] = [
  {
    name: "a changed line",
    head: "a\nb\nc\nd\ne\n",
    lineEnding: "lf",
    text: "a\nB\nc\nd\nE\n",
    line: 2,
    expected: "a\nb\nc\nd\nE\n",
  },
  {
    // autocrlf: HEAD 의 작업 트리 꼴도 CRLF 다(git.blob workTreeForm).
    name: "CRLF document with an autocrlf HEAD",
    head: "a\r\nb\r\nc\r\nd\r\ne\r\n",
    lineEnding: "crlf",
    text: "a\r\nB\r\nc\r\nd\r\nE\r\n",
    line: 2,
    expected: "a\r\nb\r\nc\r\nd\r\nE\r\n",
  },
  {
    name: "a hunk at the end of a file without a final newline",
    head: "a\nb\nc\nd\ne",
    lineEnding: "lf",
    text: "A\nb\nc\nd\nE",
    line: 5,
    expected: "A\nb\nc\nd\ne",
  },
  {
    name: "added lines at the end, removing the final newline the edit added",
    head: "a\nb\nc\nd\ne",
    lineEnding: "lf",
    text: "A\nb\nc\nd\ne\nf\n",
    line: 6,
    expected: "A\nb\nc\nd\ne",
  },
  {
    name: "a deletion hunk",
    head: "a\nb\nc\nd\ne\nf\n",
    lineEnding: "lf",
    text: "A\nb\nc\nd\nf\n",
    line: 4,
    expected: "A\nb\nc\nd\ne\nf\n",
  },
  {
    name: "a deletion of the first lines",
    head: "x\ny\na\nb\nc\nd\ne\n",
    lineEnding: "lf",
    text: "a\nb\nc\nd\nE\n",
    line: 1,
    expected: "x\ny\na\nb\nc\nd\nE\n",
  },
  {
    name: "a deletion at the end of a file without a final newline",
    head: "a\nb\nc\nd\ne\nf",
    lineEnding: "lf",
    text: "A\nb\nc\nd\ne",
    line: 5,
    expected: "A\nb\nc\nd\ne\nf",
  },
  {
    // 섞인 줄바꿈: 줄은 `\n` 으로만 나뉘고 `\r` 은 글자다 — 되돌린 줄의 `\r` 이 그대로 돌아온다.
    name: "mixed line endings",
    head: "a\r\nb\nc\r\nd\ne\n",
    lineEnding: "mixed",
    text: "a\r\nB\nc\r\nd\nE\n",
    line: 2,
    expected: "a\r\nb\nc\r\nd\nE\n",
  },
];

describe("inline diff popup", () => {
  it.each(REVERT_CASES)(
    "rolls back only that hunk, byte for byte: $name",
    async ({ head, lineEnding, text, line, expected }) => {
      const h = start({ text, lineEnding }, head);
      await ready(h);
      await vi.waitFor(() => expect(hunks(h)).toHaveLength(2));
      const before = h.live.serialize();
      expect(before).toBe(text);
      clickMark(h, line);
      popupButton(h, "revert").click();
      expect(h.live.serialize()).toBe(expected);
      // 그 hunk 하나만 — 나머지 hunk 는 남는다.
      await vi.waitFor(() => expect(hunks(h)).toHaveLength(1));
      // undo 한 번에 돌아온다.
      h.live.undo(h.view);
      expect(h.live.serialize()).toBe(before);
    },
  );

  it("shows the hunk with the diff view and steps to the previous and next change", async () => {
    const h = start({ text: "1\nX\n3\n4\n5\nY\n7\n" }, "1\n2\n3\n4\n5\n6\n7\n");
    await ready(h);
    await vi.waitFor(() => expect(hunks(h)).toHaveLength(2));
    clickMark(h, 2);
    const popup = () => withTooltips(h.view).querySelector<HTMLElement>(".pw-git-popup")!;
    expect(popup().getAttribute("role")).toBe("dialog");
    expect(popup().querySelector(".pw-git-popup-position")?.textContent).toContain('"index":1');
    // diff 뷰(통합 보기)가 HEAD 의 줄과 지금 줄을 함께 그린다.
    await vi.waitFor(() =>
      expect(popup().querySelector(".pw-git-popup-diff .cm-editor")).not.toBeNull(),
    );
    expect(popup().textContent).toContain("X");
    expect(popupButton(h, "previous").disabled).toBe(true);
    popupButton(h, "next").click();
    await vi.waitFor(() =>
      expect(popup().querySelector(".pw-git-popup-position")?.textContent).toContain('"index":2'),
    );
    expect(popupButton(h, "next").disabled).toBe(true);
    popupButton(h, "close").click();
    expect(withTooltips(h.view).querySelector(".pw-git-popup")).toBeNull();
  });

  it("does not roll back in a read-only document", async () => {
    const h = start({ text: "a\nB\n", readOnly: true }, "a\nb\n");
    await ready(h);
    await vi.waitFor(() => expect(hunks(h)).toHaveLength(1));
    clickMark(h, 2);
    expect(popupButton(h, "revert").disabled).toBe(true);
  });
});

describe("staging from the popup", () => {
  /** `git diff`(index ↔ 작업 트리)의 그 파일: 2번 줄 바뀜 · 6번 줄 바뀜. */
  const workTreeDiff: DiffFile = {
    status: "modified",
    path: "a.txt",
    oldPath: null,
    oldMode: "100644",
    newMode: "100644",
    oldId: null,
    newId: null,
    binary: false,
    tooLarge: false,
    hunksOmitted: false,
    generated: false,
    submodule: null,
    added: 2,
    removed: 2,
    error: null,
    hunks: [
      {
        oldStart: 1,
        oldLines: 7,
        newStart: 1,
        newLines: 7,
        header: "",
        lines: [
          { kind: "context", text: "1", oldLine: 1, newLine: 1, noNewline: false },
          { kind: "removed", text: "2", oldLine: 2, newLine: null, noNewline: false },
          { kind: "added", text: "X", oldLine: null, newLine: 2, noNewline: false },
          { kind: "context", text: "3", oldLine: 3, newLine: 3, noNewline: false },
          { kind: "context", text: "4", oldLine: 4, newLine: 4, noNewline: false },
          { kind: "context", text: "5", oldLine: 5, newLine: 5, noNewline: false },
          { kind: "removed", text: "6", oldLine: 6, newLine: null, noNewline: false },
          { kind: "added", text: "Y", oldLine: null, newLine: 6, noNewline: false },
          { kind: "context", text: "7", oldLine: 7, newLine: 7, noNewline: false },
        ],
      },
    ],
  };

  it("stages only the lines of that hunk with git.stageLines when the document is saved", async () => {
    const source = new FakeSource();
    source.diffs.set("a.txt", workTreeDiff);
    const h = start({ text: "1\nX\n3\n4\n5\nY\n7\n", source }, "1\n2\n3\n4\n5\n6\n7\n");
    await ready(h);
    await vi.waitFor(() => expect(hunks(h)).toHaveLength(2));
    clickMark(h, 6);
    // 열 때 스테이징할 수 있는지 미리 본다 — 그동안은 꺼져 있다.
    await vi.waitFor(() => expect(popupButton(h, "stage").disabled).toBe(false));
    popupButton(h, "stage").click();
    await vi.waitFor(() => expect(source.staged).toHaveLength(1));
    expect(source.staged[0]).toEqual({
      path: "a.txt",
      hunks: [{ hunk: workTreeDiff.hunks[0], lines: [6, 7] }],
    });
    // 스테이징해도 HEAD 기준이라 표시는 남는다(결정 M2-D9).
    expect(hunks(h)).toHaveLength(2);
  });

  it("is off with the reason while there are unsaved edits", async () => {
    const source = new FakeSource();
    source.diffs.set("a.txt", { ...workTreeDiff, ...unifiedFile("@@ -1,2 +1,2 @@\n a\n-b\n+B\n") });
    const h = start({ text: "a\nb\n", source }, "a\nb\n");
    await ready(h);
    type(h, 2, "B");
    await vi.waitFor(() => expect(hunks(h)).toHaveLength(1));
    clickMark(h, 2);
    const stage = popupButton(h, "stage");
    expect(stage.disabled).toBe(true);
    expect(stage.title).toBe("git-gutter-stage-dirty");
    // 저장하면 켜진다.
    h.live.markClean();
    await vi.waitFor(() => expect(popupButton(h, "stage").disabled).toBe(false));
  });

  it("does not stage when the disk moved away from the document", async () => {
    const source = new FakeSource();
    source.diffs.set("a.txt", workTreeDiff);
    const h = start(
      {
        text: "1\nX\n3\n4\n5\nY\n7\n",
        source,
        versions: async () => ({ document: "v1", disk: "v2" }),
      },
      "1\n2\n3\n4\n5\n6\n7\n",
    );
    await ready(h);
    await vi.waitFor(() => expect(hunks(h)).toHaveLength(2));
    clickMark(h, 2);
    // 미리 본 판정이 버튼을 끄고 까닭을 적는다.
    await vi.waitFor(() =>
      expect(withTooltips(h.view).querySelector(".pw-git-popup-status")?.textContent).toBe(
        "git-gutter-stage-disk-changed",
      ),
    );
    expect(popupButton(h, "stage").disabled).toBe(true);
    expect(popupButton(h, "stage").title).toBe("git-gutter-stage-disk-changed");
    expect(source.staged).toEqual([]);
  });

  it("is off with the reason when git's diff cannot stage that hunk on its own", async () => {
    // HEAD a b c, index a B c(b → B 를 스테이징), 작업 트리 a b C. 거터 hunk 「c → C」 의 앞 줄이 index 에서 바뀌었다.
    const source = new FakeSource();
    source.stored.set("index:a.txt", textBlob("a\nB\nc\n"));
    source.diffs.set("a.txt", {
      ...workTreeDiff,
      ...unifiedFile("@@ -1,3 +1,3 @@\n a\n-B\n-c\n+b\n+C\n"),
    });
    const h = start({ text: "a\nb\nC\n", source }, "a\nb\nc\n");
    await ready(h);
    await vi.waitFor(() => expect(hunks(h)).toHaveLength(1));
    clickMark(h, 3);
    await vi.waitFor(() => expect(popupButton(h, "stage").title).toBe("git-gutter-stage-inexact"));
    expect(popupButton(h, "stage").disabled).toBe(true);
    expect(withTooltips(h.view).querySelector(".pw-git-popup-status")?.textContent).toBe(
      "git-gutter-stage-inexact",
    );
    expect(source.staged).toEqual([]);
  });
});

/** 진짜 git 의 blame(git 2.54, 격리한 HOME · 빈 전역 설정): 세 커밋으로 만든 7줄 파일의 `git blame --porcelain` 줄마다 커밋. */
const GIT_BLAME = [
  "cfea20e5e65002ca6f23d790140cd8ce47c2a818",
  "2d86d6c502ae0964fe49f476b1b01f2f908d7eef",
  "86d03a080f9462c5171338d4ba552d4a69530f52",
  "2d86d6c502ae0964fe49f476b1b01f2f908d7eef",
  "2d86d6c502ae0964fe49f476b1b01f2f908d7eef",
  "2d86d6c502ae0964fe49f476b1b01f2f908d7eef",
  "86d03a080f9462c5171338d4ba552d4a69530f52",
];
const HEAD_TEXT = "zero\none\nTWO\nthree\nfour\nfive\nsix\n";
const AUTHORS: Record<string, [string, string, number]> = {
  cfea20e5e65002ca6f23d790140cd8ce47c2a818: ["cat", "third", 1772323200],
  "86d03a080f9462c5171338d4ba552d4a69530f52": ["bob", "second", 1769904000],
  "2d86d6c502ae0964fe49f476b1b01f2f908d7eef": ["ann", "first", 1767225600],
};

/** `git.blame` 이 그 저장소에서 보내는 꼴(청크 둘, 범위 순서는 섞여 있다). */
function blameChunks(moves = true): BlameChunk[] {
  const commit = (hash: string) => {
    const [authorName, subject, authorTime] = AUTHORS[hash]!;
    return {
      hash,
      authorName,
      authorEmail: `${authorName}@x.invalid`,
      authorTime,
      subject,
      boundary: hash.startsWith("2d86"),
    };
  };
  const range = (line: number, count: number, hash: string, previous: string | null) => ({
    line,
    count,
    commit: hash,
    sourcePath: "a.txt",
    sourceLine: line,
    previous: previous ? { commit: previous, path: "a.txt" } : null,
  });
  return [
    {
      file: { commit: "a".repeat(40), lines: 7, engine: moves ? "git" : "gix", moves },
      commits: [commit("86d03a080f9462c5171338d4ba552d4a69530f52")],
      ranges: [
        range(7, 1, "86d03a080f9462c5171338d4ba552d4a69530f52", "2d86"),
        range(3, 1, "86d03a080f9462c5171338d4ba552d4a69530f52", "2d86"),
      ],
    },
    {
      file: null,
      commits: [
        commit("cfea20e5e65002ca6f23d790140cd8ce47c2a818"),
        commit("2d86d6c502ae0964fe49f476b1b01f2f908d7eef"),
      ],
      ranges: [
        range(
          1,
          1,
          "cfea20e5e65002ca6f23d790140cd8ce47c2a818",
          "86d03a080f9462c5171338d4ba552d4a69530f52",
        ),
        range(2, 1, "2d86d6c502ae0964fe49f476b1b01f2f908d7eef", null),
        range(4, 3, "2d86d6c502ae0964fe49f476b1b01f2f908d7eef", null),
      ],
    },
  ];
}

function startBlame(options: { text?: string; moves?: boolean; available?: string[] } = {}) {
  const source = new FakeSource();
  source.blames.set("a.txt", blameChunks(options.moves ?? true));
  return start(
    { text: options.text ?? HEAD_TEXT, source, available: options.available ?? [] },
    HEAD_TEXT,
  );
}

const annotationTexts = (h: Harness) =>
  [...withTooltips(h.view).querySelectorAll<HTMLElement>(".pw-blame-annotation")].map(
    (e) => e.dataset.blameCommit ?? "uncommitted",
  );

describe("blame", () => {
  it("annotates every line with the same commit as git blame (⌃⌥A)", async () => {
    const h = startBlame();
    await ready(h);
    await vi.waitFor(() => expect(h.view.plugin(gutterLink)?.doc?.blame.kind).toBe("done"));
    expect(h.source.blameCalls).toEqual([{ path: "a.txt", rev: "a".repeat(40) }]);
    expect(annotationTexts(h)).toEqual([]);
    expect(toggleAnnotations(h.view)).toBe(true);
    await vi.waitFor(() => expect(annotationTexts(h)).toEqual(GIT_BLAME));
    expect(withTooltips(h.view).querySelector(".pw-blame-annotation")?.textContent).toContain(
      "cat",
    );
    toggleAnnotations(h.view);
    await vi.waitFor(() => expect(annotationTexts(h)).toEqual([]));
  });

  it("moves HEAD's blame onto the edited text and marks lines without a HEAD line as not committed", async () => {
    // 1번 줄 앞에 한 줄을 더하고(저장 안 함) 4번 줄(three)을 고쳤다.
    const h = startBlame();
    await ready(h);
    h.view.dispatch({ changes: { from: 0, insert: "new\n" } });
    type(h, 5, "THREE");
    await vi.waitFor(() => expect(hunks(h)).toHaveLength(2));
    toggleAnnotations(h.view);
    await vi.waitFor(() =>
      expect(annotationTexts(h)).toEqual([
        "uncommitted",
        GIT_BLAME[0],
        GIT_BLAME[1],
        GIT_BLAME[2],
        "uncommitted",
        GIT_BLAME[4],
        GIT_BLAME[5],
        GIT_BLAME[6],
      ]),
    );
  });

  it("shows the current line's blame faded at its end while focused", async () => {
    const h = startBlame();
    await ready(h);
    await vi.waitFor(() => expect(h.view.plugin(gutterLink)?.doc?.blame.kind).toBe("done"));
    focus(h);
    h.view.dispatch({ selection: { anchor: h.view.state.doc.line(3).from } });
    await vi.waitFor(() =>
      expect(withTooltips(h.view).querySelector(".pw-blame-inline")?.textContent).toContain(
        '"author":"bob"',
      ),
    );
    type(h, 3, "edited");
    await vi.waitFor(() =>
      expect(withTooltips(h.view).querySelector(".pw-blame-inline")?.textContent).toBe(
        "git-blame-uncommitted",
      ),
    );
  });

  it("tells that line moves are not tracked when blame ran without -M", async () => {
    const h = startBlame({ moves: false });
    await ready(h);
    toggleAnnotations(h.view);
    await vi.waitFor(() =>
      expect(withTooltips(h.view).querySelector(".pw-blame-notice")?.textContent).toBe(
        "git-blame-no-moves",
      ),
    );
    const tracked = startBlame({ moves: true });
    await ready(tracked);
    toggleAnnotations(tracked.view);
    await vi.waitFor(() => expect(annotationTexts(tracked)).toHaveLength(7));
    expect(tracked.view.dom.querySelector(".pw-blame-notice")).toBeNull();
  });

  it("selects the commit in the Git log on click and offers previous-revision blame and history on right click", async () => {
    const h = startBlame({ available: [REVEAL_ACTION, BLAME_REVISION_ACTION, HISTORY_ACTION] });
    await ready(h);
    toggleAnnotations(h.view);
    await vi.waitFor(() => expect(annotationTexts(h)).toHaveLength(7));
    // happy-dom 은 배치가 없어 주석 칸이 누른 자리를 맨 위 줄로 읽는다 — 첫 줄(cfea…, 앞 커밋 86d0…)을 누른다.
    const first = withTooltips(h.view).querySelector<HTMLElement>(".pw-blame-annotation")!;
    expect(first.title).toContain("cfea20e5e65002ca6f23d790140cd8ce47c2a818");
    expect(first.title).toContain("third");
    expect(first.title).toContain("git-blame-click");
    first.parentElement!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
    expect(h.ran).toEqual([
      { id: REVEAL_ACTION, args: { rev: "cfea20e5e65002ca6f23d790140cd8ce47c2a818" } },
    ]);

    first.parentElement!.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true }),
    );
    const menu = document.querySelector(".pw-blame-menu")!;
    expect(
      [...menu.querySelectorAll<HTMLElement>("[data-blame-menu]")].map((b) => b.dataset.blameMenu),
    ).toEqual(["previous", "close", "history"]);
    menu.querySelector<HTMLElement>('[data-blame-menu="previous"]')!.click();
    expect(h.ran.at(-1)).toEqual({
      id: BLAME_REVISION_ACTION,
      args: { rev: "86d03a080f9462c5171338d4ba552d4a69530f52", path: "a.txt" },
    });
    first.parentElement!.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true }),
    );
    document.querySelector<HTMLElement>('[data-blame-menu="history"]')!.click();
    expect(h.ran.at(-1)).toEqual({ id: HISTORY_ACTION, args: { path: `${TOP}/a.txt` } });
  });

  it("runs at most two blames per window, drops the oldest and fetches it again when its tab is looked at (review F5)", async () => {
    const source = new FakeSource();
    const names = ["a.txt", "b.txt", "c.txt"];
    for (const name of names) {
      source.files.set(name, textBlob("x\n"));
      source.hang.add(name);
    }
    const first = openEditor({ text: "x\n", path: `${TOP}/a.txt`, source });
    open.push(first);
    const host = first.host;
    const views = [first];
    for (const name of names.slice(1)) {
      const h = openEditor({ text: "x\n", path: `${TOP}/${name}`, source, host });
      open.push(h);
      views.push(h);
    }
    const doc = (h: Harness) => h.view.plugin(gutterLink)!.doc!;
    // 셋째 blame 이 자리를 얻으며 가장 오래된 첫째가 끊긴다.
    await vi.waitFor(() => expect(source.aborted).toEqual(["a.txt"]));
    expect(host.slots.count).toBe(2);
    expect(views.map((h) => doc(h).blame.kind)).toEqual(["idle", "loading", "loading"]);
    // 첫째 탭을 다시 본다(포커스) — 다시 받고, 이번에는 그때 가장 오래된 둘째가 끊긴다.
    vi.spyOn(first.view, "hasFocus", "get").mockReturnValue(true);
    first.view.contentDOM.dispatchEvent(new FocusEvent("focus"));
    await vi.waitFor(() => expect(source.aborted).toEqual(["a.txt", "b.txt"]));
    expect(source.blameCalls.map((c) => c.path)).toEqual(["a.txt", "b.txt", "c.txt", "a.txt"]);
    expect(views.map((h) => doc(h).blame.kind)).toEqual(["loading", "idle", "loading"]);
    // 주석 칸을 켜도 다시 받는다.
    toggleAnnotations(views[1]!.view);
    await vi.waitFor(() => expect(source.aborted).toEqual(["a.txt", "b.txt", "c.txt"]));
    expect(views.map((h) => doc(h).blame.kind)).toEqual(["loading", "loading", "idle"]);
    expect(host.slots.count).toBe(2);
  });

  it("hides the Git log, previous-revision and history entries while those actions are unavailable", async () => {
    const h = startBlame({ available: [] });
    await ready(h);
    toggleAnnotations(h.view);
    await vi.waitFor(() => expect(annotationTexts(h)).toHaveLength(7));
    const first = withTooltips(h.view).querySelector<HTMLElement>(".pw-blame-annotation")!;
    expect(first.title).not.toContain("git-blame-click");
    first.parentElement!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
    expect(h.ran).toEqual([]);
    first.parentElement!.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, cancelable: true }),
    );
    expect(
      [...document.querySelectorAll<HTMLElement>(".pw-blame-menu [data-blame-menu]")].map(
        (b) => b.dataset.blameMenu,
      ),
    ).toEqual(["close"]);
  });
});
