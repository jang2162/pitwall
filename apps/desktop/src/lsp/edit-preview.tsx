// 일괄 편집의 미리보기(15-m4-plan M4-12 ⑤, 결정 M4-D14 ④): 바뀌는 파일이 둘 이상이면 쓰기 전에 파일 목록(바뀌는 줄 수 · 파일 연산 ·
// 「에디터에만 들어감(미저장 편집 있음)」 표지)과 고른 파일의 diff(`@codemirror/merge` 의 단방향 보기 — 읽기 전용)를 보인다.
// [적용] 이면 적용기가 같은 미리 확인을 한 번 더 하고 쓴다. [취소] · Esc · 바깥 누름이면 아무것도 바뀌지 않는다. 떠 있는 동안 창의
// 키맵은 쉰다(창 팝업 규칙).

import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { unifiedMergeView } from "@codemirror/merge";
import * as Dialog from "@radix-ui/react-dialog";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import { useKeymapPopup } from "../keymap/keymap-provider";
import type { LspEditStrings } from "./edit-strings";
import type { PreviewFile } from "./workspace-edit";

const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover";
const baseName = (path: string) => path.replace(/^.*[/\\]/, "");

function DiffPane({ file }: { file: PreviewFile }) {
  const host = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!host.current) return;
    const view = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: file.after,
        extensions: [
          EditorState.readOnly.of(true),
          EditorView.editable.of(false),
          unifiedMergeView({
            original: file.before,
            mergeControls: false,
            collapseUnchanged: { margin: 3, minSize: 8 },
          }),
        ],
      }),
    });
    return () => view.destroy();
  }, [file]);
  return (
    <div
      ref={host}
      data-testid="lsp-edit-preview-diff"
      className="min-h-0 flex-1 overflow-auto bg-editor-background font-mono text-xs text-editor-foreground"
    />
  );
}

function EditPreview({
  files,
  label,
  strings: T,
  onAnswer,
}: {
  files: readonly PreviewFile[];
  label: string | undefined;
  strings: LspEditStrings["preview"];
  onAnswer(apply: boolean): void;
}) {
  useKeymapPopup(true);
  const [picked, setPicked] = useState(0);
  const file = files[picked] ?? files[0]!;
  const tag = (f: PreviewFile) =>
    f.op === "create"
      ? T.create
      : f.op === "rename"
        ? T.rename(baseName(f.from ?? ""))
        : f.op === "delete"
          ? T.delete
          : undefined;
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(false)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="lsp-edit-preview"
          className="sheet-top fixed left-1/2 flex h-[70vh] w-[880px] max-w-[96vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">
            {T.title(files.length)}
            {label !== undefined && (
              <span className="ml-2 font-normal text-ui-muted-foreground">{label}</span>
            )}
          </Dialog.Title>
          <Dialog.Description className="sr-only">{T.title(files.length)}</Dialog.Description>
          <div className="flex min-h-0 flex-1 gap-3">
            <ul
              role="listbox"
              aria-label={T.title(files.length)}
              className="w-[300px] flex-none overflow-auto rounded-sm border border-ui-border"
            >
              {files.map((f, i) => (
                <li
                  key={f.path}
                  role="option"
                  aria-selected={i === picked}
                  tabIndex={i === picked ? 0 : -1}
                  data-path={f.path}
                  onClick={() => setPicked(i)}
                  onKeyDown={(e) => {
                    if (e.key === "ArrowDown") setPicked(Math.min(i + 1, files.length - 1));
                    else if (e.key === "ArrowUp") setPicked(Math.max(i - 1, 0));
                    else return;
                    e.preventDefault();
                  }}
                  className={`cursor-default px-2 py-1 ${i === picked ? "bg-ui-hover" : ""}`}
                  title={f.path}
                >
                  <div className="truncate">{baseName(f.path)}</div>
                  <div className="flex flex-wrap gap-x-2 text-xs text-ui-muted-foreground">
                    {f.lines > 0 && <span>{T.lines(f.lines)}</span>}
                    {tag(f) !== undefined && <span>{tag(f)}</span>}
                    {f.editorOnly && (
                      <span data-testid="lsp-edit-preview-editor-only">{T.editorOnly}</span>
                    )}
                  </div>
                </li>
              ))}
            </ul>
            <DiffPane file={file} />
          </div>
          <div className="flex justify-end gap-2">
            <button type="button" className={BUTTON} onClick={() => onAnswer(false)}>
              {T.cancel}
            </button>
            <button
              type="button"
              autoFocus
              className={`${BUTTON} border-ui-accent`}
              onClick={() => onAnswer(true)}
            >
              {T.apply}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** 미리보기를 묻는 함수와 그 대화상자(`element` 를 창에 그린다). 한 번에 하나만 묻는다 — 떠 있는 동안 온 것은 취소다. */
export function useEditPreview(strings: () => LspEditStrings): {
  ask(files: readonly PreviewFile[], label: string | undefined): Promise<boolean>;
  element: ReactNode;
} {
  const [question, setQuestion] = useState<{
    files: readonly PreviewFile[];
    label: string | undefined;
    answer(apply: boolean): void;
  }>();
  const open = useRef(false);
  const ask = useCallback(
    (files: readonly PreviewFile[], label: string | undefined) =>
      new Promise<boolean>((resolve) => {
        if (open.current) return resolve(false);
        open.current = true;
        setQuestion({
          files,
          label,
          answer: (apply) => {
            open.current = false;
            setQuestion(undefined);
            resolve(apply);
          },
        });
      }),
    [],
  );
  return {
    ask,
    element: question && (
      <EditPreview
        files={question.files}
        label={question.label}
        strings={strings().preview}
        onAnswer={question.answer}
      />
    ),
  };
}
