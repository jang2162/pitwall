// 창 하나의 언어 서버 상태(15-m4-plan M4-17): 상태바 `lsp` 항목(활성 탭 문서) · 상태 팝업 · 탭 위 배너(탭마다의 문서)가 함께 보는
// 표와 그 동작. 문서를 맡는 인스턴스는 Host 가 고른다(`lsp.status.document` — 문서 열기와 같은 언어 짝짓기 규칙). 창은 서버 정의 ·
// 해석(`lsp.servers.list` — 찾은 곳 · 판 · `install_prompt`)과 설치 패키지의 라이선스(`tools.registry.get`)를 더 읽는다. `lsp.status`
// · `config.changed` 이벤트가 오면 다시 읽는다(한 틱에 여럿 와도 한 번).
//
// 동작:
// - 설치: 설치기의 확인 흐름(src/tools/install.ts — `4001` 이면 확인 다이얼로그, [설치] 를 누른 때만 `confirmed` 를 실어 다시
//   부른다). 끝나면 그 서버의 `notInstalled` 인스턴스마다 `lsp.server.restart` — Host 가 허가를 다시 받으므로 그사이 신뢰를 풀었으면
//   `untrusted` 로 남고 띄우지 않는다.
// - 「이 프로젝트에서 끄기」 · 「이 프로젝트에서 묻지 않기」 와 그 되돌리기(「이 프로젝트에서 켜기」 · 「설치 안내 다시 보기」):
//   `lsp.server.setLocal` — 커밋되지 않는 개인 파일(`project.local.toml`)에만 쓴다. 끄면 Host 가 그 인스턴스를 끝내고, 켜면 다시
//   띄운다. 쓰지 못하면(그 파일이 링크 · `.pitwall` 이 프로젝트 밖 등) Host 의 까닭을 알림으로 보인다.
// - 다시 시작 · 멈춤 · 로그 열기(`lsp.server.log` — 글로 보인다).
// - 신뢰: 창의 신뢰 액션(다이얼로그 — trust/trust.tsx). 신뢰를 얻으면 Host 가 열린 문서의 서버를 띄운다(M4-17 Host 규칙).
//
// 상태바 항목 · 배너 · 다이얼로그는 이 표를 그린다(lsp-status.tsx · install-banner.ts). 문서 기능(배너)은 창의 것을 import 하지 않고
// `doc.services` 로 찾는다(`statusWindowOf` — edit-window.ts 와 같은 꼴).

import {
  ERROR_CODE,
  type LspDocumentStatus,
  type LspInstanceStatus,
  type LspServerInfo,
  type LspServerLog,
  RpcCallError,
  type ToolConfirmRequired,
  type ToolPackageDetail,
} from "@pitwall/rpc-client";
import { createStore, type StoreApi } from "zustand";

import type { NotificationInput } from "../notifications/notifications";
import { changeTool, TOOLS_ERROR, type ToolChangeKind } from "../tools/install";
import type { ToolsStrings } from "../tools/strings";
import { LSP_STATUS_TOPIC, type LspServices } from "./lsp-sessions";

/** 설정이 다시 읽혔다(crates/pw-core 의 `TOPIC_CHANGED`) — 서버 설정(`[lsp.<id>]`)이 바뀌었을 수 있다. */
const CONFIG_CHANGED = "config.changed";

export interface StatusDoc {
  path: string;
  /** Linguist 이름(`Rust`). */
  language: string;
  /** 탭의 워크트리 꼭대기. 메인 체크아웃이면 없다. */
  worktree?: string;
}

export interface TrackedDoc extends StatusDoc {
  status?: LspDocumentStatus;
}

export interface PendingConfirm {
  request: ToolConfirmRequired;
  kind: ToolChangeKind;
  detail?: ToolPackageDetail;
  resolve(ok: boolean): void;
}

export interface LspStatusState {
  docs: Readonly<Record<string, TrackedDoc>>;
  /** 워크트리(없으면 "")마다 서버 정의 · 해석. */
  servers: Readonly<Record<string, readonly LspServerInfo[]>>;
  /** Mason 패키지 → 라이선스(`tools.registry.get` — 모르면 없음). */
  licenses: Readonly<Record<string, readonly string[]>>;
  /** 설치 중인 서버 id. */
  installing: Readonly<Record<string, true>>;
  /** 상태바 항목이 보이는 문서(활성 탭). */
  active?: string;
  popup: boolean;
  log?: LspServerLog & { server: string };
  confirm?: PendingConfirm;
}

export interface StatusWindowDeps {
  services: LspServices;
  projectId: string;
  strings(): ToolsStrings;
  notify(input: NotificationInput): void;
  /** 창의 신뢰 액션(다이얼로그를 연다). */
  trust(): void;
  /** 설정 창을 연다(「도구」 쪽). */
  openSettings(): void;
}

export interface LspStatusWindow {
  readonly store: StoreApi<LspStatusState>;
  readonly projectId: string;
  readonly services: LspServices;
  strings(): ToolsStrings;
  /** 문서의 상태를 쫓는다(탭 · 활성 문서). 해제 함수를 돌려준다. 같은 문서를 여럿이 쫓아도 한 줄이다. */
  track(doc: StatusDoc): () => void;
  /** 상태바 항목이 보일 문서. */
  setActive(doc: StatusDoc | undefined): void;
  /** 모두 다시 읽는다. 끝날 때 풀린다. */
  refresh(): Promise<void>;
  serverInfo(server: string, worktree: string | undefined): LspServerInfo | undefined;
  install(server: string, worktree: string | undefined): Promise<void>;
  dontAsk(server: string, worktree: string | undefined): Promise<void>;
  /** 「묻지 않기」 를 되돌린다(`install_prompt = true`). */
  askAgain(server: string, worktree: string | undefined): Promise<void>;
  disable(server: string, worktree: string | undefined): Promise<void>;
  /** 「끄기」 를 되돌린다(`enabled = true`). */
  enable(server: string, worktree: string | undefined): Promise<void>;
  restart(instance: LspInstanceStatus, worktree: string | undefined): Promise<void>;
  stop(instance: LspInstanceStatus, worktree: string | undefined): Promise<void>;
  openLog(instance: LspInstanceStatus, worktree: string | undefined): Promise<void>;
  trust(): void;
  openSettings(): void;
  dispose(): void;
}

const windows = new WeakMap<LspServices, LspStatusWindow>();

/** 문서의 창(`doc.services`)의 상태 표. 창이 붙지 않았으면(시험 · 파일 창) 없다. */
export function statusWindowOf(services: LspServices): LspStatusWindow | undefined {
  return windows.get(services);
}

export function docKey(doc: StatusDoc): string {
  return `${doc.worktree ?? ""}\u0000${doc.path}\u0000${doc.language}`;
}

const worktreeKey = (worktree: string | undefined) => worktree ?? "";

/** 읽기 실패를 남긴다. 그 메서드가 없는 Host(이 행 전의 판 · 원격)는 상태를 보이지 않을 뿐이라 남기지 않는다. */
function warn(what: string, e: unknown, ...more: unknown[]) {
  if (e instanceof RpcCallError && e.code === ERROR_CODE.METHOD_NOT_FOUND) return;
  console.warn(`[lsp-status] ${what} failed`, ...more, e);
}

function message(e: unknown): string {
  if (e instanceof RpcCallError && typeof e.data === "string") return `${e.message}: ${e.data}`;
  return e instanceof Error ? e.message : String(e);
}

export function createStatusWindow(deps: StatusWindowDeps): LspStatusWindow {
  const { services, projectId } = deps;
  const store = createStore<LspStatusState>()(() => ({
    docs: {},
    servers: {},
    licenses: {},
    installing: {},
    popup: false,
  }));
  const refs = new Map<string, number>();
  let disposed = false;
  let scheduled = false;
  let running: Promise<void> | undefined;
  let again = false;

  const set = (
    patch: Partial<LspStatusState> | ((s: LspStatusState) => Partial<LspStatusState>),
  ) => {
    if (!disposed) store.setState(patch);
  };

  async function loadDoc(key: string) {
    const doc = store.getState().docs[key];
    if (!doc) return;
    try {
      const status = await services.call("lsp.status.document", {
        projectId,
        worktree: doc.worktree ?? null,
        path: doc.path,
        language: doc.language,
      });
      set((s) => (s.docs[key] ? { docs: { ...s.docs, [key]: { ...s.docs[key], status } } } : {}));
    } catch (e) {
      warn("lsp.status.document", e, doc.path);
    }
  }

  async function loadServers(worktree: string | undefined) {
    try {
      const list = await services.call("lsp.servers.list", {
        projectId,
        worktree: worktree ?? null,
      });
      set((s) => ({ servers: { ...s.servers, [worktreeKey(worktree)]: list.servers } }));
    } catch (e) {
      warn("lsp.servers.list", e);
    }
  }

  /** 설치를 제안할 패키지의 라이선스를 한 번 읽는다. */
  async function loadLicenses() {
    const { docs, licenses } = store.getState();
    const wanted = new Set<string>();
    for (const doc of Object.values(docs))
      for (const i of doc.status?.instances ?? [])
        if (i.state.kind === "notInstalled" && i.state.mason && !(i.state.mason in licenses))
          wanted.add(i.state.mason);
    await Promise.all(
      [...wanted].map(async (name) => {
        try {
          const detail = await services.call("tools.registry.get", { name });
          set((s) => ({ licenses: { ...s.licenses, [name]: detail.summary.licenses } }));
        } catch {
          set((s) => ({ licenses: { ...s.licenses, [name]: [] } }));
        }
      }),
    );
  }

  async function refreshAll() {
    const { docs } = store.getState();
    const worktrees = new Set(Object.values(docs).map((d) => worktreeKey(d.worktree)));
    await Promise.all([
      ...Object.keys(docs).map(loadDoc),
      ...[...worktrees].map((w) => loadServers(w === "" ? undefined : w)),
    ]);
    await loadLicenses();
  }

  /** 다시 읽기는 한 번에 하나 — 도는 중에 또 오면 끝난 뒤 한 번 더. */
  function refresh(): Promise<void> {
    if (running) {
      again = true;
      return running;
    }
    running = (async () => {
      do {
        again = false;
        await refreshAll();
      } while (again && !disposed);
    })().finally(() => {
      running = undefined;
    });
    return running;
  }

  function schedule() {
    if (scheduled) return;
    scheduled = true;
    setTimeout(() => {
      scheduled = false;
      if (!disposed) void refresh();
    }, 0);
  }

  const unsubscribe = services.subscribe([LSP_STATUS_TOPIC, CONFIG_CHANGED], schedule);

  function serverInfo(server: string, worktree: string | undefined) {
    return store.getState().servers[worktreeKey(worktree)]?.find((s) => s.id === server);
  }

  function ask(request: ToolConfirmRequired, kind: ToolChangeKind): Promise<boolean> {
    return new Promise<boolean>((resolve) => {
      store.getState().confirm?.resolve(false);
      set({ confirm: { request, kind, resolve } });
      services.call("tools.registry.get", { name: request.name }).then(
        (detail) =>
          set((s) =>
            s.confirm && s.confirm.request === request ? { confirm: { ...s.confirm, detail } } : {},
          ),
        () => {},
      );
    });
  }

  /** 그 서버의 인스턴스(쫓는 문서들의 것, 루트마다 하나). */
  function instancesOf(server: string, worktree: string | undefined): LspInstanceStatus[] {
    const roots = new Map<string, LspInstanceStatus>();
    for (const doc of Object.values(store.getState().docs)) {
      if (worktreeKey(doc.worktree) !== worktreeKey(worktree)) continue;
      for (const i of doc.status?.instances ?? []) if (i.server === server) roots.set(i.root, i);
    }
    return [...roots.values()];
  }

  const control = (instance: LspInstanceStatus, worktree: string | undefined) => ({
    projectId,
    worktree: worktree ?? null,
    server: instance.server,
    root: instance.root,
  });

  const failed = (e: unknown) =>
    deps.notify({ level: "error", title: deps.strings().popup.actionFailed, body: message(e) });

  async function setInstallPrompt(server: string, worktree: string | undefined, value: boolean) {
    try {
      await services.call("lsp.server.setLocal", {
        projectId,
        worktree: worktree ?? null,
        server,
        key: "installPrompt",
        value,
      });
      await loadServers(worktree);
    } catch (e) {
      failed(e);
    }
  }

  async function setEnabled(server: string, worktree: string | undefined, value: boolean) {
    try {
      const { file } = await services.call("lsp.server.setLocal", {
        projectId,
        worktree: worktree ?? null,
        server,
        key: "enabled",
        value,
      });
      if (!value) deps.notify({ level: "info", title: deps.strings().popup.disabledNote(file) });
      await refresh();
    } catch (e) {
      failed(e);
    }
  }

  const win: LspStatusWindow = {
    store,
    projectId,
    services,
    strings: deps.strings,
    track(doc) {
      const key = docKey(doc);
      refs.set(key, (refs.get(key) ?? 0) + 1);
      if (!store.getState().docs[key]) {
        set((s) => ({ docs: { ...s.docs, [key]: { ...doc } } }));
        void loadDoc(key).then(loadLicenses);
        if (!store.getState().servers[worktreeKey(doc.worktree)]) void loadServers(doc.worktree);
      }
      let released = false;
      return () => {
        if (released) return;
        released = true;
        const left = (refs.get(key) ?? 1) - 1;
        if (left > 0) {
          refs.set(key, left);
          return;
        }
        refs.delete(key);
        set((s) => {
          const { [key]: _gone, ...docs } = s.docs;
          return { docs };
        });
      };
    },
    setActive(doc) {
      set({ active: doc ? docKey(doc) : undefined });
    },
    refresh,
    serverInfo,
    async install(server, worktree) {
      const mason = serverInfo(server, worktree)?.mason;
      const T = deps.strings();
      if (!mason || store.getState().installing[server]) return;
      set((s) => ({ installing: { ...s.installing, [server]: true } }));
      try {
        const done = await changeTool(services.call, ask, "install", mason);
        if (!done) return;
        // 설치 뒤 기동: Host 가 허가를 다시 받는다(그사이 신뢰를 풀었으면 띄우지 않는다).
        for (const instance of instancesOf(server, worktree))
          if (instance.state.kind === "notInstalled")
            await services.call("lsp.server.restart", control(instance, worktree));
        await refresh();
      } catch (e) {
        const runtime =
          e instanceof RpcCallError && e.code === TOOLS_ERROR.RUNTIME_MISSING
            ? (e.data as { runtime?: string } | undefined)?.runtime
            : undefined;
        deps.notify({
          level: "error",
          title: T.banner.failed(mason),
          body: runtime ? T.manage.runtimeMissing(runtime) : message(e),
        });
      } finally {
        set((s) => {
          const { [server]: _done, ...installing } = s.installing;
          return { installing };
        });
      }
    },
    dontAsk: (server, worktree) => setInstallPrompt(server, worktree, false),
    askAgain: (server, worktree) => setInstallPrompt(server, worktree, true),
    disable: (server, worktree) => setEnabled(server, worktree, false),
    enable: (server, worktree) => setEnabled(server, worktree, true),
    async restart(instance, worktree) {
      try {
        await services.call("lsp.server.restart", control(instance, worktree));
        await refresh();
      } catch (e) {
        failed(e);
      }
    },
    async stop(instance, worktree) {
      try {
        await services.call("lsp.server.stop", control(instance, worktree));
        await refresh();
      } catch (e) {
        failed(e);
      }
    },
    async openLog(instance, worktree) {
      try {
        const log = await services.call("lsp.server.log", control(instance, worktree));
        set({ log: { ...log, server: instance.server } });
      } catch (e) {
        failed(e);
      }
    },
    trust: deps.trust,
    openSettings: deps.openSettings,
    dispose() {
      disposed = true;
      unsubscribe();
      store.getState().confirm?.resolve(false);
      if (windows.get(services) === win) windows.delete(services);
    },
  };
  windows.set(services, win);
  return win;
}

/** 문서의 상태 중 상태바 · 배너가 앞세울 인스턴스(서버 정의 순서의 첫 것). */
export function primaryInstance(
  status: LspDocumentStatus | undefined,
): LspInstanceStatus | undefined {
  return status?.instances[0];
}
