// 커밋 보내기 · 가져오기 화면(18-m7-plan M7-17, 10-ui.md 2.9 「보내기 · 가져오기」, 08-remote-mobile.md 「커밋 보내기 · 가져오기」).
//
// - 액션 `git.send.open`(보내기 — 활성 체크아웃 → 고른 체크아웃)과 `git.receive.open`(가져오기 — 고른 체크아웃 → 활성 체크아웃). VCS
//   작업 팝업(⌃V — 「Git 메뉴」)에 저절로 나오고, 브랜치 팝업 첫 화면에도 같은 두 줄이 있다(branch-popup.tsx). 묶음에 다른 머신의
//   체크아웃이 없으면 꺼진다.
// - **대상은 그때 고르고 기억한다**(D57 ③): 고르는 팝업은 다른 머신 멤버의 메인 체크아웃과, 그 머신 연결이 이미 붙어 있으면 그
//   머신의 워크트리를 보인다(팝업을 연다고 연결을 열지 않는다 — D56). 원본 멤버의 「마지막 대상」(M7-10 `lastTarget`)이 먼저 골라져
//   있고, 보낼 때 `bundle.setLastSendTarget` 으로 갱신한다. 가져오기는 원본이 다른 머신이라 그 머신 멤버의 마지막 대상이 지금
//   체크아웃인 것을 먼저 고른다.
// - 확인에서 원본 · 대상 머신 이름을 글로 보인다(`data-machine`). 원본에 커밋 안 한 변경이 있으면 「가지 않는다」, 대상 체크아웃에서
//   도는 실행 · 터미널이 있으면 그 수(막지 않는다).
// - 진행은 상태바의 이 항목(보낸 바이트 / 전체, 취소)에 보인다. 대상 Host 의 작업(`receive` · `apply` 의 Task)은 그 Host 의 것이라 대상이
//   이 머신일 때만 창의 작업 항목에 나온다(창의 작업 표는 이 머신 연결만 구독한다 — 메인 판정 2026-11-23).
// - **대상 연결에서도 `git.prompts` 를 듣는다**(M7-16 보고 5): 바뀌는 경로가 있는 `apply` 는 대상 프로젝트 창의 미저장 확인을 거친다 —
//   듣는 창이 없으면 아무것도 하지 않고 실패한다. 대상이 활성 체크아웃이면 창의 `GitPrompts` 가 이미 듣고, 아니면 이 기능이 확인 ·
//   보내기 동안 그 머신 연결로 듣는다(질문의 경로에 그 머신 접두를 붙여 미저장 문서를 찾는다).
// - 끝 알림: 맞췄으면 [거기서 실행](`run.chooseIn`) · [거기서 터미널](`terminal.newTabIn`) — **그 액션이 인자를 받을 때만**
//   (`needsArgs` · `isAvailable`, 메인 판정 2026-11-23: 인자를 모르는 액션을 부르면 활성 체크아웃 — 엉뚱한 머신에서 연다). 이미 있던
//   브랜치를 올렸으면 [되돌리기] — 대상 연결로 `git.reset`(keep, `expected` = 맞춘 끝 · `expectedRef` = 그 브랜치)이라 그사이
//   대상의 HEAD 가 바뀌었으면 Host 가 거절하고 아무것도 하지 않는다(D42 · dialogs/undo.ts). 그 확인 다이얼로그의 머신 문장은 대상
//   머신이다. 만든 브랜치는 되돌리기를 달지 않는다(움직인 ref 가 없다).
// - 멈춤(갈라짐 · 그사이 움직임 · git 의 거절)은 덮지 않는다: 알림에 까닭 · git 원문 · 「받은 커밋은 `refs/pitwall/incoming/…` 에
//   있다」 를 보인다.
// - 대상 머신의 git 훅: 맞추기(merge --ff-only · switch)는 대상에서 git 이 하는 대로 훅을 돌린다(08). 사람의 보내기는 Owner 가 그
//   머신에서 이미 할 수 있는 일이라 새 권한이 아니어서 확인에 따로 알리지 않는다(M7-17 보고 — 에이전트 갈래는 M7-19).

import type { Action, ActionRegistry } from "@pitwall/keymap";
import type { CheckoutRef, GitWorktree, ProjectBundle } from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import { Command } from "cmdk";
import { useMemo, useRef, useState } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

import { type Checkout, LOCAL_MACHINE } from "../../checkout/checkout";
import {
  type CheckoutHosts,
  useCheckoutHost,
  useCheckoutHosts,
  useLocalHost,
} from "../../checkout/checkout-hosts";
import { MachineMark, TargetMachineProvider, useMachineName } from "../../checkout/machine-mark";
import { useEditorWorkspace } from "../../editor/editor-workspace";
import { useKeymap, useKeymapPopup, useRegisterActions } from "../../keymap/keymap-provider";
import { useNotify } from "../../notifications/notifications";
import type { NotificationAction } from "../../notifications/notifications";
import { useActiveMachine, useActiveWorktree } from "../../open-with/active-worktree";
import { HostConnectionProvider, useRpcQueryOn } from "../../rpc/host";
import { STATUS_SLOTS, useStatusBarItem } from "../../status-bar/status-bar";
import { SEARCH_ITEM_CLASS } from "../../ui/search-popup";
import { BUTTON, SHEET, useGitDialogs } from "../dialogs/git-dialogs";
import { useGitDialogStrings } from "../dialogs/strings";
import { errorText, shortHash, undoMove } from "../dialogs/undo";
import { GitPrompts } from "../git-prompts";
import { SendCancelled, type SendResult, sendCommits } from "./send-flow";
import { type GitSendStrings, useGitSendStrings } from "./strings";

const LOG = "[git-send]";

export const SEND_ACTION = "git.send.open";
export const RECEIVE_ACTION = "git.receive.open";
/** 끝 알림의 [거기서 실행] · [거기서 터미널] 이 부르는 액션(M7-14 — 인자 `{ checkout: CheckoutRef }`). */
export const RUN_IN_ACTION = "run.chooseIn";
export const TERMINAL_IN_ACTION = "terminal.newTabIn";

export type SendDirection = "send" | "receive";

/** 보낼 것: 원본 → 대상. */
export interface SendPlan {
  direction: SendDirection;
  source: Checkout;
  target: Checkout;
}

/** 두 체크아웃이 같은가. */
export function sameCheckout(a: Checkout | CheckoutRef | null | undefined, b: Checkout): boolean {
  return !!a && a.machine === b.machine && (a.worktree ?? undefined) === b.worktree;
}

/** 이 창의 묶음 — 창이 아는 멤버(머신 → 프로젝트 id)와 같은 멤버를 가진 묶음. */
export function bundleOf(
  bundles: readonly ProjectBundle[] | undefined,
  members: ReadonlyMap<string, string>,
): ProjectBundle | undefined {
  return bundles?.find(
    (b) =>
      !b.implicit &&
      b.members.length === members.size &&
      b.members.every((m) => members.get(m.machine) === m.projectId),
  );
}

/** 고르는 팝업이 먼저 고를 체크아웃(머리 주석 — D57 ③). */
export function preselected(
  bundle: ProjectBundle | undefined,
  direction: SendDirection,
  active: Checkout,
): Checkout | undefined {
  if (!bundle) return undefined;
  if (direction === "send") {
    const last = bundle.members.find((m) => m.machine === active.machine)?.lastTarget;
    if (!last || last.machine === active.machine) return undefined;
    if (!bundle.members.some((m) => m.machine === last.machine)) return undefined;
    return { machine: last.machine, ...(last.worktree && { worktree: last.worktree }) };
  }
  const from = bundle.members.find(
    (m) => m.machine !== active.machine && sameCheckout(m.lastTarget, active),
  );
  return from && { machine: from.machine };
}

const itemValue = (c: Checkout) => `checkout:${c.machine}:${c.worktree ?? ""}`;

// ── 진행(상태바) ──────────────────────────────────────────────────────────────────────────────────────────────

interface Progress {
  to: string;
  sent: number;
  size: number;
  cancel(): void;
}

/** 창 하나의 보내기 진행. 한 번에 하나다. */
export const sendProgress = createStore<{ now: Progress | undefined }>()(() => ({
  now: undefined,
}));

function SendProgressItem() {
  const T = useGitSendStrings();
  const now = useStore(sendProgress, (s) => s.now);
  if (!now) return null;
  const percent = now.size > 0 ? Math.round((now.sent / now.size) * 100) : 0;
  const label = T.result.progress(now.to);
  return (
    <div data-git-send-progress="" className="flex items-center gap-1">
      <span className="max-w-48 truncate">{label}</span>
      <span
        role="progressbar"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >{`${percent}%`}</span>
      <button
        type="button"
        aria-label={T.result.cancel}
        title={T.result.cancel}
        onClick={now.cancel}
        className="rounded-sm px-1 hover:bg-ui-hover"
      >
        ×
      </button>
    </div>
  );
}

// ── 고르기 ────────────────────────────────────────────────────────────────────────────────────────────────

function MachineGroup({
  machine,
  hosts,
  extra,
  onPick,
}: {
  machine: string;
  hosts: CheckoutHosts;
  /** 목록에 없어도 보일 워크트리(마지막 대상). */
  extra: string | undefined;
  onPick(checkout: Checkout): void;
}) {
  const T = useGitSendStrings();
  const name = useMachineName()(machine);
  // 붙어 있을 때만 그 머신의 워크트리를 읽는다 — 팝업이 연결을 열지 않는다(D56).
  const client = useStore(hosts.store, (s) =>
    machine === LOCAL_MACHINE ? s.local?.client : s.remote[machine]?.client,
  );
  const projectId = useStore(hosts.store, (s) => s.directory.members.get(machine));
  const key = hosts.connection(machine).key;
  const worktrees = useRpcQueryOn(
    { client: projectId === undefined ? undefined : client, key },
    "git.worktrees",
    { projectId: projectId ?? "", worktree: null },
    { staleTime: 0, retry: false },
  );
  const linked = (worktrees.data?.worktrees ?? []).filter(
    (w: GitWorktree) => !w.main && !w.missing,
  );
  const paths = linked.map((w) => w.path);
  if (extra !== undefined && !paths.includes(extra)) paths.push(extra);
  const head = (w: GitWorktree | undefined) =>
    w?.head?.kind === "branch"
      ? w.head.name
      : w?.head?.kind === "detached"
        ? shortHash(w.head.commit)
        : "";
  const main = worktrees.data?.worktrees.find((w) => w.main);
  const item = (checkout: Checkout, label: string, detail: string) => (
    <Command.Item
      key={itemValue(checkout)}
      value={itemValue(checkout)}
      keywords={[name, label, detail]}
      onSelect={() => onPick(checkout)}
      data-checkout={itemValue(checkout)}
      className={SEARCH_ITEM_CLASS}
    >
      <MachineMark machine={machine} />
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <span className="flex-none text-[11px] text-ui-muted-foreground">{detail}</span>
    </Command.Item>
  );
  return (
    <Command.Group heading={name}>
      {item({ machine }, T.pick.main, head(main))}
      {paths.map((path) =>
        item({ machine, worktree: path }, path, head(linked.find((w) => w.path === path))),
      )}
    </Command.Group>
  );
}

function SendPicker({
  direction,
  hosts,
  bundle,
  onPick,
  onClose,
}: {
  direction: SendDirection;
  hosts: CheckoutHosts;
  bundle: ProjectBundle | undefined;
  onPick(checkout: Checkout): void;
  onClose(): void;
}) {
  const T = useGitSendStrings();
  useKeymapPopup(true);
  const activeMachine = useActiveMachine();
  const activeWorktree = useActiveWorktree();
  const members = useStore(hosts.store, (s) => s.directory.members);
  const others = [...members.keys()].filter((m) => m !== activeMachine);
  const active: Checkout = {
    machine: activeMachine,
    ...(activeWorktree && { worktree: activeWorktree }),
  };
  const first = preselected(bundle, direction, active);
  const [value, setValue] = useState(first ? itemValue(first) : "");
  const label = direction === "send" ? T.pick.send : T.pick.receive;
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-git-send-picker={direction}
          aria-label={label}
          aria-describedby={undefined}
          className="fixed top-[15%] left-1/2 w-[520px] max-w-[90vw] -translate-x-1/2 overflow-hidden rounded-md border border-ui-border bg-ui-panel text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="truncate border-b border-ui-border px-3 py-1 text-center text-[12px] text-ui-muted-foreground">
            {label}
          </Dialog.Title>
          <Command label={label} value={value} onValueChange={setValue} loop>
            <Command.Input
              autoFocus
              placeholder={T.pick.placeholder}
              className="w-full border-b border-ui-border bg-transparent px-3 py-2 outline-none placeholder:text-ui-muted-foreground"
            />
            <Command.List className="max-h-[360px] overflow-y-auto py-1">
              <Command.Empty className="px-3 py-2 text-ui-muted-foreground">
                {T.pick.empty}
              </Command.Empty>
              {others.map((machine) => (
                <MachineGroup
                  key={machine}
                  machine={machine}
                  hosts={hosts}
                  extra={first?.machine === machine ? first.worktree : undefined}
                  onPick={onPick}
                />
              ))}
            </Command.List>
          </Command>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

// ── 확인 ──────────────────────────────────────────────────────────────────────────────────────────────────

/** 확인이 읽은 원본의 끝. */
export interface SendHead {
  branch: string;
  commit: string;
}

function CheckoutLine({ label, checkout }: { label: string; checkout: Checkout }) {
  const T = useGitSendStrings();
  const name = useMachineName()(checkout.machine);
  return (
    <div data-send-side={label} data-machine={checkout.machine} className="flex min-w-0 gap-2">
      <span className="w-12 flex-none text-ui-muted-foreground">{label}</span>
      <span className="flex-none font-semibold">{name}</span>
      <span className="min-w-0 flex-1 truncate text-ui-muted-foreground" title={checkout.worktree}>
        {checkout.worktree ?? T.pick.main}
      </span>
    </div>
  );
}

function SendConfirm({
  plan,
  onCancel,
  onGo,
}: {
  plan: SendPlan;
  onCancel(): void;
  onGo(head: SendHead): void;
}) {
  const T = useGitSendStrings();
  const D = useGitDialogStrings();
  useKeymapPopup(true);
  const name = useMachineName();
  const source = useCheckoutHost(plan.source.machine);
  const target = useCheckoutHost(plan.target.machine);
  const at = (c: Checkout, projectId: string | undefined) => ({
    projectId: projectId ?? "",
    worktree: c.worktree ?? null,
  });
  const ready = (conn: typeof source) => ({
    client: conn.projectId === undefined ? undefined : conn.client,
    key: conn.key,
  });
  const fresh = { staleTime: 0, retry: false } as const;
  const refs = useRpcQueryOn(ready(source), "git.refs", at(plan.source, source.projectId), fresh);
  const status = useRpcQueryOn(
    ready(source),
    "git.status",
    at(plan.source, source.projectId),
    fresh,
  );
  const runs = useRpcQueryOn(
    ready(target),
    "run.list",
    { projectId: target.projectId ?? "" },
    fresh,
  );
  const terminals = useRpcQueryOn(
    ready(target),
    "terminal.list",
    { projectId: target.projectId ?? "" },
    fresh,
  );
  const there = (w: string | null) => (w ?? undefined) === plan.target.worktree;
  const busyRuns = (runs.data?.runs ?? []).filter((r) => r.state !== "exited" && there(r.worktree));
  const busyTerminals = (terminals.data?.terminals ?? []).filter(
    (t) => !t.exited && there(t.worktree),
  );
  const head = refs.data?.head;
  const branch = head?.kind === "branch" ? { branch: head.name, commit: head.commit } : undefined;
  const connected = !!source.client && !!target.client;
  const go = plan.direction === "send" ? T.confirm.send : T.confirm.receive;
  const title = T.confirm.title(
    branch?.branch ?? "…",
    name(plan.source.machine),
    name(plan.target.machine),
  );
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onCancel()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content data-testid="git-send-confirm" className={SHEET}>
          <Dialog.Title className="text-sm font-semibold">{title}</Dialog.Title>
          <Dialog.Description className="sr-only">{title}</Dialog.Description>
          <CheckoutLine label={T.confirm.from} checkout={plan.source} />
          <CheckoutLine label={T.confirm.to} checkout={plan.target} />
          {!connected && <p className="text-ui-muted-foreground">{T.confirm.notConnected}</p>}
          {connected && !refs.data && !refs.error && (
            <p className="text-ui-muted-foreground">{T.confirm.loading}</p>
          )}
          {refs.error && <p role="alert">{errorText(refs.error)}</p>}
          {refs.data && !branch && <p role="alert">{T.confirm.notBranch}</p>}
          {(status.data?.entries.length ?? 0) > 0 && (
            <p data-send-note="uncommitted">{T.confirm.uncommitted}</p>
          )}
          {(busyRuns.length > 0 || busyTerminals.length > 0) && (
            <p data-send-note="busy">{T.confirm.busy(busyRuns.length, busyTerminals.length)}</p>
          )}
          <p className="text-ui-muted-foreground">{T.confirm.noForce}</p>
          <div className="flex justify-end gap-2">
            <button type="button" className={BUTTON} onClick={onCancel}>
              {D.common.cancel}
            </button>
            <button
              type="button"
              className={BUTTON}
              data-send-go=""
              disabled={!branch || !connected}
              onClick={() => branch && onGo(branch)}
            >
              {go}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** 대상 머신 연결의 `git.prompts`(머리 주석). 대상이 활성 체크아웃 머신이면 창의 것이 이미 듣는다 — 부르는 쪽이 거른다. */
function TargetPrompts({ machine }: { machine: string }) {
  const workspace = useEditorWorkspace();
  const conn = useCheckoutHost(machine);
  const name = useMachineName()(machine);
  if (conn.projectId === undefined) return null;
  return (
    <HostConnectionProvider connection={conn}>
      <GitPrompts
        projectId={conn.projectId}
        workspace={workspace}
        machine={name}
        machineId={machine}
      />
    </HostConnectionProvider>
  );
}

// ── 끝 ───────────────────────────────────────────────────────────────────────────────────────────────────

/** 끝 알림의 제목 · 본문. */
export function resultText(
  T: GitSendStrings,
  branch: string,
  to: string,
  result: SendResult,
): { level: "info" | "warning"; title: string; body: string | undefined } {
  const lines: string[] = [];
  const { outcome } = result;
  if (outcome.kind === "stopped") {
    lines.push(T.result.reason(outcome.reason));
    if (outcome.message) lines.push(outcome.message);
    lines.push(T.result.incoming(result.incomingRef));
    if (result.uncommitted) lines.push(T.result.uncommitted);
    return { level: "warning", title: T.result.stopped(branch, to), body: lines.join("\n") };
  }
  if (result.uncommitted) lines.push(T.result.uncommitted);
  if (outcome.lfsPointers > 0) lines.push(T.result.lfs(outcome.lfsPointers));
  const title = outcome.created
    ? T.result.created(branch, to)
    : outcome.previous === outcome.current
      ? T.result.same(branch, to)
      : T.result.moved(
          branch,
          to,
          `${shortHash(outcome.previous)} → ${shortHash(outcome.current)}`,
        );
  return { level: "info", title, body: lines.length ? lines.join("\n") : undefined };
}

/**
 * [거기서 실행] · [거기서 터미널] — **그 액션이 인자를 받을 때만**(`needsArgs` · `isAvailable`). 인자를 모르는 액션은 활성 체크아웃에서
 * 연다 — 엉뚱한 머신에서 실행된다(메인 판정 2026-11-23).
 */
export function thereButtons(
  registry: Pick<ActionRegistry, "needsArgs" | "isAvailable" | "run">,
  checkout: CheckoutRef,
  T: GitSendStrings,
): NotificationAction[] {
  const out: NotificationAction[] = [];
  for (const [id, label] of [
    [RUN_IN_ACTION, T.result.runThere],
    [TERMINAL_IN_ACTION, T.result.terminalThere],
  ] as const)
    if (registry.needsArgs(id) && registry.isAvailable(id))
      out.push({ label, run: () => void registry.run(id, { checkout }) });
  return out;
}

export function sendActions({
  strings,
  enabled,
  open,
}: {
  strings: GitSendStrings["actions"];
  enabled(): boolean;
  open(direction: SendDirection): void;
}): Action[] {
  return [
    {
      id: SEND_ACTION,
      label: strings.send,
      keywords: ["Send Commits", "Send to Checkout"],
      enabled,
      run: () => open("send"),
    },
    {
      id: RECEIVE_ACTION,
      label: strings.receive,
      keywords: ["Receive Commits", "Fetch from Checkout"],
      enabled,
      run: () => open("receive"),
    },
  ];
}

type Stage =
  | { kind: "pick"; direction: SendDirection }
  | { kind: "confirm"; plan: SendPlan }
  | { kind: "running"; plan: SendPlan };

/** 프로젝트 창의 보내기 · 가져오기. 브랜치 기능(git-branch-feature.tsx)이 둔다. */
export function GitSendFeature() {
  const T = useGitSendStrings();
  const D = useGitDialogStrings();
  const hosts = useCheckoutHosts();
  const local = useLocalHost();
  const { registry } = useKeymap();
  const notify = useNotify();
  const dialogs = useGitDialogs();
  const name = useMachineName();
  const activeMachine = useActiveMachine();
  const activeWorktree = useActiveWorktree();
  const [stage, setStage] = useState<Stage>();
  /**
   * 되돌리는 중인 대상 머신(겹치면 여럿). 되돌리기(`git.reset` keep)는 작업 트리 쓰기라 대상 Host 가 그 프로젝트의 `git.prompts` 를
   * 듣는 창에 미저장 확인을 묻는다 — 듣는 창이 없으면 `2006` 으로 하지 않는다(M7-17 리뷰 B1). 그래서 되돌리기가 끝날 때까지 그 머신의
   * 듣기(`TargetPrompts`)를 둔다. 보내기의 듣기와 같은 수명 규칙이다(확인 · 보내기 동안).
   */
  const [undoing, setUndoing] = useState<readonly string[]>([]);
  useStatusBarItem({ id: "gitSend", ...STATUS_SLOTS.gitSend, component: SendProgressItem });

  const members = useStore(hosts?.store ?? NO_HOSTS, (s) => s.directory.members);
  const bundles = useRpcQueryOn(local, "bundle.list", null, { retry: false });
  const bundle = bundleOf(bundles.data, members);
  const others = [...members.keys()].filter((m) => m !== activeMachine);

  const live = useRef({ T, D, notify, dialogs, name, bundle, others, registry, hosts, local });
  live.current = { T, D, notify, dialogs, name, bundle, others, registry, hosts, local };

  const actions = useMemo(
    () =>
      sendActions({
        strings: T.actions,
        enabled: () => !!live.current.hosts && live.current.others.length > 0,
        open: (direction) => setStage({ kind: "pick", direction }),
      }),
    [T],
  );
  useRegisterActions(actions);

  const active: Checkout = {
    machine: activeMachine,
    ...(activeWorktree && { worktree: activeWorktree }),
  };

  const pick = (direction: SendDirection, chosen: Checkout) =>
    setStage({
      kind: "confirm",
      plan:
        direction === "send"
          ? { direction, source: active, target: chosen }
          : { direction, source: chosen, target: active },
    });

  const run = async (plan: SendPlan, head: SendHead) => {
    const { hosts, local } = live.current;
    if (!hosts) return;
    setStage({ kind: "running", plan });
    const releases = [hosts.retain(plan.source.machine), hosts.retain(plan.target.machine)];
    const abort = new AbortController();
    const to = live.current.name(plan.target.machine);
    try {
      const [source, target] = await Promise.all([
        hosts.client(plan.source.machine),
        hosts.client(plan.target.machine),
      ]);
      const bundleId = live.current.bundle?.id;
      if (bundleId && local.client)
        await local.client
          .call("bundle.setLastSendTarget", {
            bundleId,
            source: plan.source.machine,
            target: { machine: plan.target.machine, worktree: plan.target.worktree ?? null },
          })
          .catch((e: unknown) => console.warn(LOG, "could not remember the send target", e));
      sendProgress.setState({ now: { to, sent: 0, size: 0, cancel: () => abort.abort() } });
      const result = await sendCommits({
        source: { ...source, machine: plan.source.machine, worktree: plan.source.worktree ?? null },
        target: { ...target, machine: plan.target.machine, worktree: plan.target.worktree ?? null },
        branch: head.branch,
        commit: head.commit,
        signal: abort.signal,
        onProgress: (sent, size) =>
          sendProgress.setState((s) => (s.now ? { now: { ...s.now, sent, size } } : s)),
      });
      finish(plan, head, result);
    } catch (e) {
      if (e instanceof SendCancelled || abort.signal.aborted) return;
      live.current.notify({
        level: "error",
        title: live.current.T.result.failed,
        body: errorText(e),
      });
    } finally {
      sendProgress.setState({ now: undefined });
      for (const release of releases) release();
      setStage((s) => (s?.kind === "running" && s.plan === plan ? undefined : s));
    }
  };

  const finish = (plan: SendPlan, head: SendHead, result: SendResult) => {
    const { T, D, notify, dialogs, name, registry, hosts } = live.current;
    const to = name(plan.target.machine);
    const text = resultText(T, head.branch, to, result);
    const buttons: NotificationAction[] = [];
    const { outcome } = result;
    if (outcome.kind === "applied") {
      const checkout: CheckoutRef = {
        machine: plan.target.machine,
        worktree: plan.target.worktree ?? null,
      };
      buttons.push(...thereButtons(registry, checkout, T));
      if (outcome.previous !== null && outcome.previous !== outcome.current && hosts) {
        const move = {
          refName: `refs/heads/${head.branch}`,
          previous: outcome.previous,
          current: outcome.current,
        };
        buttons.push({
          label: D.undo.button,
          run: () => {
            const machine = plan.target.machine;
            const release = hosts.retain(machine);
            setUndoing((now) => [...now, machine]);
            const done = () => {
              release();
              setUndoing((now) => {
                const at = now.indexOf(machine);
                return at < 0 ? now : [...now.slice(0, at), ...now.slice(at + 1)];
              });
            };
            void hosts
              .client(machine)
              .then(({ client, projectId }) =>
                undoMove(
                  {
                    client,
                    projectId,
                    ...(plan.target.worktree !== undefined && { worktree: plan.target.worktree }),
                    confirm: dialogs.confirm,
                    notify,
                    strings: D,
                  },
                  move,
                  { kind: "head", mode: "keep" },
                ),
              )
              .catch((e: unknown) =>
                notify({ level: "error", title: D.undo.failed, body: errorText(e) }),
              )
              .finally(done);
          },
        });
      }
    }
    notify({ ...text, ...(buttons.length > 0 && { actions: buttons }) });
  };

  // 대상 머신의 `git.prompts` 듣기 — 확인 · 보내기 동안의 대상과 되돌리는 중인 대상. 활성 체크아웃 머신은 창의 것이 이미 듣는다.
  const listening = [
    ...new Set([
      ...(stage && stage.kind !== "pick" ? [stage.plan.target.machine] : []),
      ...undoing,
    ]),
  ].filter((m) => m !== activeMachine);
  const undoMachine = undoing.at(-1);

  return (
    <>
      {stage?.kind === "pick" && hosts && (
        <SendPicker
          direction={stage.direction}
          hosts={hosts}
          bundle={bundle}
          onPick={(c) => pick(stage.direction, c)}
          onClose={() => setStage(undefined)}
        />
      )}
      {stage?.kind === "confirm" && (
        <SendConfirm
          plan={stage.plan}
          onCancel={() => setStage(undefined)}
          onGo={(head) => void run(stage.plan, head)}
        />
      )}
      {listening.map((machine) => (
        <TargetPrompts key={machine} machine={machine} />
      ))}
      <TargetMachineProvider machine={undoMachine ?? LOCAL_MACHINE}>
        {dialogs.element}
      </TargetMachineProvider>
    </>
  );
}

const NO_HOSTS = createStore<{ directory: { members: ReadonlyMap<string, string> } }>()(() => ({
  directory: { members: new Map() },
}));

/** 시험용. */
export function resetSendProgress() {
  sendProgress.setState({ now: undefined });
}
