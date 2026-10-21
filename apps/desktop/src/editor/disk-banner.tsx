// 외부 변경 배너(12-m1-plan M1-13, F3-6 · F4-2): 디스크의 파일이 이 탭의 문서와 달라졌을 때 텍스트 뷰어 위에 뜬다. 상태는
// 문서 모델의 `disk`(documents.ts)다 — 미저장 편집이 없으면 문서가 새 내용을 바로 받으므로 배너가 뜨지 않는다.
//
// - 바뀜(미저장이 있어 받지 않음 · 저장이 `CONFLICT`): [디스크 것 받기] [내 것 유지] [비교]. [비교] 는 뷰어 자리에 두 내용을
//   나란히(`@codemirror/merge` 의 MergeView, 왼쪽 내 것 · 오른쪽 디스크) 보인다. 읽기만 한다 — 고르는 것은 배너의 두 단추다.
// - 지워짐: 저장하면 다시 만든다는 안내와 [닫기].
// - 이름 바뀜(지워진 것과 같은 때 같은 내용의 파일이 생김): [새 이름으로 열기] [닫기].
// 같은 문서를 보는 탭마다 배너가 뜬다 — 단추는 문서에 하므로 한 탭에서 고르면 모두 걷힌다.

import { MergeView } from "@codemirror/merge";
import { EditorState, type Text } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import type { FsRead } from "@pitwall/rpc-client";
import { useEffect, useRef } from "react";

import { useProjectStrings } from "../layout/strings";
import type { TextDocumentState } from "./documents";
import { useEditorWorkspace } from "./editor-workspace";

const BUTTON =
  "flex-none rounded-sm border border-ui-border px-2 py-px text-ui-foreground hover:bg-ui-hover";

export function DiskBanner({
  path,
  worktree,
  doc,
  comparing,
  onCompare,
}: {
  path: string;
  /** 탭의 워크트리. 새 이름도 그 워크트리로 연다. */
  worktree: string | undefined;
  doc: TextDocumentState;
  comparing: boolean;
  onCompare(): void;
}) {
  const T = useProjectStrings().editor.disk;
  const workspace = useEditorWorkspace();
  const { documents } = workspace;
  const { disk } = doc;
  if (disk.kind === "same") return null;
  const failed = (e: unknown) => console.warn("[editor] banner action failed", path, e);
  return (
    <div
      role="alert"
      data-testid="disk-banner"
      data-disk={disk.kind}
      className="flex flex-none flex-wrap items-center gap-2 border-b border-ui-accent bg-ui-panel px-2 py-1 text-[12px]"
    >
      <span className="min-w-0 flex-1 truncate">
        {disk.kind === "changed"
          ? T.changed
          : disk.kind === "deleted"
            ? T.deleted
            : T.renamed(disk.to)}
      </span>
      {disk.kind === "changed" && (
        <>
          <button
            type="button"
            className={BUTTON}
            onClick={() => void documents.takeDisk(path).catch(failed)}
          >
            {T.take}
          </button>
          <button type="button" className={BUTTON} onClick={() => documents.keepMine(path)}>
            {T.keep}
          </button>
          {disk.read.content.kind === "text" && (
            <button type="button" className={BUTTON} aria-pressed={comparing} onClick={onCompare}>
              {comparing ? T.compareClose : T.compare}
            </button>
          )}
        </>
      )}
      {disk.kind === "renamed" && (
        <button
          type="button"
          className={BUTTON}
          onClick={() => void workspace.open({ path: disk.to, worktree })}
        >
          {T.openRenamed}
        </button>
      )}
      {disk.kind !== "changed" && (
        <button type="button" className={BUTTON} onClick={() => documents.keepMine(path)}>
          {T.dismiss}
        </button>
      )}
    </div>
  );
}

const readOnlySide = [
  lineNumbers(),
  EditorState.readOnly.of(true),
  EditorView.editable.of(false),
  EditorView.theme({ "&": { height: "100%" } }),
];

/** 내 것(편집 중)과 디스크를 나란히. 연 순간의 두 내용을 보인다. */
export function CompareView({ mine, disk }: { mine: Text; disk: FsRead }) {
  const T = useProjectStrings().editor.disk;
  const host = useRef<HTMLDivElement>(null);
  const diskText = disk.content.kind === "text" ? disk.content.text : "";
  useEffect(() => {
    const parent = host.current;
    if (!parent) return;
    const view = new MergeView({
      parent,
      a: { doc: mine, extensions: readOnlySide },
      b: { doc: diskText, extensions: readOnlySide },
      gutter: true,
      highlightChanges: true,
    });
    return () => view.destroy();
  }, [mine, diskText]);
  return (
    <div data-testid="disk-compare" className="flex min-h-0 flex-1 flex-col">
      <div className="grid flex-none grid-cols-2 border-b border-ui-border text-[12px] text-ui-muted-foreground">
        <span className="px-2 py-0.5">{T.compareMine}</span>
        <span className="border-l border-ui-border px-2 py-0.5">{T.compareDisk}</span>
      </div>
      <div ref={host} className="min-h-0 flex-1 overflow-auto select-text" />
    </div>
  );
}
