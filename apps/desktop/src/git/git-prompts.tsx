// git 질의(docs/design/13-m2-plan.md M2-06): 프로젝트 창이 떠 있는 동안 `git.prompts` 스트림을 열어 두고 Host 의 질문에 답한다
// (`git.prompt.answer`). 창이 하나도 듣지 않으면 Host 는 askpass 를 바로 실패시키고 미저장 확인 없이 쓰기를 한다.
//
// - 미저장 확인(`unsaved`): 쓰기가 바꿀 경로(없으면 작업 트리 전체)와 겹치는 미저장 문서가 없으면 바로 `proceed`, 있으면 M1-13 의
//   확인(저장 · 저장 안 함 · 취소, editor-workspace.tsx 의 `settleUnsaved`)을 묻는다. 저장했거나 저장하지 않기로 했으면 `proceed` —
//   저장하지 않은 문서는 쓰기 뒤 외부 변경 배너(M1-13)를 따른다. 취소 · 저장 실패면 `cancel` 이고 쓰기는 돌지 않는다.
// - askpass(`askpass`): git · ssh 의 질문(원문)을 비밀번호 칸 다이얼로그로 묻는다. 꾸밈은 M2-20 몫이고 여기는 자리다.
// - 질문이 닫히면(`closed` — 다른 창이 답함 · 시간 초과 · 쓰기가 끝남) 다이얼로그를 내린다.
//
// 비밀(askpass 의 답)은 로그 · 상태 · 알림에 남기지 않는다 — 다이얼로그를 내리면 칸도 사라지고, 답하지 못한 실패는 값 없이 경고만
// 남긴다.

import * as Dialog from "@radix-ui/react-dialog";
import type { GitPromptAnswer, GitPromptRequest, RpcClient } from "@pitwall/rpc-client";
import { type FormEvent, useEffect, useRef, useState } from "react";

import type { EditorWorkspace } from "../editor/editor-workspace";
import { joinPath, separatorOf } from "../file-tree/tree-model";
import { useKeymapPopup } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { useHost } from "../rpc/host";

const LOG = "[git]";
const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover";

export type UnsavedRequest = Extract<GitPromptRequest, { kind: "unsaved" }>;

/** 미저장 확인에 쓰는 작업 공간의 기능. */
export type UnsavedGuard = Pick<EditorWorkspace, "unsavedUnder" | "settleUnsaved">;

/** 열린 askpass 질문. */
export interface AskpassPrompt {
  promptId: number;
  text: string;
}

/** `workTree` 기준 상대 경로(`/` 구분, git RPC 의 꼴)를 그 작업 트리의 Host 경로로 잇는다 — Windows 의 작업 트리면 `\` 로. */
export function hostPath(workTree: string, relative: string): string {
  const sep = separatorOf(workTree);
  return relative
    .split("/")
    .filter((part) => part !== "")
    .reduce((dir, part) => joinPath(dir, part, sep), workTree);
}

/** 미저장 확인의 답: 바뀔 경로와 겹치는 미저장 문서가 없으면 묻지 않고 `proceed`, 있으면 M1-13 의 확인대로. */
export async function answerUnsaved(
  request: UnsavedRequest,
  guard: UnsavedGuard,
): Promise<GitPromptAnswer> {
  const targets =
    request.paths === null
      ? [request.workTree]
      : request.paths.map((path) => hostPath(request.workTree, path));
  const overlapping = [...new Set(targets.flatMap((path) => guard.unsavedUnder(path)))];
  if (overlapping.length === 0) return { kind: "proceed" };
  return (await guard.settleUnsaved(overlapping)) ? { kind: "proceed" } : { kind: "cancel" };
}

export interface PromptHandlers {
  unsaved(request: UnsavedRequest): Promise<GitPromptAnswer>;
  askpass(prompt: AskpassPrompt): void;
  closed(promptId: number): void;
}

/**
 * `projectId` 의 `git.prompts` 를 열어 두고 질문을 `handlers` 에 넘긴다. 미저장 확인은 정한 답을 바로 보낸다. 스트림이 끊기면 경고만
 * 남긴다 — 그 창에 질문이 오지 않을 뿐이다(Host 는 듣는 창이 없는 것으로 본다). 돌려준 함수가 스트림을 닫는다.
 */
export function listenGitPrompts(
  client: RpcClient,
  projectId: string,
  handlers: PromptHandlers,
): () => void {
  const abort = new AbortController();
  const reply = (promptId: number, answer: GitPromptAnswer) =>
    client.call("git.prompt.answer", { projectId, promptId, answer }).catch((error: unknown) => {
      // 질문이 그사이 닫혔으면(다른 창이 답함 · 시간 초과) not found 다. 답의 값은 남기지 않는다.
      if (!abort.signal.aborted) console.warn(LOG, "could not answer a prompt", promptId, error);
    });
  void (async () => {
    try {
      for await (const event of client.stream(
        "git.prompts",
        { projectId },
        { signal: abort.signal },
      )) {
        if (abort.signal.aborted) return;
        if (event.kind === "closed") {
          handlers.closed(event.promptId);
          continue;
        }
        const { promptId, request } = event.prompt;
        if (request.kind === "askpass") {
          handlers.askpass({ promptId, text: request.text });
        } else {
          handlers
            .unsaved(request)
            .catch((error: unknown) => {
              console.warn(LOG, "the unsaved check failed; cancelling the write", error);
              return { kind: "cancel" } as const;
            })
            .then((answer) => reply(promptId, answer));
        }
      }
    } catch (error) {
      if (!abort.signal.aborted) console.warn(LOG, "stopped listening for git prompts", error);
    }
  })();
  return () => abort.abort();
}

function AskpassDialog({
  prompt,
  onAnswer,
}: {
  prompt: AskpassPrompt;
  onAnswer(value: string | undefined): void;
}) {
  const T = useProjectStrings().git.askpass;
  const [value, setValue] = useState("");
  useKeymapPopup(true);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onAnswer(value);
  };
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(undefined)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="git-askpass-dialog"
          className="sheet-top fixed left-1/2 flex w-[460px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.title}</Dialog.Title>
          <Dialog.Description className="break-all whitespace-pre-line text-ui-muted-foreground">
            {prompt.text}
          </Dialog.Description>
          <form className="flex flex-col gap-3" onSubmit={submit}>
            <input
              type="password"
              aria-label={T.input}
              autoFocus
              autoComplete="off"
              spellCheck={false}
              className="rounded-sm border border-ui-border bg-ui-background px-2 py-1"
              value={value}
              onChange={(e) => setValue(e.target.value)}
            />
            <div className="flex justify-end gap-2">
              <button type="button" className={BUTTON} onClick={() => onAnswer(undefined)}>
                {T.cancel}
              </button>
              <button type="submit" className={`${BUTTON} border-ui-accent`}>
                {T.submit}
              </button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** 프로젝트 창의 git 질의. 창이 떠 있는 동안 둔다. */
export function GitPrompts({
  projectId,
  workspace,
}: {
  projectId: string;
  workspace: UnsavedGuard;
}) {
  const { client } = useHost();
  // 한 번에 하나를 묻고 나머지는 차례를 기다린다.
  const [askpass, setAskpass] = useState<readonly AskpassPrompt[]>([]);
  const latest = useRef(workspace);
  latest.current = workspace;
  useEffect(() => {
    if (!client) return;
    setAskpass([]);
    return listenGitPrompts(client, projectId, {
      unsaved: (request) => answerUnsaved(request, latest.current),
      askpass: (prompt) => setAskpass((queue) => [...queue, prompt]),
      closed: (promptId) => setAskpass((queue) => queue.filter((p) => p.promptId !== promptId)),
    });
  }, [client, projectId]);
  const current = askpass[0];
  if (!current || !client) return null;
  const answer = (value: string | undefined) => {
    setAskpass((queue) => queue.filter((p) => p.promptId !== current.promptId));
    const reply: GitPromptAnswer =
      value === undefined ? { kind: "cancel" } : { kind: "askpass", value };
    client
      .call("git.prompt.answer", { projectId, promptId: current.promptId, answer: reply })
      .catch((error: unknown) =>
        console.warn(LOG, "could not answer the git prompt", current.promptId, error),
      );
  };
  return <AskpassDialog key={current.promptId} prompt={current} onAnswer={answer} />;
}
