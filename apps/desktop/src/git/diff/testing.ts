// 시험용: git 의 patch(`git show` · `git diff` 의 출력)를 읽는다 — 파일마다 hunk(`git.diff` 의 꼴)와 `-` · `+` 줄 번호. 창 시험은
// 칠한 줄을 이것과 견준다(진짜 git 의 출력이 정본이다).

import { EditorView } from "@codemirror/view";
import type { DiffFile, DiffHunk, DiffLine } from "@pitwall/rpc-client";

import type { DiffController } from "./diff-view";

export interface PatchFile {
  /** 새 경로(지운 파일은 옛 경로). */
  path: string;
  hunks: DiffHunk[];
  /** `-` 줄의 옛 쪽 번호. */
  deleted: number[];
  /** `+` 줄의 새 쪽 번호. */
  added: number[];
  binary: boolean;
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/;

/** `diff --git` 으로 나뉜 patch 를 파일마다 읽는다. 따옴표로 감싼 경로(공백 · 특수 글자)는 다루지 않는다. */
export function parsePatch(patch: string): PatchFile[] {
  const files: PatchFile[] = [];
  let file: PatchFile | undefined;
  let hunk: DiffHunk | undefined;
  let oldLine = 0;
  let newLine = 0;
  for (const raw of patch.split("\n")) {
    if (raw.startsWith("diff --git ")) {
      const m = / b\/(.*)$/.exec(raw);
      file = { path: m?.[1] ?? "", hunks: [], deleted: [], added: [], binary: false };
      files.push(file);
      hunk = undefined;
      continue;
    }
    if (!file) continue;
    if (!hunk) {
      if (raw.startsWith("Binary files ")) file.binary = true;
      if (raw.startsWith("+++ ") && raw !== "+++ /dev/null") file.path = raw.slice(6);
      if (raw.startsWith("--- ") && raw.startsWith("--- a/") && file.path === "")
        file.path = raw.slice(6);
    }
    const h = HUNK.exec(raw);
    if (h) {
      hunk = {
        oldStart: Number(h[1]),
        oldLines: h[2] === undefined ? 1 : Number(h[2]),
        newStart: Number(h[3]),
        newLines: h[4] === undefined ? 1 : Number(h[4]),
        header: h[5] ?? "",
        lines: [],
      };
      file.hunks.push(hunk);
      oldLine = hunk.oldLines === 0 ? hunk.oldStart + 1 : hunk.oldStart;
      newLine = hunk.newLines === 0 ? hunk.newStart + 1 : hunk.newStart;
      continue;
    }
    if (!hunk) continue;
    const mark = raw[0];
    const text = raw.slice(1);
    let line: DiffLine | undefined;
    if (mark === " ")
      line = { kind: "context", text, oldLine: oldLine++, newLine: newLine++, noNewline: false };
    else if (mark === "-") {
      file.deleted.push(oldLine);
      line = { kind: "removed", text, oldLine: oldLine++, newLine: null, noNewline: false };
    } else if (mark === "+") {
      file.added.push(newLine);
      line = { kind: "added", text, oldLine: null, newLine: newLine++, noNewline: false };
    } else if (mark === "\\") {
      const last = hunk.lines[hunk.lines.length - 1];
      if (last) last.noNewline = true;
    }
    if (line) hunk.lines.push(line);
  }
  return files;
}

/** `root` 안의 diff 뷰(DOM 에서 찾는다 — 창 시험은 손잡이가 없다). */
export function diffIn(root: ParentNode): Pick<DiffController, "mode" | "views"> | undefined {
  const host = root.querySelector<HTMLElement>(".pw-diff-host");
  if (!host) return undefined;
  const editors = [...host.querySelectorAll<HTMLElement>(".cm-editor")]
    .map((el) => EditorView.findFromDOM(el))
    .filter((v): v is EditorView => v !== null);
  if (editors.length === 0) return undefined;
  const mode = host.dataset.diffMode === "unified" ? "unified" : "split";
  return { mode, views: () => editors };
}

/** 칠한 줄: 나란히 보기는 쪽마다 `data-diff` 줄의 번호, 통합 보기는 뺀 줄 블록의 옛 번호와 더한 줄의 새 번호. */
export function paintedLines(root: ParentNode, controller: Pick<DiffController, "mode" | "views">) {
  const lineNo = (view: EditorView, el: Element) => view.state.doc.lineAt(view.posAtDOM(el)).number;
  const marked = (view: EditorView, mark: string) =>
    [...view.contentDOM.querySelectorAll(`.cm-line[data-diff="${mark}"]`)]
      .filter((el) => !el.closest(".pw-diff-deleted-block"))
      .map((el) => lineNo(view, el));
  const views = controller.views();
  if (controller.mode === "split") {
    return { deleted: marked(views[0]!, "deleted"), added: marked(views[1]!, "added") };
  }
  const deleted = [...root.querySelectorAll(".pw-diff-deleted-block [data-old-line]")].map((el) =>
    Number((el as HTMLElement).dataset.oldLine),
  );
  return { deleted, added: marked(views[0]!, "added") };
}

/** patch 하나의 첫 파일을 `git.diff` 의 파일 꼴로(그 밖의 칸은 수정된 텍스트 파일). */
export function fileOf(patch: string, extra: Partial<DiffFile> = {}): DiffFile {
  const [p] = parsePatch(patch);
  return {
    status: "modified",
    path: p!.path,
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
    added: p!.added.length,
    removed: p!.deleted.length,
    hunks: p!.hunks,
    error: null,
    ...extra,
  };
}
