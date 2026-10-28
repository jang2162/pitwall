// 충돌 목록 다이얼로그(13-m2-plan M2-19, F5-13): 충돌 중인 파일마다 갈래와 [내 것] [그쪽 것] [병합…].
//
// - 목록은 열 때 `git.conflicts` 로 읽고, 고를 때마다 resolve 가 돌려준 목록으로 바꾼다. 창의 git status 가 바뀌면(터미널에서 풀었다
//   등) 다시 읽는다.
// - [내 것] · [그쪽 것] 은 그 경로를 먼저 get 해 `version` 을 받고 그것을 실어 푼다(source.ts `pickSide`). 그사이 파일이 바뀌어
//   `CONFLICT` 면 고르지 않고 알린 뒤 목록을 다시 읽는다. 지운 쪽을 고르면 그 파일을 지운다(`git rm`).
// - [병합…] 은 다이얼로그를 닫고 그 파일의 3-way 머지 문서를 연다(액션 `git.openMerge { path }`). 바이너리 · 5 MiB 초과 · 한쪽에서 지움 ·
//   서브모듈 · 심링크는 머지 문서가 없다 — 단추를 끄고 까닭을 툴팁으로 보인다(결정 M2-D4).

import * as Dialog from "@radix-ui/react-dialog";
import type { ConflictFile, ConflictPick } from "@pitwall/rpc-client";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";

import { useKeymapPopup } from "../../keymap/keymap-provider";
import { windowGitStatus } from "../git-status";
import { isConflict, isGone, type MergeSource, pickOnlyReason, pickSide } from "./source";
import type { GitMergeStrings } from "./strings";

const BUTTON =
  "rounded-sm border border-ui-border px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50 disabled:hover:bg-transparent";

type Listed =
  | { kind: "loading" }
  | { kind: "failed"; message: string }
  | { kind: "ready"; files: readonly ConflictFile[] };

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export function ConflictsDialog({
  open,
  onOpenChange,
  source,
  worktree,
  strings: T,
  onMerge,
  notify,
}: {
  open: boolean;
  onOpenChange(open: boolean): void;
  source: MergeSource | undefined;
  worktree: string | undefined;
  strings: GitMergeStrings;
  /** 그 파일의 머지 문서를 연다. */
  onMerge(path: string): void;
  notify(level: "warning" | "error", title: string, body?: string): void;
}) {
  useKeymapPopup(open);
  const [listed, setListed] = useState<Listed>({ kind: "loading" });
  const [busy, setBusy] = useState<string>();
  const status = windowGitStatus();
  const statusVersion = useSyncExternalStore(
    (l) => status.subscribe(l),
    () => status.getVersion(),
  );

  const load = useCallback(async () => {
    if (!source) return;
    try {
      const { files } = await source.conflicts(worktree);
      setListed({ kind: "ready", files });
    } catch (e) {
      setListed({ kind: "failed", message: errorText(e) });
    }
  }, [source, worktree]);

  useEffect(() => {
    if (!open) {
      setListed({ kind: "loading" });
      return;
    }
    void load();
  }, [open, load, statusVersion]);

  const pick = async (path: string, side: ConflictPick) => {
    if (!source) return;
    setBusy(path);
    try {
      const { files } = await pickSide(source, path, side, worktree);
      setListed({ kind: "ready", files });
    } catch (e) {
      if (isConflict(e) || isGone(e)) {
        notify("warning", T.conflicts.changed);
        await load();
      } else {
        notify(
          "error",
          T.results.failed(side === "ours" ? T.conflicts.ours : T.conflicts.theirs),
          errorText(e),
        );
      }
    } finally {
      setBusy(undefined);
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="git-conflicts-dialog"
          className="sheet-top fixed left-1/2 flex max-h-[70vh] w-[640px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.conflicts.title}</Dialog.Title>
          <Dialog.Description className="sr-only">{T.conflicts.title}</Dialog.Description>
          <div className="min-h-0 flex-1 overflow-auto">
            {listed.kind === "loading" && (
              <p className="text-ui-muted-foreground">{T.conflicts.loading}</p>
            )}
            {listed.kind === "failed" && (
              <p role="alert" className="text-ui-muted-foreground">
                {T.conflicts.failed(listed.message)}
              </p>
            )}
            {listed.kind === "ready" && listed.files.length === 0 && (
              <p data-testid="git-conflicts-empty" className="text-ui-muted-foreground">
                {T.conflicts.empty}
              </p>
            )}
            {listed.kind === "ready" && listed.files.length > 0 && (
              <ul className="flex flex-col">
                {listed.files.map((file) => {
                  const reason = pickOnlyReason(file);
                  const disabled = busy !== undefined;
                  return (
                    <li
                      key={file.path}
                      data-git-conflict={file.path}
                      className="flex items-center gap-2 border-b border-ui-border py-1 last:border-b-0"
                    >
                      <span
                        className="min-w-0 flex-1 truncate font-mono text-[12px]"
                        title={file.path}
                      >
                        {file.path}
                      </span>
                      <span className="flex-none text-[12px] text-git-conflicted">
                        {T.conflicts.kind(file.kind)}
                      </span>
                      <button
                        type="button"
                        data-git-pick="ours"
                        className={BUTTON}
                        disabled={disabled}
                        onClick={() => void pick(file.path, "ours")}
                      >
                        {T.conflicts.ours}
                      </button>
                      <button
                        type="button"
                        data-git-pick="theirs"
                        className={BUTTON}
                        disabled={disabled}
                        onClick={() => void pick(file.path, "theirs")}
                      >
                        {T.conflicts.theirs}
                      </button>
                      <button
                        type="button"
                        data-git-merge-open=""
                        className={BUTTON}
                        disabled={disabled || reason !== undefined}
                        title={reason && T.conflicts.pickOnly(reason)}
                        onClick={() => {
                          onOpenChange(false);
                          onMerge(file.path);
                        }}
                      >
                        {T.conflicts.merge}
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
          <div className="flex justify-end">
            <Dialog.Close asChild>
              <button type="button" className={BUTTON}>
                {T.conflicts.close}
              </button>
            </Dialog.Close>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
