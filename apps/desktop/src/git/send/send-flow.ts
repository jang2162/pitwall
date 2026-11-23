// 커밋 보내기 · 가져오기의 흐름(18-m7-plan M7-17, 08-remote-mobile.md 「커밋 보내기 · 가져오기」). Desktop 이 두 머신의 연결 사이를
// 잇는다 — 엔진은 M7-16 의 `git.send.*` 이고 여기는 부르기만 한다. 순서(M7-16 보고 「Desktop 의 순서」):
//
// 1. 대상 `git.send.theirs` — 대상이 가진 커밋과 그 브랜치의 지금 끝(`branchTip`).
// 2. 원본 `git.send.prepare` 스트림 — 첫 항목 `info`(크기 · 커밋 안 한 변경 · 뺀 커밋), 이어서 `data` 청크.
// 3. 대상 `receive.begin`(크기) → 청크마다 `receive.chunk`(응답을 받은 뒤 다음 청크) → `receive.finish`.
// 4. 대상 `git.send.apply` — `expected` 는 1 의 `branchTip`. 그사이 대상 브랜치가 움직였으면 엔진이 덮지 않고 멈춘다.
//
// **Desktop 은 바이트를 디스크에 두지 않는다**: 청크는 받은 대로 대상에 넘기고 버린다. 스트림의 credit 은 소비한 만큼만 다시 주므로
// (rpc-client `StreamOptions.window`), 대상의 `chunk` 응답을 기다리는 동안 원본도 더 보내지 않는다 — 두 연결이 같은 속도로 간다.
// 청크 내용은 로그 · 오류 문장에 넣지 않는다.

import type { GitSendOutcome, RpcClient } from "@pitwall/rpc-client";

/** 한쪽 체크아웃. `worktree` 는 그 머신의 경로(접두 없음), 메인 체크아웃이면 `null`. */
export interface SendSide {
  client: RpcClient;
  projectId: string;
  machine: string;
  worktree: string | null;
}

export interface SendInput {
  source: SendSide;
  target: SendSide;
  /** 보낼 브랜치(짧은 이름) — 원본 체크아웃의 지금 브랜치. */
  branch: string;
  /** 보낼 커밋(전체 id) — 고른 때 원본 브랜치의 끝. 그 뒤에 더한 커밋은 가지 않는다. */
  commit: string;
  /** 보낸 바이트 · 전체 바이트. */
  onProgress?(sent: number, size: number): void;
  signal?: AbortSignal;
}

export interface SendResult {
  /** 원본에 커밋 안 한 변경이 있었다 — 가지 않았다. */
  uncommitted: boolean;
  /** 받은 커밋을 둔 대상의 전용 ref. */
  incomingRef: string;
  /** 대상의 작업 트리. */
  workTree: string;
  outcome: GitSendOutcome;
}

/** 취소됐다(사람이 상태바에서 끔). */
export class SendCancelled extends Error {
  constructor() {
    super("the send was cancelled");
  }
}

/**
 * 머신 id → `sourceMachine` 의 꼴(`[A-Za-z0-9._-]`, 128자 이하, `.` 으로 시작하지 않고 `.lock` 으로 끝나지 않음 —
 * crates/pw-git/src/send 의 규칙). 머신 id 는 `SHA256:<base64>` 라(pw-link `DeviceId`) 그대로는 못 쓴다: 접두는 `sha256-`, base64 의
 * `+` `/` 는 URL 꼴 `-` `_` 로, 끝의 `=` 는 뺀다. 그 밖의 글자는 `_`. 이 Desktop 은 `local` 이다(메인 판정 2026-11-23 — 이 기기 id 를
 * 웹뷰가 모른다).
 */
export function sourceMachineName(machine: string): string {
  let name = machine
    .replace(/^SHA256:/, "sha256-")
    .replace(/=+$/, "")
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/[^A-Za-z0-9._-]/g, "_");
  if (name === "" || name.startsWith(".")) name = `m${name}`;
  if (name.length > 128) name = name.slice(0, 128);
  if (name.endsWith(".lock")) name = `${name.slice(0, -5)}_lock`;
  return name;
}

function check(signal: AbortSignal | undefined) {
  if (signal?.aborted) throw new SendCancelled();
}

/** 보낸다. git 의 멈춤은 오류가 아니라 `outcome.kind === "stopped"` 다. RPC 실패 · 취소는 던진다. */
export async function sendCommits(input: SendInput): Promise<SendResult> {
  const { source, target, branch, commit, signal } = input;
  const at = (side: SendSide) => ({ projectId: side.projectId, worktree: side.worktree });
  const sourceMachine = sourceMachineName(source.machine);

  const { theirs, branchTip } = await target.client.call("git.send.theirs", {
    ...at(target),
    branch,
  });
  check(signal);

  let uncommitted = false;
  let size: number | undefined;
  let uploadId: number | undefined;
  let sent = 0;
  for await (const item of source.client.stream(
    "git.send.prepare",
    { ...at(source), branch, commit, theirs },
    { signal },
  )) {
    if (item.kind === "info") {
      uncommitted = item.uncommitted;
      size = item.size;
      ({ uploadId } = await target.client.call("git.send.receive.begin", {
        ...at(target),
        sourceMachine,
        branch,
        commit,
        size,
      }));
      input.onProgress?.(0, size);
      continue;
    }
    if (uploadId === undefined || size === undefined)
      throw new Error("git.send.prepare sent data before its info");
    const { received } = await target.client.call("git.send.receive.chunk", {
      uploadId,
      offset: item.offset,
      data: item.data,
    });
    sent = received;
    input.onProgress?.(sent, size);
  }
  // 스트림이 취소로 끝나면 오류 없이 반복이 끝난다 — 받기는 대상이 연결 · 유휴로 버린다.
  check(signal);
  if (uploadId === undefined) throw new Error("git.send.prepare ended without a bundle");

  const received = await target.client.call("git.send.receive.finish", { uploadId });
  // 대상이 받은 것이 보낸 커밋이 아니면 맞추지 않는다(엔진도 전용 ref 를 대조하지만 Desktop 이 고른 커밋으로 묶는다).
  if (received.commit !== commit)
    throw new Error(`the target received ${received.commit}, not the commit that was sent`);
  check(signal);
  const applied = await target.client.call("git.send.apply", {
    ...at(target),
    sourceMachine,
    branch,
    commit,
    expected: branchTip,
  });
  return {
    uncommitted,
    incomingRef: applied.incomingRef,
    workTree: applied.workTree,
    outcome: applied.outcome,
  };
}
