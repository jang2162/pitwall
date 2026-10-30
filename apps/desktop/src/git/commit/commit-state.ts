// 커밋 툴 윈도우의 창 상태(docs/design/13-m2-plan.md M2-16): 메시지 칸 · amend · sign-off · 훅 출력 · 고른 파일 · 접은 그룹 · stash
// 목록. 창마다 웹뷰가 따로라 모듈 상태(zustand)로 둔다 — 툴 윈도우를 닫았다 열어도(패널이 다시 그려져도) 쓰던 메시지가 남는다.
// 프로젝트 창의 `GitCommitFeature`(commit-feature.tsx)가 쓰기(`CommitOps`) · 프로젝트 상태(메시지 기록)를 `commitFeature` 에 걸고,
// 툴 윈도우 · 액션이 그것을 부른다.
//
// - 커밋(`runCommit`): 메시지에서 손대지 않은 템플릿 주석 줄을 걷고(commit-model.ts `withoutTemplateComments`) `git.commit` 을
//   부른다. 되면 메시지를 기록(최근 20개 — project-state.ts)에 더하고 칸을 비운 뒤 템플릿을 다시 채운다. 훅 출력이 있으면 툴 윈도우에
//   남긴다. git 이 거절하면(실패하는 훅) 그 출력을 남기고 메시지 칸은 그대로다.
// - 커밋 후 푸시: 커밋이 되면 푸시 다이얼로그(M2-20 의 액션 `git.push.open`)를 부른다 — 그 액션이 이 창에 없으면(`isAvailable`)
//   경고만.
// - amend 를 켜면 메시지 칸이 비었거나 템플릿 그대로일 때 HEAD 의 메시지(`git.show`)를 채우고, 끄면 그 메시지가 그대로일 때 켜기 전의
//   칸으로 돌린다.
// - amend 는 ref 를 옮긴 쓰기다: 결과의 `moved` 로 M2-20 의 액션 `git.undo.offer { move, mode: "soft" }` 를 부른다 — 그 알림(이전 →
//   지금 해시)에 [되돌리기](soft reset — index 의 고친 내용은 남는다)를 단다. 그 액션이 이 창에 없으면 단추 없이 해시만 알린다.

import type { ActionRegistry } from "@pitwall/keymap";
import type { StashEntry } from "@pitwall/rpc-client";
import { create } from "zustand";

import type { NotificationInput } from "../../notifications/notifications";
import type { CommitOps } from "./commit-ops";
import { templateUnedited, withoutTemplateComments } from "./commit-model";
import type { GitCommitStrings } from "./strings";

/** 푸시 다이얼로그(M2-20 의 주인 — 행 사이 이음매). */
export const PUSH_OPEN = "git.push.open";
/** ref 를 옮긴 쓰기의 알림과 [되돌리기](M2-20 의 주인 — 행 사이 이음매). */
export const UNDO_OFFER = "git.undo.offer";

export interface CommitOutput {
  readonly failed: boolean;
  readonly text: string;
}

export interface CommitPanelState {
  message: string;
  amend: boolean;
  signoff: boolean;
  /** 미리 채운 `commit.template`(손대지 않은 주석 줄을 걷을 때 쓴다)과 주석 글자(`core.commentChar`). */
  template: string | undefined;
  commentChar: string;
  /** amend 를 켜기 전의 칸과 채운 HEAD 메시지(끌 때 돌린다). */
  amendFill: { before: string; filled: string } | undefined;
  output: CommitOutput | undefined;
  busy: boolean;
  /** 고른 파일(`CommitItem.key`)과 ⇧ 의 기준. */
  selected: readonly string[];
  anchor: string | undefined;
  collapsed: ReadonlySet<string>;
  stashes: readonly StashEntry[];
  includeUntracked: boolean;
}

const INITIAL: CommitPanelState = {
  message: "",
  amend: false,
  signoff: false,
  template: undefined,
  commentChar: "#",
  amendFill: undefined,
  output: undefined,
  busy: false,
  selected: [],
  anchor: undefined,
  collapsed: new Set(),
  stashes: [],
  includeUntracked: false,
};

export const commitPanel = create<CommitPanelState>(() => ({ ...INITIAL }));

/** 시험용: 창 상태를 처음으로. */
export function resetCommitPanel() {
  commitPanel.setState({ ...INITIAL, collapsed: new Set() });
}

/** 프로젝트 창의 기능이 거는 것. */
export interface CommitFeature {
  readonly ops: CommitOps;
  readonly registry: ActionRegistry;
  strings(): GitCommitStrings;
  notify(input: NotificationInput): void;
  /** 메시지 기록(최근 것부터)과 더하기 — 프로젝트 상태(`state.json`). */
  history(): readonly string[];
  remember(message: string): void;
  /** 스테이징된 항목이 있다. */
  hasStaged(): boolean;
}

export const commitFeature = create<{ current: CommitFeature | undefined }>(() => ({
  current: undefined,
}));

const short = (hash: string | null | undefined) => (hash ? hash.slice(0, 7) : "");

/** 메시지 칸을 고친다(사람이 쓴 것). */
export function setMessage(message: string) {
  commitPanel.setState({ message });
}

/** 비어 있는 메시지 칸에 템플릿을 채운다. */
export async function fillTemplate(feature: CommitFeature) {
  const { template, commentChar } = await feature.ops.template();
  const s = commitPanel.getState();
  if (s.message === "" && !s.amend && template)
    commitPanel.setState({ message: template, template, commentChar });
  else commitPanel.setState({ template, commentChar });
}

/** amend 켜기 · 끄기(머리 주석). */
export async function setAmend(feature: CommitFeature, amend: boolean) {
  const s = commitPanel.getState();
  if (!amend) {
    const fill = s.amendFill;
    commitPanel.setState({
      amend: false,
      amendFill: undefined,
      ...(fill && s.message === fill.filled && { message: fill.before }),
    });
    return;
  }
  commitPanel.setState({ amend: true });
  if (s.message.trim() !== "" && s.message !== s.template) return;
  const head = await feature.ops.headMessage();
  const now = commitPanel.getState();
  if (head !== undefined && now.amend && now.message === s.message)
    commitPanel.setState({ message: head, amendFill: { before: s.message, filled: head } });
}

/** 커밋한다(머리 주석). 했으면 참. */
export async function runCommit(feature: CommitFeature, push: boolean): Promise<boolean> {
  const s = commitPanel.getState();
  if (s.busy) return false;
  const T = feature.strings();
  const message = withoutTemplateComments(s.message, s.template, s.commentChar);
  if (!s.amend && templateUnedited(s.message, s.template, s.commentChar)) {
    feature.notify({ level: "warning", title: T.message.templateUnedited });
    return false;
  }
  if (message.trim() === "") {
    feature.notify({ level: "warning", title: T.message.empty });
    return false;
  }
  if (!s.amend && !feature.hasStaged()) {
    feature.notify({ level: "warning", title: T.message.nothingStaged });
    return false;
  }
  commitPanel.setState({ busy: true, output: undefined });
  const outcome = await feature.ops.commit(message, s.amend, s.signoff);
  if (outcome.kind !== "done") {
    commitPanel.setState({
      busy: false,
      output: outcome.kind === "rejected" ? { failed: true, text: outcome.output } : undefined,
    });
    if (outcome.kind === "failed")
      feature.notify({ level: "error", title: T.result.failed(outcome.message) });
    return false;
  }
  const { result } = outcome;
  feature.remember(message);
  const output = result.output.trim();
  // 칸은 커밋하는 동안 잠겨 있다. 그래도 그사이 바뀌었으면(기록에서 고름 등) 그 글을 지우지 않는다.
  const unchanged = commitPanel.getState().message === s.message;
  commitPanel.setState({
    busy: false,
    ...(unchanged && { message: "" }),
    amend: false,
    amendFill: undefined,
    selected: [],
    anchor: undefined,
    output: output !== "" ? { failed: false, text: result.output } : undefined,
  });
  if (!s.amend) feature.notify({ level: "info", title: T.result.done(short(result.commit)) });
  else if (feature.registry.isAvailable(UNDO_OFFER))
    feature.registry.run(UNDO_OFFER, { move: result.moved, mode: "soft" });
  else
    feature.notify({
      level: "info",
      title: T.result.amended(short(result.moved.previous), short(result.commit)),
    });
  void fillTemplate(feature);
  if (push) {
    if (feature.registry.isAvailable(PUSH_OPEN)) feature.registry.run(PUSH_OPEN);
    else feature.notify({ level: "warning", title: T.result.pushUnavailable });
  }
  return true;
}

/** stash 목록을 다시 읽는다. */
export async function reloadStashes(feature: CommitFeature) {
  const list = await feature.ops.stashList();
  commitPanel.setState({ stashes: list?.stashes ?? [] });
}

/** 메시지 칸에 포커스를 둔다. 툴 윈도우가 막 열렸으면 그려질 때까지 몇 프레임 기다린다. */
export function focusCommitMessage(tries = 20) {
  const box = document.querySelector<HTMLTextAreaElement>("[data-commit-message]");
  if (box) {
    box.focus();
    return;
  }
  if (tries > 0) requestAnimationFrame(() => focusCommitMessage(tries - 1));
}
