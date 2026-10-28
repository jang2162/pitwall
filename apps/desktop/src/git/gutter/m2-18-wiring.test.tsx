// @vitest-environment happy-dom
// 행 사이 이음매(13-m2-plan M2-17 ↔ M2-18)를 진짜 액션으로 본다: blame 주석 칸의 클릭 · 우클릭이 진짜 레지스트리에 등록된 M2-18 의
// `git.log.reveal`(src/git/log/log-reveal.ts)과 `git.showFileHistory`(src/git/history/git-history.tsx)를 부르고, 그 액션이 받은
// 인자로 Git 로그의 고를 커밋 · 파일 히스토리 문서를 연다. 창(git-gutter.tsx)도 같은 `registry.isAvailable` · `registry.run` 으로 잇는다.

import { ActionRegistry } from "@pitwall/keymap";
import { afterEach, expect, it, vi } from "vitest";

import type { GitDocumentSpec } from "../documents/spec";
import { fileHistoryAction } from "../history/git-history";
import { gitHistoryStrings } from "../history/strings";
import { logRevealAction, logRevealRequests } from "../log/log-reveal";
import { toggleAnnotations } from "./blame-extension";
import { gutterLink } from "./gutter-extension";
import { FakeSource, type Harness, openEditor, textBlob, TOP } from "./testing";

let h: Harness | undefined;
afterEach(() => {
  h?.destroy();
  h = undefined;
  document.querySelectorAll(".pw-blame-menu").forEach((m) => m.remove());
});

it("calls M2-18's real git.log.reveal and git.showFileHistory from the blame annotations", async () => {
  const registry = new ActionRegistry();
  registry.register(logRevealAction("reveal", () => undefined));
  const opened: { spec: GitDocumentSpec; worktree: string | undefined }[] = [];
  registry.register(
    fileHistoryAction({
      strings: () => gitHistoryStrings((id) => id),
      ready: () => true,
      current: () => undefined,
      nested: () => undefined,
      workTree: async () => TOP,
      open: (spec, _title, worktree) => opened.push({ spec, worktree }),
      notify: (level, title) => {
        throw new Error(`unexpected ${level} notice: ${title}`);
      },
    }),
  );
  const hash = "cfea20e5e65002ca6f23d790140cd8ce47c2a818";
  const source = new FakeSource();
  source.files.set("src/a.rs", textBlob("one\n"));
  source.blames.set("src/a.rs", [
    {
      file: { commit: "a".repeat(40), lines: 1, engine: "git", moves: true },
      commits: [
        {
          hash,
          authorName: "ann",
          authorEmail: "a@x",
          authorTime: 0,
          subject: "s",
          boundary: false,
        },
      ],
      ranges: [
        { line: 1, count: 1, commit: hash, sourcePath: "src/a.rs", sourceLine: 1, previous: null },
      ],
    },
  ]);
  h = openEditor({ text: "one\n", path: `${TOP}/src/a.rs`, source, registry });
  await vi.waitFor(() => expect(h!.view.plugin(gutterLink)?.doc?.blame.kind).toBe("done"));
  toggleAnnotations(h.view);
  await vi.waitFor(() => expect(h!.view.dom.querySelector(".pw-blame-annotation")).not.toBeNull());
  const cell = h.view.dom.querySelector<HTMLElement>(".pw-blame-annotation")!;
  // 진짜 액션이 등록돼 있으니 툴팁이 클릭을 알린다.
  expect(cell.title).toContain("git-blame-click");

  const before = logRevealRequests.getState().seq;
  cell.parentElement!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
  expect(logRevealRequests.getState()).toMatchObject({ rev: hash, seq: before + 1 });

  cell.parentElement!.dispatchEvent(
    new MouseEvent("contextmenu", { bubbles: true, cancelable: true }),
  );
  document.querySelector<HTMLElement>('[data-blame-menu="history"]')!.click();
  await vi.waitFor(() => expect(opened).toHaveLength(1));
  expect(opened[0]!.spec).toEqual({ kind: "history", path: "src/a.rs" });
});
