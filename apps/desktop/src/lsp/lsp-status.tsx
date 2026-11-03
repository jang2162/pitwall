// 상태바 `lsp` 항목 · 상태 팝업 · 도구 업데이트 알림 · 「도구 관리 열기」 액션(15-m4-plan M4-17). 항목은 활성 탭 문서를 맡는 서버의
// 이름과 상태(시작 중 · 준비 · 진행 % · 오류 · 다시 시작 · 설치 필요 · 런타임 없음 · 미신뢰 · 꺼짐 · 멈춤)를 글자와 표지로 보인다 —
// 색만으로 가르지 않는다(10-ui §6). 진행 %는 서버 관리자가 같은 진행을 `Task` 로도 내므로 상태바 작업 항목과 같은 것이다. 누르면
// 상태 팝업: 인스턴스마다 루트 · 판 · 찾은 곳과 [다시 시작] [멈추기] [로그 열기] [이 프로젝트에서 끄기] [설치…], 꺼진 서버면
// [이 프로젝트에서 켜기], 설치 안내를 끈 서버면 [설치 안내 다시 보기](둘 다 개인 설정을 되돌린다). 팝업을 열면 처음 포커스는 [닫기]
// 다 — 키보드로 연 사람의 다음 Enter 가 서버를 다시 띄우거나 설치를 시작하지 않게.
//
// 표 · 동작은 창의 상태 표(status-window.ts)에 있다. 확인 다이얼로그 · 로그 대화상자도 항목 컴포넌트가 그린다(항목은 창이 사는 동안
// 마운트돼 있다 — 보일 것이 없으면 단추만 그리지 않는다).
//
// 업데이트 알림: 하루 한 번 `tools.updates` 를 보고 업데이트가 있으면 알림(기록에 남는다)을 낸다. 마지막으로 본 때는 이 앱의 웹뷰
// 저장소에 두고, 부르기 **전에** 적어 자리를 잡는다 — 세션 복원으로 창 여럿이 함께 떠도 먼저 적은 창만 묻는다(실패하면 되돌려 다음에
// 다시 묻는다). 저장소를 못 쓰면 모듈 변수에 둬 그 창 안에서 하루 한 번이다(창마다 한 번은 더 알릴 수 있다).

import type { Action } from "@pitwall/keymap";
import {
  ERROR_CODE,
  type LspInstanceState,
  type LspInstanceStatus,
  type LspServerInfo,
  RpcCallError,
} from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import type { DockviewApi } from "dockview-react";
import {
  Ban,
  CircleCheck,
  CircleX,
  Download,
  LoaderCircle,
  type LucideIcon,
  Pause,
  RefreshCw,
  RotateCcw,
  ShieldAlert,
  TriangleAlert,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";

import { useEditorWorkspace, type EditorWorkspace } from "../editor/editor-workspace";
import { editorContext } from "../editor/extensions/context";
import { useI18n } from "../i18n/i18n-provider";
import { useKeymap, useKeymapPopup, useRegisterActions } from "../keymap/keymap-provider";
import { useNotify } from "../notifications/notifications";
import { useHost } from "../rpc/host";
import { STATUS_SLOTS, useStatusBarItem } from "../status-bar/status-bar";
import { ToolConfirmDialog } from "../tools/confirm-dialog";
import { runtimeHint } from "../tools/install";
import { type ToolsStrings, useToolsStrings } from "../tools/strings";
import { TRUST_ACTION } from "../trust/trust";
import {
  createStatusWindow,
  docKey,
  type LspStatusWindow,
  statusWindowOf,
  type StatusDoc,
} from "./status-window";

/** 「도구 관리 열기」 액션 id. */
export const TOOLS_MANAGE_ACTION = "tools.manage";
/** 설정 창 액션(src/keymap/actions.ts). 설정 창의 「도구」 쪽을 연다. */
const SETTINGS_ACTION = "settings.open";
/** 업데이트를 마지막으로 본 때(epoch ms)의 웹뷰 저장소 키. */
export const UPDATES_CHECKED_KEY = "pitwall.tools.updatesCheckedAt";
/** 업데이트를 보는 간격. */
export const UPDATES_INTERVAL_MS = 24 * 60 * 60 * 1000;

const BUTTON =
  "rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";

export interface StateView {
  /** 표지의 종류(시험 · `data-state`). */
  kind: string;
  label: string;
  Icon: LucideIcon;
  /** 경고로 보일 상태(오류 · 런타임 없음 · 설치 필요 · 미신뢰). */
  warning: boolean;
}

/** 인스턴스 상태의 글자 · 표지. 상태마다 글자와 표지가 다르다. */
export function describeState(instance: LspInstanceStatus, T: ToolsStrings["status"]): StateView {
  const state: LspInstanceState = instance.state;
  switch (state.kind) {
    case "starting":
      return { kind: "starting", label: T.starting, Icon: LoaderCircle, warning: false };
    case "ready":
      return instance.progress
        ? {
            kind: "progress",
            label: T.progress(instance.progress.title, instance.progress.percentage),
            Icon: RefreshCw,
            warning: false,
          }
        : { kind: "ready", label: T.ready, Icon: CircleCheck, warning: false };
    case "restartNeeded":
      return { kind: "restartNeeded", label: T.restartNeeded, Icon: RotateCcw, warning: false };
    case "restartSuggested":
      return {
        kind: "restartSuggested",
        label: T.restartSuggested,
        Icon: RotateCcw,
        warning: false,
      };
    case "crashed":
      return { kind: "crashed", label: T.crashed, Icon: CircleX, warning: true };
    case "failed":
      return { kind: "failed", label: T.error, Icon: CircleX, warning: true };
    case "stopped":
      return { kind: "stopped", label: T.stopped, Icon: Pause, warning: false };
    case "untrusted":
      return { kind: "untrusted", label: T.untrusted, Icon: ShieldAlert, warning: true };
    case "notInstalled":
      return { kind: "notInstalled", label: T.notInstalled, Icon: Download, warning: true };
    case "runtimeMissing":
      return {
        kind: "runtimeMissing",
        label: T.runtimeMissing,
        Icon: TriangleAlert,
        warning: true,
      };
    case "disabled":
      return { kind: "disabled", label: T.disabled, Icon: Ban, warning: false };
  }
}

/** 띄울 수 없는 까닭의 문장(런타임 없음 · 미신뢰 · 실패 · 죽음 반복). 없으면 `undefined`. */
export function stateDetail(state: LspInstanceState, T: ToolsStrings): string | undefined {
  switch (state.kind) {
    case "runtimeMissing": {
      const hint = runtimeHint(state.tool, state.needed);
      return T.runtimeMissing(hint.name, state.needed, hint.install);
    }
    case "untrusted":
      return T.untrusted;
    case "failed":
      return T.popup.failedDetail(state.message);
    case "crashed":
      return T.popup.crashedDetail;
    default:
      return undefined;
  }
}

function foundText(info: LspServerInfo | undefined, T: ToolsStrings["popup"]) {
  const r = info?.resolution;
  if (r?.kind !== "found") return { found: T.unknown, version: T.unknown, path: undefined };
  return { found: T.foundIn[r.step], version: r.version ?? T.unknown, path: r.path };
}

function useWindowStatus(): LspStatusWindow | undefined {
  const workspace = useEditorWorkspace();
  const services = workspace.lsp?.services;
  return services ? statusWindowOf(services) : undefined;
}

function InstanceRow({
  win,
  instance,
  worktree,
}: {
  win: LspStatusWindow;
  instance: LspInstanceStatus;
  worktree: string | undefined;
}) {
  const T = useToolsStrings();
  const P = T.popup;
  const view = describeState(instance, T.status);
  const info = useStore(win.store, (s) =>
    s.servers[worktree ?? ""]?.find((i) => i.id === instance.server),
  );
  const installing = useStore(win.store, (s) => s.installing[instance.server] === true);
  const found = foundText(info, P);
  const detail = stateDetail(instance.state, T);
  const kind = instance.state.kind;
  const running =
    kind === "ready" ||
    kind === "restartNeeded" ||
    kind === "restartSuggested" ||
    kind === "starting";
  return (
    <li
      data-testid="lsp-status-instance"
      data-server={instance.server}
      data-state={view.kind}
      className="flex flex-col gap-1 border-b border-ui-border py-2 last:border-b-0"
    >
      <div className="flex items-center gap-2 font-semibold">
        <view.Icon size={12} aria-hidden />
        <span>{instance.server}</span>
        <span className={view.warning ? "text-ui-accent" : "text-ui-muted-foreground"}>
          {view.label}
        </span>
      </div>
      <dl className="grid grid-cols-[max-content_1fr] gap-x-3 text-[12px]">
        <dt className="text-ui-muted-foreground">{P.root}</dt>
        <dd className="truncate" title={instance.root}>
          {instance.root}
        </dd>
        <dt className="text-ui-muted-foreground">{P.version}</dt>
        <dd>{found.version}</dd>
        <dt className="text-ui-muted-foreground">{P.found}</dt>
        <dd className="truncate" title={found.path}>
          {found.found}
          {found.path ? ` — ${found.path}` : ""}
        </dd>
      </dl>
      {detail && (
        <p data-testid="lsp-status-detail" className="text-[12px] whitespace-pre-wrap">
          {detail}
        </p>
      )}
      <div className="flex flex-wrap gap-1">
        {kind === "untrusted" && (
          <button type="button" className={BUTTON} onClick={() => win.trust()}>
            {T.trust}
          </button>
        )}
        {instance.state.kind === "notInstalled" && instance.state.mason && (
          <button
            type="button"
            data-testid="lsp-status-install"
            className={BUTTON}
            disabled={installing}
            onClick={() => void win.install(instance.server, worktree)}
          >
            {installing ? T.banner.installing(instance.server) : P.install}
          </button>
        )}
        {kind !== "untrusted" && kind !== "notInstalled" && kind !== "runtimeMissing" && (
          <button
            type="button"
            data-testid="lsp-status-restart"
            className={BUTTON}
            onClick={() => void win.restart(instance, worktree)}
          >
            {P.restart}
          </button>
        )}
        {running && (
          <button
            type="button"
            data-testid="lsp-status-stop"
            className={BUTTON}
            onClick={() => void win.stop(instance, worktree)}
          >
            {P.stop}
          </button>
        )}
        <button
          type="button"
          data-testid="lsp-status-log"
          className={BUTTON}
          onClick={() => void win.openLog(instance, worktree)}
        >
          {P.log}
        </button>
        {kind !== "disabled" && (
          <button
            type="button"
            data-testid="lsp-status-disable"
            className={BUTTON}
            onClick={() => void win.disable(instance.server, worktree)}
          >
            {P.disable}
          </button>
        )}
        {kind === "disabled" && (
          <button
            type="button"
            data-testid="lsp-status-enable"
            className={BUTTON}
            onClick={() => void win.enable(instance.server, worktree)}
          >
            {P.enable}
          </button>
        )}
        {info?.installPrompt === false && (
          <button
            type="button"
            data-testid="lsp-status-ask-again"
            className={BUTTON}
            onClick={() => void win.askAgain(instance.server, worktree)}
          >
            {P.askAgain}
          </button>
        )}
      </div>
    </li>
  );
}

function StatusPopup({ win, doc }: { win: LspStatusWindow; doc: StatusDoc & { key: string } }) {
  const T = useToolsStrings().popup;
  useKeymapPopup(true);
  const status = useStore(win.store, (s) => s.docs[doc.key]?.status);
  const close = () => win.store.setState({ popup: false });
  const closeButton = useRef<HTMLButtonElement>(null);
  return (
    <Dialog.Root open onOpenChange={(open) => !open && close()}>
      <Dialog.Portal>
        <Dialog.Content
          data-testid="lsp-status-popup"
          aria-describedby={undefined}
          // Radix 기본은 첫 단추(인스턴스의 [설치…] · [다시 시작])다 — 동작 단추가 아닌 [닫기] 에 둔다.
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            closeButton.current?.focus();
          }}
          className="fixed bottom-7 left-2 flex max-h-[70vh] w-[480px] max-w-[94vw] flex-col gap-1 overflow-y-auto rounded-md border border-ui-border bg-ui-panel px-3 py-2 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.title}</Dialog.Title>
          <ul>
            {status?.instances.map((instance) => (
              <InstanceRow
                key={`${instance.server}\u0000${instance.root}`}
                win={win}
                instance={instance}
                worktree={doc.worktree}
              />
            ))}
          </ul>
          <div className="flex justify-end">
            <button
              ref={closeButton}
              type="button"
              data-testid="lsp-status-close"
              className={BUTTON}
              onClick={close}
            >
              {T.close}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function LogDialog({ win }: { win: LspStatusWindow }) {
  const T = useToolsStrings().popup;
  const log = useStore(win.store, (s) => s.log);
  useKeymapPopup(log !== undefined);
  if (!log) return null;
  const close = () => win.store.setState({ log: undefined });
  return (
    <Dialog.Root open onOpenChange={(open) => !open && close()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="lsp-status-log-dialog"
          aria-describedby={undefined}
          className="fixed top-1/2 left-1/2 flex h-[70vh] w-[760px] max-w-[94vw] -translate-x-1/2 -translate-y-1/2 flex-col gap-2 rounded-md border border-ui-border bg-ui-panel px-3 py-2 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.logTitle(log.server)}</Dialog.Title>
          {log.file && <p className="truncate text-[12px] text-ui-muted-foreground">{log.file}</p>}
          {log.truncated && (
            <p className="text-[12px] text-ui-muted-foreground">{T.logTruncated}</p>
          )}
          <pre
            data-testid="lsp-status-log-text"
            className="min-h-0 flex-1 overflow-auto rounded-sm border border-ui-border p-2 font-mono text-[12px] whitespace-pre-wrap"
          >
            {log.text || T.logEmpty}
          </pre>
          <div className="flex justify-end">
            <button type="button" className={BUTTON} onClick={close}>
              {T.close}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function ConfirmFromWindow({ win }: { win: LspStatusWindow }) {
  const pending = useStore(win.store, (s) => s.confirm);
  if (!pending) return null;
  return (
    <ToolConfirmDialog
      request={pending.request}
      kind={pending.kind}
      detail={pending.detail}
      openUrl={(url) => win.services.openExternal?.(url)}
      onAnswer={(ok) => {
        win.store.setState({ confirm: undefined });
        pending.resolve(ok);
      }}
    />
  );
}

function ItemFor({ win }: { win: LspStatusWindow }) {
  const T = useToolsStrings();
  const active = useStore(win.store, (s) => (s.active ? s.docs[s.active] : undefined));
  const activeKey = useStore(win.store, (s) => s.active);
  const popup = useStore(win.store, (s) => s.popup);
  const instances = active?.status?.instances ?? [];
  const first = instances[0];
  return (
    <>
      {first && active && activeKey && (
        <ItemButton win={win} instance={first} more={instances.length - 1} T={T} />
      )}
      {popup && active && activeKey && (
        <StatusPopup win={win} doc={{ ...active, key: activeKey }} />
      )}
      <LogDialog win={win} />
      <ConfirmFromWindow win={win} />
    </>
  );
}

function ItemButton({
  win,
  instance,
  more,
  T,
}: {
  win: LspStatusWindow;
  instance: LspInstanceStatus;
  more: number;
  T: ToolsStrings;
}) {
  const view = describeState(instance, T.status);
  return (
    <button
      type="button"
      data-testid="status-lsp"
      data-state={view.kind}
      title={T.status.hint(instance.server)}
      onClick={() => win.store.setState({ popup: true })}
      className={`flex items-center gap-1 rounded-sm px-1 hover:bg-ui-hover ${view.warning ? "text-ui-accent" : ""}`}
    >
      <view.Icon size={11} aria-hidden data-icon={view.kind} />
      <span>{instance.server}</span>
      <span data-testid="status-lsp-label">{view.label}</span>
      {more > 0 && <span className="text-ui-muted-foreground">{T.status.more(more)}</span>}
    </button>
  );
}

/** 상태바 항목. 창의 상태 표가 없으면(파일 창 · Host 없음) 그리지 않는다. */
export function LspStatusItem() {
  const win = useWindowStatus();
  return win ? <ItemFor win={win} /> : null;
}

/** 지금 활성 탭의 LSP 문서. */
function activeDoc(workspace: EditorWorkspace): StatusDoc | undefined {
  const panel = workspace.activeTab();
  const view = panel && workspace.view(panel.id);
  const doc = view?.state.facet(editorContext)?.lsp;
  return doc ? { path: doc.path, language: doc.language, worktree: doc.worktree } : undefined;
}

/** 저장소를 못 쓸 때 마지막으로 본 때(epoch ms). 창(웹뷰) 하나에 하나다. */
const updatesMemory = { at: 0 };

type UpdatesStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

/**
 * 업데이트가 있으면 알린다(하루 한 번). `now` · `storage` · `memory` 는 시험이 준다.
 *
 * 부르기 전에 지금 시각을 적어 자리를 잡는다(먼저 적은 창이 이긴다 — 다른 창은 적힌 시각을 보고 묻지 않는다). `tools.updates` 가
 * 실패하면 자리를 되돌려 다음에 다시 묻는다. 저장소를 읽거나 쓸 수 없으면 `memory`(모듈 변수)로 그 창 안에서 하루 한 번이다.
 */
export async function checkToolUpdates(deps: {
  call: LspStatusWindow["services"]["call"];
  notify(input: {
    level: "info";
    title: string;
    body?: string;
    action?: { label: string; run(): void };
  }): void;
  /** 알릴 때의 문자열(답을 받은 뒤에 읽는다 — 창의 언어가 그사이 정해진다). */
  strings(): ToolsStrings;
  openTools(): void;
  now?: number;
  storage?: UpdatesStorage;
  memory?: { at: number };
}): Promise<boolean> {
  const now = deps.now ?? Date.now();
  const memory = deps.memory ?? updatesMemory;
  let storage: UpdatesStorage | undefined;
  try {
    storage = deps.storage ?? localStorage;
  } catch {
    storage = undefined;
  }
  // 저장소에 적힌 원래 값(되돌릴 때). 저장소를 못 쓰면 `undefined` 이고 `memory` 를 쓴다.
  let stored: string | null | undefined;
  try {
    stored = storage ? storage.getItem(UPDATES_CHECKED_KEY) : undefined;
  } catch {
    stored = undefined;
  }
  const last = stored === undefined ? memory.at : Number(stored ?? 0) || 0;
  if (now - last < UPDATES_INTERVAL_MS) return false;
  const before = memory.at;
  memory.at = now;
  let claimed = false;
  if (stored !== undefined) {
    try {
      storage!.setItem(UPDATES_CHECKED_KEY, String(now));
      claimed = true;
    } catch {
      // 쓰지 못하면 이 창은 `memory` 로만 하루 한 번이다.
    }
  }
  let updates;
  try {
    updates = await deps.call("tools.updates", null);
  } catch (e) {
    // 자리를 되돌린다 — 그사이 다른 창이 다시 적었으면(드물다) 그대로 둔다.
    if (memory.at === now) memory.at = before;
    if (claimed) {
      try {
        if (storage!.getItem(UPDATES_CHECKED_KEY) === String(now)) {
          if (stored === null) storage!.removeItem(UPDATES_CHECKED_KEY);
          else storage!.setItem(UPDATES_CHECKED_KEY, stored!);
        }
      } catch {
        // 되돌리지 못하면 다음 날 다시 묻는다.
      }
    }
    throw e;
  }
  if (updates.items.length === 0) return false;
  const T = deps.strings();
  deps.notify({
    level: "info",
    title: T.updatesTitle(updates.items.length),
    body: updates.items.map((u) => `${u.name} ${u.installed} → ${u.available}`).join("\n"),
    action: { label: T.manageAction, run: deps.openTools },
  });
  return true;
}

/**
 * 프로젝트 창에 언어 서버 상태를 붙인다: 상태 표(status-window.ts)를 만들고 활성 탭 문서를 쫓고, 상태바 `lsp` 항목 · 「도구 관리
 * 열기」 액션 · 업데이트 알림을 단다. `api` 는 도킹 영역(활성 탭이 바뀌면 다시 본다).
 */
export function useLspStatusUi(
  workspace: EditorWorkspace,
  projectId: string | undefined,
  api: DockviewApi | undefined,
) {
  const strings = useToolsStrings();
  const notify = useNotify();
  const { registry } = useKeymap();
  const { client } = useHost();
  // 창의 언어를 읽은 뒤에 알린다(그 전에는 폴백 언어다).
  const languageReady = useI18n().setting !== undefined;
  const latest = useRef({ strings, notify, registry });
  latest.current = { strings, notify, registry };
  const sessions = workspace.lsp;
  const [win, setWin] = useState<LspStatusWindow>();

  useEffect(() => {
    if (!sessions || projectId === undefined) return;
    const created = createStatusWindow({
      services: sessions.services,
      projectId,
      strings: () => latest.current.strings,
      notify: (input) => latest.current.notify(input),
      trust: () => void latest.current.registry.run(TRUST_ACTION),
      openSettings: () => void latest.current.registry.run(SETTINGS_ACTION),
    });
    setWin(created);
    return () => {
      created.dispose();
      setWin(undefined);
    };
  }, [sessions, projectId]);

  // 활성 탭 문서를 쫓는다(탭 바뀜 · 뷰가 생김 — 문서는 뷰를 구성할 때 붙는다).
  useEffect(() => {
    if (!win) return;
    let release: (() => void) | undefined;
    let current: string | undefined;
    let live = true;
    const refresh = () => {
      const doc = activeDoc(workspace);
      const key = doc && docKey(doc);
      if (key === current) return;
      current = key;
      release?.();
      release = doc ? win.track(doc) : undefined;
      win.setActive(doc);
    };
    const later = () =>
      queueMicrotask(() => {
        if (live) refresh();
      });
    refresh();
    const offCursor = workspace.onCursor(later);
    const active = api?.onDidActivePanelChange(later);
    const removed = api?.onDidRemovePanel(later);
    return () => {
      live = false;
      offCursor();
      active?.dispose();
      removed?.dispose();
      release?.();
      win.setActive(undefined);
    };
  }, [win, workspace, api]);

  // 업데이트 알림(하루 한 번 — 창이 떠 있는 동안 한 시간마다 다시 본다). Host 에 붙은 뒤에 묻는다.
  useEffect(() => {
    if (!sessions || !client || !languageReady) return;
    let live = true;
    const check = () =>
      void checkToolUpdates({
        call: sessions.services.call,
        notify: (input) => live && latest.current.notify(input),
        strings: () => latest.current.strings,
        openTools: () => void latest.current.registry.run(SETTINGS_ACTION),
      }).catch((e: unknown) => {
        if (!(e instanceof RpcCallError && e.code === ERROR_CODE.METHOD_NOT_FOUND))
          console.warn("[tools] tools.updates failed", e);
      });
    check();
    const timer = setInterval(check, 60 * 60 * 1000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [sessions, client, languageReady]);

  const actions = useMemo<Action[]>(
    () => [
      {
        id: TOOLS_MANAGE_ACTION,
        label: strings.manageAction,
        keywords: ["Tools", "Language Servers", "Mason", "LSP", "도구"],
        run: () => void latest.current.registry.run(SETTINGS_ACTION),
      },
    ],
    [strings.manageAction],
  );
  useRegisterActions(actions);
  useStatusBarItem({ id: "lsp", ...STATUS_SLOTS.lsp, component: LspStatusItem });
}
