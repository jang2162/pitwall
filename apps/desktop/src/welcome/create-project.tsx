// S3 프로젝트 생성 시트(10-ui §2.3). 이름 · 루트 · 설정 저장 위치를 받아 `project.create` 를 부른다. 자동 감지(F1-4)는
// 아직 없으므로 「감지됨」 은 빈 목록이다. 만들기는 `⌘↩`(팝업 액션 `project.create`), 취소는 Esc · 취소 버튼.
// Enter 로 만들지 않는다 — 이름 칸에서 한글 조합을 확정하는 Enter 가 폼 제출로 새지 않게. 조합을 취소하는 Esc 는 시트를 닫지
// 않는다(ui/ime.ts).

import type { ConfigMode, Project } from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import { useMemo, useRef, useState } from "react";

import { useKeymap, useKeymapPopup, useRegisterActions } from "../keymap/keymap-provider";
import { useHost } from "../rpc/host";
import { keepOpenWhileComposing } from "../ui/ime";
import { createProjectActions } from "./actions";
import { baseName, errorText } from "./projects";
import { STRINGS } from "./strings";

const T = STRINGS.create;

const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";

export function CreateProjectSheet({
  initialRoot,
  readOnly,
  pickFolder,
  onWriteError,
  onCancel,
  onCreated,
}: {
  /** 비어 있으면 루트를 먼저 고른다(「새 프로젝트…」). */
  initialRoot: string;
  /** 레지스트리가 쓰기를 거절하는 상태(storage 배너). */
  readOnly: boolean;
  pickFolder(defaultPath?: string): Promise<string | null>;
  /** 만들기가 거절됐을 때(시트 안 오류 표시와 별도로 — 저장소 상태 갱신 등). */
  onWriteError(e: unknown): void;
  onCancel(): void;
  /** 만든 뒤. 시트를 닫고 프로젝트 창을 여는 것은 부른 쪽이 한다. */
  onCreated(project: Project): Promise<void>;
}) {
  const { client } = useHost();
  const { shortcuts } = useKeymap();
  const [root, setRoot] = useState(initialRoot);
  const [name, setName] = useState(baseName(initialRoot));
  const nameEdited = useRef(false);
  const [configMode, setConfigMode] = useState<ConfigMode>("repository");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  useKeymapPopup(true);

  const canSubmit = !busy && !readOnly && client !== undefined && root !== "" && name.trim() !== "";
  const submit = useRef<() => Promise<void>>(async () => {});
  submit.current = async () => {
    if (!canSubmit || !client) return;
    setBusy(true);
    setError(undefined);
    try {
      const project = await client.call("project.create", {
        root,
        name: name.trim(),
        configMode,
      });
      await onCreated(project);
    } catch (e) {
      setError(STRINGS.errors.action(STRINGS.errors.create, errorText(e)));
      setBusy(false);
      onWriteError(e);
    }
  };
  const enabled = useRef<() => boolean>(() => false);
  enabled.current = () => canSubmit;

  const actions = useMemo(
    () =>
      createProjectActions({ canSubmit: () => enabled.current(), submit: () => submit.current() }),
    [],
  );
  useRegisterActions(actions);

  async function changeRoot() {
    const picked = await pickFolder(root || undefined).catch((e: unknown) => {
      setError(STRINGS.errors.action(STRINGS.errors.pickFolder, errorText(e)));
      return null;
    });
    if (picked === null) return;
    setRoot(picked);
    if (!nameEdited.current) setName(baseName(picked));
  }

  const submitKey = shortcuts("project.create")[0]?.text;
  return (
    <Dialog.Root open onOpenChange={(open) => !open && !busy && onCancel()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          aria-describedby={undefined}
          onEscapeKeyDown={keepOpenWhileComposing}
          data-keymap-context="createProjectSheet"
          className="sheet-top fixed left-1/2 flex w-[560px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">
            {T.title(name.trim() || baseName(root))}
          </Dialog.Title>
          <div className="flex items-center gap-2">
            <label className="flex items-center gap-2">
              <span>{T.name}</span>
              <input
                autoFocus
                value={name}
                onChange={(e) => {
                  nameEdited.current = true;
                  setName(e.target.value);
                }}
                className="w-[180px] rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5 outline-none focus:border-ui-accent"
              />
            </label>
            <span className="ml-2">{T.root}</span>
            <span className="min-w-0 flex-1 truncate text-ui-muted-foreground" title={root}>
              {root || T.rootNone}
            </span>
            <button type="button" className={BUTTON} onClick={() => void changeRoot()}>
              {root ? T.changeRoot : T.chooseRoot}
            </button>
          </div>
          <div role="radiogroup" aria-label={T.configMode} className="flex items-center gap-3">
            <span>{T.configMode}</span>
            {(
              [
                ["repository", T.repository],
                ["personal", T.personal],
              ] as const
            ).map(([mode, label]) => (
              <label key={mode} className="flex items-center gap-1">
                <input
                  type="radio"
                  name="configMode"
                  value={mode}
                  checked={configMode === mode}
                  onChange={() => setConfigMode(mode)}
                />
                {label}
              </label>
            ))}
          </div>
          <section>
            <h3 className="mb-1 text-[12px] font-semibold">{T.detected}</h3>
            <p className="text-ui-muted-foreground">{T.detectedEmpty}</p>
          </section>
          {error && (
            <p role="alert" className="text-[12px] text-ui-accent">
              {error}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <button type="button" className={BUTTON} disabled={busy} onClick={onCancel}>
              {T.cancel}
            </button>
            <button
              type="button"
              className={`${BUTTON} border-ui-accent`}
              disabled={!canSubmit}
              onClick={() => void submit.current()}
            >
              {T.submit}
              {submitKey && <kbd className="ml-2 font-sans text-[11px]">{submitKey}</kbd>}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
