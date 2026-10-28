// 푸시 다이얼로그(⌘⇧K, 액션 `git.push.open` — 13-m2-plan.md M2-20 · M2-09 판정 문장, 결정 M2-D6).
//
// - 보내기 전에 `git.push.plan` 을 보인다: 보낼 곳(브랜치 → 원격 : 원격 ref)과 그것을 정한 것, 터미널의 `git push` 와의 차이
//   (`terminal` — 보통 `null`), git 이 거절할 상황이면 그 까닭과 후보(고르면 그 후보를 `remoteRef` 로 다시 계획한다). 보낼 커밋은
//   `git.push.commits`(원격에 있다고 아는 마지막 커밋 `lease.commit` 부터).
// - 보낼 때는 계획에 쓴 입력 그대로에 그 계획의 `remote` · `remoteRef` 를 `expectedTarget` 으로 싣는다 — 계획 뒤에 설정이 바뀌어
//   대상이 달라졌으면 Host 가 `-32004` 로 아무것도 보내지 않고, 다이얼로그는 계획을 다시 보인다(`2102` 도 같다). 대상을 입력
//   `remoteRef` 로 고정해 보내지 않는다 — 그러면 그 검사가 늘 맞아 바뀐 설정을 못 본다.
// - force 는 force-with-lease 하나뿐이다: 「강제 푸시」 를 고르면 보내기 전에 위험 다이얼로그로 묻고, `forceWithLease` 에 계획의
//   `lease.commit`(창이 마지막으로 본 원격 해시)을 싣는다. 원격 추적 ref 가 없으면(원격에 아직 없는 브랜치) 덮을 것이 없어 칸을 끈다.
// - git 이 거절하면(`2004`, 앞선 원격 등) 그 원문을 다이얼로그에 그대로 보이고 [풀…] 을 단다(`git.pull.open`). 원문을 해석하지
//   않는다 — 어느 거절이든 [풀…] 을 보인다.
// - 보낸 뒤에는 결과의 실제 대상(브랜치 → 원격 : 원격 ref)과 보낸 refspec, 터미널과의 차이를 알림으로 보인다.

import * as Dialog from "@radix-ui/react-dialog";
import {
  ERROR_CODE,
  type GitPushCommits,
  type GitPushPlanResult,
  type GitPushTerminal,
  type GitRefs,
  RpcCallError,
  type RpcClient,
} from "@pitwall/rpc-client";
import { useEffect, useRef, useState } from "react";

import { useKeymapPopup } from "../../keymap/keymap-provider";
import type { NotificationInput } from "../../notifications/notifications";
import { BUTTON, type GitConfirm, SHEET } from "./git-dialogs";
import { type GitDialogStrings, useGitDialogStrings } from "./strings";
import { GIT_FAILED, errorText, shortHash } from "./undo";

/** `crates/pw-git/src/remote/mod.rs` 의 `code::PUSH_REFUSED`. */
export const PUSH_REFUSED = 2102;
/** 보낼 커밋을 보이는 수. */
export const COMMITS_SHOWN = 100;

/** 계획 · 보내기에 함께 넘기는 입력. 후보를 고르면 `remoteRef` 가 생긴다. */
export interface PushInput {
  branch?: string;
  remoteRef?: string;
}

export interface PushDeps {
  client: RpcClient;
  projectId: string;
  worktree?: string;
  /** 지금 ref(창의 `windowBranch()`) — upstream 이 있는지 본다. */
  refs(): GitRefs | undefined;
  confirm(question: GitConfirm): Promise<boolean>;
  notify(input: NotificationInput): void;
  /** [풀…]. 없으면 단추가 없다. */
  pull?: () => void;
}

/** 터미널과의 차이를 줄로. */
export function terminalLines(T: GitDialogStrings["push"], terminal: GitPushTerminal): string[] {
  const lines = [T.terminalDefault(terminal.pushDefault)];
  if (terminal.configured.length > 0)
    lines.push(T.terminalConfigured(terminal.configured.join(", ")));
  if (terminal.refusal) lines.push(T.terminalRefuses(terminal.refusal.message));
  else if (terminal.sends.length === 0) lines.push(T.terminalNothing);
  else lines.push(T.terminalSends(terminal.sends.join(", ") + (terminal.truncated ? ", …" : "")));
  return lines;
}

type Load =
  | { kind: "loading" }
  | { kind: "failed"; message: string }
  | { kind: "ready"; plan: GitPushPlanResult; commits?: GitPushCommits };

export function PushDialog({
  deps,
  initial,
  onClose,
}: {
  deps: PushDeps;
  initial: PushInput;
  onClose(): void;
}) {
  const T = useGitDialogStrings();
  const P = T.push;
  useKeymapPopup(true);
  const [input, setInput] = useState<PushInput>(initial);
  const [load, setLoad] = useState<Load>({ kind: "loading" });
  const [notice, setNotice] = useState<string>();
  const [rejected, setRejected] = useState<string>();
  const [setUpstream, setSetUpstream] = useState(false);
  const [force, setForce] = useState(false);
  const [sending, setSending] = useState(false);
  const [seq, setSeq] = useState(0);
  const latest = useRef(deps);
  latest.current = deps;
  const base = { projectId: deps.projectId, worktree: deps.worktree ?? null };

  useEffect(() => {
    let stale = false;
    const { client, projectId, worktree } = latest.current;
    const base = { projectId, worktree: worktree ?? null };
    setLoad({ kind: "loading" });
    void (async () => {
      try {
        const plan = await client.call("git.push.plan", { ...base, ...input });
        if (stale) return;
        const upstream = latest.current
          .refs()
          ?.branches.find((b) => b.name === plan.branch)?.upstream;
        setSetUpstream(upstream == null);
        setLoad({ kind: "ready", plan });
        if (plan.remoteRef === null) return;
        const commits = await client.call("git.push.commits", {
          ...base,
          branch: plan.branch,
          remote: plan.remote,
          base: plan.lease?.commit ?? null,
          limit: COMMITS_SHOWN,
        });
        if (!stale) setLoad({ kind: "ready", plan, commits });
      } catch (e) {
        if (!stale) setLoad({ kind: "failed", message: errorText(e) });
      }
    })();
    return () => {
      stale = true;
    };
    // 입력이 바뀌거나 다시 계획할 때(`seq`)만 다시 읽는다 — 연결 · 프로젝트는 누른 때의 값(`latest`)이다.
  }, [input, seq]);

  const plan = load.kind === "ready" ? load.plan : undefined;
  const lease = plan?.lease?.commit ?? null;
  const replan = (message?: string) => {
    setNotice(message);
    setForce(false);
    setSeq((s) => s + 1);
  };

  const send = async () => {
    if (!plan || plan.remoteRef === null) return;
    const remoteRef = plan.remoteRef;
    if (force) {
      if (lease === null) return;
      const ok = await deps.confirm({
        title: P.forceTitle,
        body: P.forceBody(remoteRef, shortHash(lease)),
        confirm: P.forceConfirm,
        danger: true,
      });
      if (!ok) return;
    }
    setSending(true);
    setNotice(undefined);
    setRejected(undefined);
    try {
      const result = await deps.client.call("git.push", {
        ...base,
        ...input,
        setUpstream,
        forceWithLease: force ? lease : null,
        expectedTarget: { remote: plan.remote, remoteRef },
      });
      onClose();
      deps.notify({
        level: result.terminal ? "warning" : "info",
        title: P.done(result.branch, result.remote, result.remoteRef),
        body: [
          P.doneRefspec(result.refspec),
          ...(result.terminal ? [P.terminal, ...terminalLines(P, result.terminal)] : []),
        ].join("\n"),
      });
    } catch (e) {
      setSending(false);
      if (e instanceof RpcCallError && e.code === ERROR_CODE.CONFLICT) return replan(P.changed);
      if (e instanceof RpcCallError && e.code === PUSH_REFUSED) return replan(P.changed);
      if (e instanceof RpcCallError && e.code === GIT_FAILED) return setRejected(errorText(e));
      setNotice(T.common.failed(P.title) + ": " + errorText(e));
    }
  };

  const commits = load.kind === "ready" ? load.commits : undefined;
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content data-testid="git-push-dialog" className={SHEET}>
          <Dialog.Title className="text-sm font-semibold">{P.title}</Dialog.Title>
          <Dialog.Description className="sr-only">{P.title}</Dialog.Description>
          {notice && (
            <p role="alert" data-testid="git-push-notice" className="text-ui-accent">
              {notice}
            </p>
          )}
          {load.kind === "loading" && <p className="text-ui-muted-foreground">{P.loading}</p>}
          {load.kind === "failed" && (
            <p role="alert" className="break-all text-ui-accent">
              {load.message}
            </p>
          )}
          {plan && plan.refusal && (
            <div data-testid="git-push-refusal" className="flex flex-col gap-1">
              <p className="text-ui-accent">{P.refused(plan.refusal.reason)}</p>
              <p className="text-[12px] text-ui-muted-foreground">{plan.refusal.message}</p>
              {plan.refusal.candidates.length > 0 && (
                <>
                  <p>{P.candidates}</p>
                  {plan.refusal.candidates.map((c) => (
                    <button
                      key={c}
                      type="button"
                      data-candidate={c}
                      className={`${BUTTON} text-left font-mono text-[12px]`}
                      onClick={() => {
                        setNotice(undefined);
                        setInput({ ...input, remoteRef: c });
                      }}
                    >
                      {c}
                    </button>
                  ))}
                </>
              )}
            </div>
          )}
          {plan && plan.remoteRef !== null && (
            <div className="flex flex-col gap-1">
              <p data-testid="git-push-target" className="font-mono text-[12px]">
                {P.target(plan.branch, plan.remote, plan.remoteRef)}
              </p>
              {plan.target && (
                <p className="text-[12px] text-ui-muted-foreground">{P.source(plan.target)}</p>
              )}
              {plan.terminal && (
                <div
                  data-testid="git-push-terminal"
                  className="rounded-sm border border-ui-border px-2 py-1 text-[12px]"
                >
                  <p className="text-ui-accent">{P.terminal}</p>
                  {terminalLines(P, plan.terminal).map((line) => (
                    <p key={line} className="font-mono break-all">
                      {line}
                    </p>
                  ))}
                </div>
              )}
              {commits && (
                <div className="flex min-h-0 flex-col gap-1">
                  <p>
                    {commits.commits.length === 0 ? P.noCommits : P.commits(commits.commits.length)}
                  </p>
                  {commits.commits.length > 0 && (
                    <ul
                      data-testid="git-push-commits"
                      className="max-h-48 min-h-0 overflow-auto rounded-sm border border-ui-border px-2 py-1 text-[12px]"
                    >
                      {commits.commits.map((c) => (
                        <li key={c.hash} className="flex gap-2">
                          <code className="text-ui-muted-foreground">{shortHash(c.hash)}</code>
                          <span className="min-w-0 flex-1 truncate">{c.subject}</span>
                          <span className="text-ui-muted-foreground">{c.author}</span>
                        </li>
                      ))}
                      {commits.truncated && (
                        <li className="text-ui-muted-foreground">{P.commitsMore}</li>
                      )}
                    </ul>
                  )}
                </div>
              )}
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={setUpstream}
                  onChange={(e) => setSetUpstream(e.target.checked)}
                />
                {P.setUpstream}
              </label>
              <label
                className="flex items-center gap-2"
                title={lease === null ? P.forceUnavailable : undefined}
              >
                <input
                  type="checkbox"
                  checked={force}
                  disabled={lease === null}
                  onChange={(e) => setForce(e.target.checked)}
                />
                <span className={force ? "text-ui-accent" : ""}>{P.force}</span>
              </label>
            </div>
          )}
          {rejected && (
            <div data-testid="git-push-rejected" className="flex flex-col gap-1">
              <p className="text-ui-accent">{P.rejected}</p>
              <pre className="max-h-40 overflow-auto rounded-sm border border-ui-border px-2 py-1 text-[12px] whitespace-pre-wrap">
                {rejected}
              </pre>
            </div>
          )}
          <div className="flex justify-end gap-2">
            {rejected && deps.pull && (
              <button
                type="button"
                className={BUTTON}
                onClick={() => {
                  onClose();
                  deps.pull?.();
                }}
              >
                {P.pull}
              </button>
            )}
            <button type="button" className={BUTTON} onClick={onClose}>
              {T.common.cancel}
            </button>
            <button
              type="button"
              autoFocus
              disabled={!plan || plan.remoteRef === null || sending}
              className={`${BUTTON} border-ui-accent ${force ? "text-ui-accent" : ""}`}
              onClick={() => void send()}
            >
              {sending ? P.sending : force ? P.forceConfirm : P.send}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
