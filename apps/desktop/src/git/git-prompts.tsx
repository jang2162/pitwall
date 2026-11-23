// git 질의(docs/design/13-m2-plan.md M2-06): 프로젝트 창이 떠 있는 동안 `git.prompts` 스트림을 열어 두고 Host 의 질문에 답한다
// (`git.prompt.answer`). 듣고 답하는 것은 이 프로젝트 창의 연결뿐이다(Host 가 연결 범위로 가른다). 창이 하나도 듣지 않으면 Host 는
// askpass 를 바로 실패시키고, 작업 트리를 바꾸는 쓰기는 하지 않는다(미저장 확인을 할 창이 없다).
//
// - 미저장 확인(`unsaved`): 쓰기가 바꿀 경로(없으면 작업 트리 전체)와 겹치는 미저장 문서가 없으면 바로 `proceed`, 있으면 M1-13 의
//   확인(저장 · 저장 안 함 · 취소, editor-workspace.tsx 의 `settleUnsaved`)을 묻는다. 저장했거나 저장하지 않기로 했으면 `proceed` —
//   저장하지 않은 문서는 쓰기 뒤 외부 변경 배너(M1-13)를 따른다. 취소 · 저장 실패면 `cancel` 이고 쓰기는 돌지 않는다.
// - askpass(`askpass`): git · ssh 의 질문(Host 가 제어 문자 · 길이를 다듬은 원문)을 다이얼로그로 묻는다. `hint` 가 `secret` 이면 비밀번호
//   칸, `confirm`(ssh 의 예 · 아니오)이면 예(`proceed`) · 아니오(`cancel`), `notice`(보안 키 안내 — ssh 가 일을 마치면 닫힌다)면
//   내리기(`cancel`)만. 꾸밈은 M2-20 몫이고 여기는 자리다.
// - 호스트 키 확인(18-m7-plan 메인 판정 M7-P17): ssh 는 처음 보는 호스트 키를 `SSH_ASKPASS_PROMPT` 힌트 **없이**(`secret` 으로) 묻고
//   답이 `yes`(또는 지문)여야 받는다 — 빈 답 · `no` 는 거절이다. 그래서 질문 글(`The authenticity of host … can't be established`)로
//   알아보고 [취소] · [예] 로 그린다. 처음 포커스는 [취소] 이고 [예] 는 `yes` 를 돌려준다. 지문 글은 그대로 보인다.
// - 질문이 닫히면(`closed` — 다른 창이 답함 · 시간 초과 · 쓰기가 끝남) 다이얼로그를 내린다.
//
// 비밀(askpass 의 답)은 로그 · 상태 · 알림에 남기지 않는다 — 다이얼로그를 내리면 칸도 사라지고, 답하지 못한 실패는 값 없이 경고만
// 남긴다.

import * as Dialog from "@radix-ui/react-dialog";
import type {
  AskpassHint,
  GitPromptAnswer,
  GitPromptRequest,
  RpcClient,
} from "@pitwall/rpc-client";
import { type FormEvent, useEffect, useRef, useState } from "react";

import type { EditorWorkspace } from "../editor/editor-workspace";
import { joinPath, separatorOf } from "../file-tree/tree-model";
import { useKeymapPopup } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { useHost } from "../rpc/host";
import { LOCAL_MACHINE, qualify } from "../checkout/checkout";

const LOG = "[git]";
const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover";

export type UnsavedRequest = Extract<GitPromptRequest, { kind: "unsaved" }>;

/** 미저장 확인에 쓰는 작업 공간의 기능. */
export type UnsavedGuard = Pick<EditorWorkspace, "unsavedUnder" | "settleUnsaved">;

/** 열린 askpass 질문. */
export interface AskpassPrompt {
  promptId: number;
  text: string;
  hint: AskpassHint;
}

/** `workTree` 기준 상대 경로(`/` 구분, git RPC 의 꼴)를 그 작업 트리의 Host 경로로 잇는다 — Windows 의 작업 트리면 `\` 로. */
export function hostPath(workTree: string, relative: string): string {
  const sep = separatorOf(workTree);
  return relative
    .split("/")
    .filter((part) => part !== "")
    .reduce((dir, part) => joinPath(dir, part, sep), workTree);
}

/**
 * 미저장 확인의 답: 바뀔 경로와 겹치는 미저장 문서가 없으면 묻지 않고 `proceed`, 있으면 M1-13 의 확인대로. `machine` 은 질문을 보낸
 * Host 의 머신 id 다 — 질문의 경로는 그 머신의 경로(접두 없음)이고, 다른 머신의 탭 문서 열쇠는 `pitwall-remote://` 접두가 붙어 있어
 * 접두를 붙여 찾는다(checkout/checkout.ts `qualify`). 붙이지 않으면 원격 미저장 문서를 못 찾아 묻지 않고 git 이 파일을 바꾼다
 * (18-m7-plan M7-17, 메인 판정 2026-11-23).
 */
export async function answerUnsaved(
  request: UnsavedRequest,
  guard: UnsavedGuard,
  machine: string = LOCAL_MACHINE,
): Promise<GitPromptAnswer> {
  const targets = (
    request.paths === null
      ? [request.workTree]
      : request.paths.map((path) => hostPath(request.workTree, path))
  ).map((path) => qualify(machine, path));
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
          handlers.askpass({ promptId, text: request.text, hint: request.hint });
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

const CANCEL: GitPromptAnswer = { kind: "cancel" };

/**
 * ssh 의 호스트 키 확인 질문인가(OpenSSH `sshconnect.c` 의 문장 — 힌트 없이 온다). 글의 **맨 앞**에서만 본다 — 원격이 고르는
 * keyboard-interactive 질문은 OpenSSH 가 `(user@host) ` 를 앞에 붙이므로 그 문장을 넣어도 여기 걸리지 않는다(리뷰 R1).
 */
export function isHostKeyQuestion(text: string): boolean {
  return /^The authenticity of host '[^\n]*' can't be established\./.test(text);
}

/** 창에 보일 askpass 질문 글의 최대 글자 수(`pw_git::write::MAX_PROMPT_CHARS` 와 같다). */
export const MAX_PROMPT_CHARS = 512;

/**
 * askpass 질문 글을 보일 꼴로: 줄바꿈만 남기고 제어 문자(C0 · DEL · C1 — 탭은 빈칸)와 양방향 제어 문자(방향 표식 · 덮어쓰기 ·
 * 고립)를 빼고, `MAX_PROMPT_CHARS` 글자(코드 포인트)에서 잘라 `…` 를 붙인다. Host 가 이미 같은 규칙(`pw_git::write::askpass_text`)
 * 으로 다듬어 보내지만 원격 Host(Relay)의 글은 Desktop 이 다시 걷는다 — Desktop 은 Relay 를 믿지 않는다(18-m7-plan 메인 판정
 * M7-P19, M7-15 리뷰 F5). 같은 규칙이라 두 번 걸러도 같다.
 */
export function askpassText(raw: string): string {
  let out = "";
  let count = 0;
  for (const c of raw) {
    let kept: string | undefined = c;
    if (c === "\t") kept = " ";
    else if (c !== "\n" && /[\p{Cc}\u061C\u200E\u200F\u202A-\u202E\u2066-\u2069]/u.test(c))
      kept = undefined;
    if (kept === undefined) continue;
    if (count === MAX_PROMPT_CHARS) return `${out}…`;
    out += kept;
    count += 1;
  }
  return out;
}

/**
 * askpass 질문의 다이얼로그(git · 원격 호스트의 ssh 가 같이 쓴다). `machine` 은 질문을 낸 다른 머신의 이름(Desktop 의 별명 — Relay 가
 * 댄 이름이 아니다, 18-m7-plan 메인 판정 M7-P19)이다. 있으면 제목 위에 「<머신> 의 git · ssh 가 묻는다」 를 글로 보인다 — 사람이
 * 원격의 질문을 이 Mac 의 것으로 읽고 암호를 넣지 않게(M7-15). 이 머신의 질문이면 비운다.
 */
export function AskpassDialog({
  prompt,
  machine,
  onAnswer,
}: {
  prompt: AskpassPrompt;
  machine?: string;
  onAnswer(answer: GitPromptAnswer): void;
}) {
  const T = useProjectStrings().git.askpass;
  const [value, setValue] = useState("");
  useKeymapPopup(true);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onAnswer({ kind: "askpass", value });
  };
  const text = askpassText(prompt.text);
  const hostKey = prompt.hint === "secret" && isHostKeyQuestion(text);
  const title = hostKey
    ? T.hostKeyTitle
    : { secret: T.title, confirm: T.confirmTitle, notice: T.noticeTitle }[prompt.hint];
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onAnswer(CANCEL)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="git-askpass-dialog"
          className="sheet-top fixed left-1/2 flex w-[460px] max-w-[94vw] -translate-x-1/2 flex-col gap-3 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          {machine !== undefined && (
            <p data-testid="git-askpass-machine" className="text-xs text-ui-muted-foreground">
              {T.machine(machine)}
            </p>
          )}
          <Dialog.Title className="text-sm font-semibold">{title}</Dialog.Title>
          <Dialog.Description className="break-all whitespace-pre-line text-ui-muted-foreground">
            {text}
          </Dialog.Description>
          {hostKey && (
            <div className="flex justify-end gap-2">
              {/* 처음 포커스는 [취소] 다 — 모르는 호스트 키를 받아들이는 것은 보안 결정이다(M7-P17). [예] 는 ssh 가 받는 `yes` 다. */}
              <button type="button" autoFocus className={BUTTON} onClick={() => onAnswer(CANCEL)}>
                {T.cancel}
              </button>
              <button
                type="button"
                className={`${BUTTON} border-ui-accent`}
                onClick={() => onAnswer({ kind: "askpass", value: "yes" })}
              >
                {T.yes}
              </button>
            </div>
          )}
          {prompt.hint === "secret" && !hostKey && (
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
                <button type="button" className={BUTTON} onClick={() => onAnswer(CANCEL)}>
                  {T.cancel}
                </button>
                <button type="submit" className={`${BUTTON} border-ui-accent`}>
                  {T.submit}
                </button>
              </div>
            </form>
          )}
          {prompt.hint === "confirm" && (
            <div className="flex justify-end gap-2">
              {/* 처음 포커스는 「아니오」 다 — 모르는 호스트 키를 받아들이는 것은 보안 결정이라 뜬 순간 친 Enter · Space 로 예가 되면
                  안 된다(신뢰 확인 · 위험 작업 확인과 같은 원칙). */}
              <button type="button" autoFocus className={BUTTON} onClick={() => onAnswer(CANCEL)}>
                {T.no}
              </button>
              <button
                type="button"
                className={`${BUTTON} border-ui-accent`}
                onClick={() => onAnswer({ kind: "proceed" })}
              >
                {T.yes}
              </button>
            </div>
          )}
          {prompt.hint === "notice" && (
            <div className="flex justify-end gap-2">
              <button type="button" autoFocus className={BUTTON} onClick={() => onAnswer(CANCEL)}>
                {T.dismiss}
              </button>
            </div>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * 프로젝트 창의 git 질의. 창이 떠 있는 동안 둔다. 원격 체크아웃의 질의(그 머신 연결 — M7-11b 의 `useCheckoutHost` 가 띄운다)면
 * `machine` 에 그 머신의 이름을 준다.
 */
export function GitPrompts({
  projectId,
  workspace,
  machine,
  machineId = LOCAL_MACHINE,
}: {
  projectId: string;
  workspace: UnsavedGuard;
  /** 다이얼로그에 보일 머신 이름. */
  machine?: string;
  /** 이 연결의 머신 id — 미저장 확인이 경로에 접두를 붙인다(`answerUnsaved`). */
  machineId?: string;
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
      unsaved: (request) => answerUnsaved(request, latest.current, machineId),
      askpass: (prompt) => setAskpass((queue) => [...queue, prompt]),
      closed: (promptId) => setAskpass((queue) => queue.filter((p) => p.promptId !== promptId)),
    });
  }, [client, projectId, machineId]);
  const current = askpass[0];
  if (!current || !client) return null;
  const answer = (reply: GitPromptAnswer) => {
    setAskpass((queue) => queue.filter((p) => p.promptId !== current.promptId));
    client
      .call("git.prompt.answer", { projectId, promptId: current.promptId, answer: reply })
      .catch((error: unknown) =>
        console.warn(LOG, "could not answer the git prompt", current.promptId, error),
      );
  };
  return (
    <AskpassDialog key={current.promptId} prompt={current} machine={machine} onAnswer={answer} />
  );
}
