// 풀 옵션 · 업데이트 다이얼로그(⌘T, 액션 `git.pull.open` — 13-m2-plan.md M2-20, `git.pull` 은 M2-09). 방법(merge · rebase)과 받을
// 원격 브랜치를 고른다. 지금 브랜치의 upstream 을 고르면 원격 · 브랜치를 넘기지 않는다(git 이 upstream 으로 정한다 — 터미널의
// `git pull` 과 같다). upstream 이 없으면 원격 브랜치를 골라야 보낸다. 미저장 문서 확인은 쓰기 실행기가 창에 묻는다(git-prompts.tsx).
// 충돌로 멈추면(`stopped`) 경고 알림만 — 계속 · 중단은 진행 중 작업 배너(M2-19)와 커밋 툴 윈도우의 몫이다.

import * as Dialog from "@radix-ui/react-dialog";
import type { GitPullMode, GitRefs, RpcClient } from "@pitwall/rpc-client";
import { useState } from "react";

import { useKeymapPopup } from "../../keymap/keymap-provider";
import type { NotificationInput } from "../../notifications/notifications";
import { BUTTON, SHEET } from "./git-dialogs";
import { useGitDialogStrings } from "./strings";
import { errorText } from "./undo";

export interface PullDeps {
  client: RpcClient;
  projectId: string;
  worktree?: string;
  refs(): GitRefs | undefined;
  notify(input: NotificationInput): void;
}

export function PullDialog({ deps, onClose }: { deps: PullDeps; onClose(): void }) {
  const T = useGitDialogStrings();
  const P = T.pull;
  useKeymapPopup(true);
  const [refs] = useState(() => deps.refs());
  const head = refs?.branches.find((b) => b.head);
  const upstream = head?.upstream ?? null;
  const remotes = refs?.remoteBranches ?? [];
  const [mode, setMode] = useState<GitPullMode>("merge");
  const [chosen, setChosen] = useState<string>(upstream?.refName ?? "");
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string>();

  const send = async () => {
    const pick = remotes.find((r) => r.refName === chosen);
    const target =
      upstream && chosen === upstream.refName
        ? {}
        : pick
          ? { remote: pick.remote, branch: pick.name.slice(pick.remote.length + 1) }
          : undefined;
    if (!target) return;
    setSending(true);
    setError(undefined);
    try {
      const result = await deps.client.call("git.pull", {
        projectId: deps.projectId,
        worktree: deps.worktree ?? null,
        mode,
        ...target,
      });
      onClose();
      deps.notify(
        result.stopped ? { level: "warning", title: P.stopped } : { level: "info", title: P.done },
      );
    } catch (e) {
      setSending(false);
      // git 의 거절은 쓰기 Task 의 실패 알림이 원문을 이미 보였다 — 다이얼로그에도 그대로 둔다.
      setError(errorText(e));
    }
  };

  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content data-testid="git-pull-dialog" className={SHEET}>
          <Dialog.Title className="text-sm font-semibold">{P.title}</Dialog.Title>
          <Dialog.Description className="text-ui-muted-foreground">
            {upstream ? P.upstream(upstream.name) : P.noUpstream}
          </Dialog.Description>
          <fieldset className="flex items-center gap-3">
            <legend className="sr-only">{P.mode}</legend>
            <span>{P.mode}</span>
            {(["merge", "rebase"] as const).map((m) => (
              <label key={m} className="flex items-center gap-1">
                <input
                  type="radio"
                  name="git-pull-mode"
                  value={m}
                  checked={mode === m}
                  onChange={() => setMode(m)}
                />
                {m === "merge" ? P.merge : P.rebase}
              </label>
            ))}
          </fieldset>
          <label className="flex items-center gap-3">
            <span>{P.branch}</span>
            <select
              aria-label={P.branch}
              value={chosen}
              onChange={(e) => setChosen(e.target.value)}
              className="min-w-0 flex-1 rounded-sm border border-ui-border bg-ui-background px-1.5 py-0.5"
            >
              {!upstream && <option value="">—</option>}
              {remotes.map((r) => (
                <option key={r.refName} value={r.refName}>
                  {r.name}
                </option>
              ))}
            </select>
          </label>
          {error && (
            <pre
              role="alert"
              className="max-h-40 overflow-auto rounded-sm border border-ui-border px-2 py-1 text-[12px] whitespace-pre-wrap text-ui-accent"
            >
              {error}
            </pre>
          )}
          <div className="flex justify-end gap-2">
            <button type="button" className={BUTTON} onClick={onClose}>
              {T.common.cancel}
            </button>
            <button
              type="button"
              autoFocus
              disabled={chosen === "" || sending}
              className={`${BUTTON} border-ui-accent`}
              onClick={() => void send()}
            >
              {P.send}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
