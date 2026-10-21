// 줄로 이동(⌘L, IntelliJ 「Go to Line:Column」): `줄` 이나 `줄:열` 을 받아 커서를 두고 그 줄을 화면 가운데로 가져온다
// (editor-workspace.tsx 의 `placeCursor` — 범위를 넘으면 끝에 붙인다). 칸은 지금 커서(`줄:열`)를 고른 채로 열린다. 틀린 입력이면
// 닫지 않고 알린다. Esc · 바깥 누르기 · [취소] 는 닫기만 하고 에디터로 포커스를 돌린다. 떠 있는 동안 창의 키맵은 쉰다(창 팝업
// 규칙, keymap-provider.tsx). 한글을 조합하는 중의 ↩ · Esc 는 조합에 둔다(ui/ime.ts).

import type { EditorView } from "@codemirror/view";
import * as Dialog from "@radix-ui/react-dialog";
import { type ReactNode, useCallback, useState } from "react";

import { useKeymapPopup } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { isComposingKey, keepOpenWhileComposing } from "../ui/ime";
import { parseLineColumn } from "./edit-commands";
import { cursorOf, placeCursor } from "./editor-workspace";

const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover";

function GotoLineDialog({ view, onClose }: { view: EditorView; onClose(): void }) {
  const T = useProjectStrings().editor.gotoLine;
  useKeymapPopup(true);
  const now = cursorOf(view);
  const [text, setText] = useState(`${now.line}:${now.column}`);
  const [invalid, setInvalid] = useState(false);
  const close = () => {
    onClose();
    view.focus();
  };
  const go = () => {
    const target = parseLineColumn(text);
    if (!target) return setInvalid(true);
    onClose();
    placeCursor(view, target);
    view.focus();
  };
  return (
    <Dialog.Root open onOpenChange={(open) => !open && close()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="goto-line-dialog"
          onEscapeKeyDown={keepOpenWhileComposing}
          className="sheet-top fixed left-1/2 flex w-[360px] max-w-[94vw] -translate-x-1/2 flex-col gap-2 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.title}</Dialog.Title>
          <Dialog.Description className="text-[12px] text-ui-muted-foreground">
            {T.hint(`${now.line}:${now.column}`, view.state.doc.lines)}
          </Dialog.Description>
          <input
            autoFocus
            aria-label={T.input}
            placeholder={T.input}
            value={text}
            spellCheck={false}
            onFocus={(e) => e.currentTarget.select()}
            onChange={(e) => {
              setText(e.target.value);
              setInvalid(false);
            }}
            onKeyDown={(e) => {
              if (e.key !== "Enter" || isComposingKey(e.nativeEvent)) return;
              e.preventDefault();
              go();
            }}
            aria-invalid={invalid}
            className="rounded-sm border border-ui-border bg-editor-background px-2 py-1 outline-none focus:border-ui-accent"
          />
          {invalid && (
            <div role="alert" className="text-[12px] text-ui-error">
              {T.invalid}
            </div>
          )}
          <div className="flex justify-end gap-2">
            <button type="button" className={BUTTON} onClick={close}>
              {T.cancel}
            </button>
            <button type="button" className={`${BUTTON} border-ui-accent`} onClick={go}>
              {T.go}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** 줄로 이동 대화상자를 여는 함수와 그 대화상자. `element` 를 창에 그린다. */
export function useGotoLine(): { open(view: EditorView): void; element: ReactNode } {
  const [view, setView] = useState<EditorView>();
  const open = useCallback((next: EditorView) => setView(next), []);
  return {
    open,
    element: view && <GotoLineDialog view={view} onClose={() => setView(undefined)} />,
  };
}
