// 상태바의 브랜치 항목(10-ui.md §2.4 「상태바」 행, 13-m2-plan.md M2-13): 왼쪽 맨 앞에 `⎇ 이름 ↑n↓m`, 진행 중 작업이면 `MERGING` 등을
// 덧붙인다. 누르면 브랜치 팝업(`git.branches`). 툴팁은 잡은 저장소(작업 트리 꼭대기 — 결정 M2-D3 의 「어느 저장소인가」)와 upstream.
//
// - ↑ · ↓ 는 지금 브랜치의 upstream 과의 차이(`git.refs` 의 `track`, `git rev-list --left-right --count` 와 같다)이고 0 인 쪽은 뺀다.
//   upstream 이 없거나 사라졌으면(`[gone]`) 둘 다 없다.
// - 분리된 HEAD 는 짧은 해시, 태어나지 않은 브랜치는 그 이름이다.
// - 저장소가 없으면 그리지 않는다. gix 로 읽지 못하는 형식(reftable)이면 「지원하지 않는 저장소」 — 트리 · 탭은 칠하지 않고 이 항목만
//   문장을 단다(10-ui.md 「마우스 · 트랙패드」 의 「Git」 절).

import type { GitBranch, GitRefs, OperationInfo } from "@pitwall/rpc-client";
import { GitBranch as BranchIcon } from "lucide-react";

import { useKeymap } from "../keymap/keymap-provider";
import { useBranch } from "./branch-state";
import { type BranchStrings, useBranchStrings } from "./branch-strings";

export const SHORT_HASH = 7;

/** 지금 HEAD 의 로컬 브랜치(분리 · 태어나지 않음이면 없다). */
export function headBranch(refs: GitRefs): GitBranch | undefined {
  return refs.branches.find((b) => b.head);
}

/** HEAD 의 이름: 브랜치 · 태어나지 않은 브랜치는 그 이름, 분리된 HEAD 는 짧은 해시. */
export function headName(refs: GitRefs): string | undefined {
  const head = refs.head;
  if (!head) return undefined;
  if (head.kind === "detached") return head.commit.slice(0, SHORT_HASH);
  return head.name;
}

/** `↑2↓1` — 0 인 쪽은 뺀다. upstream 이 없으면 빈 문자열. */
export function trackText(refs: GitRefs): string {
  const track = headBranch(refs)?.upstream?.track;
  if (!track) return "";
  return `${track.ahead > 0 ? `↑${track.ahead}` : ""}${track.behind > 0 ? `↓${track.behind}` : ""}`;
}

/** 진행 중 작업 표지(`MERGING`, `REBASING 2/5`). */
export function operationText(T: BranchStrings, operation: OperationInfo | null): string {
  if (!operation) return "";
  const step = operation.step ? ` ${operation.step.current}/${operation.step.total}` : "";
  return `${T.status.operation(operation.kind)}${step}`;
}

export function BranchStatus() {
  const T = useBranchStrings();
  const { registry } = useKeymap();
  const { refs, operation } = useBranch();
  if (!refs || refs.workTree === null) return null;
  const open = () => registry.run("git.branches");
  if (refs.unsupported) {
    return (
      <button
        type="button"
        data-testid="status-branch"
        data-unsupported={refs.unsupported}
        title={`${T.status.unsupportedDetail}\n${T.status.repository(refs.workTree)}`}
        className="flex items-center gap-1 rounded-sm px-1 text-ui-muted-foreground hover:bg-ui-hover"
      >
        <BranchIcon size={11} aria-hidden />
        {T.status.unsupported}
      </button>
    );
  }
  const name = headName(refs) ?? "";
  const track = trackText(refs);
  const busy = operationText(T, operation);
  const upstream = headBranch(refs)?.upstream?.name;
  const title = [
    refs.head?.kind === "detached" ? `${T.status.detached} ${refs.head.commit}` : name,
    upstream && `→ ${upstream}`,
    T.status.repository(refs.workTree),
  ]
    .filter(Boolean)
    .join("\n");
  return (
    <button
      type="button"
      data-testid="status-branch"
      title={title}
      aria-label={T.status.label([name, track, busy].filter(Boolean).join(" "))}
      onClick={open}
      className="flex items-center gap-1 rounded-sm px-1 hover:bg-ui-hover"
    >
      <BranchIcon size={11} aria-hidden />
      <span data-testid="status-branch-name">{name}</span>
      {track && <span data-testid="status-branch-track">{track}</span>}
      {busy && (
        <span data-testid="status-branch-operation" className="font-semibold">
          {busy}
        </span>
      )}
    </button>
  );
}
