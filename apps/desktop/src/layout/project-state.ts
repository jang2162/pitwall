// 프로젝트 창의 상태(결정 M1-D19): 레이아웃(project-layout.ts)과 워크트리별 탭 묶음(worktree-session.ts)을 Host 의
// `$DATA/projects/<id>/state.json` 에 둔다(`project.state.get` · `set`, crates/pw-core/src/registry/state.rs). Host 는 내용을 보지
// 않는다 — 형식은 여기서 정한다.
//
// - 창이 열릴 때 한 번 읽고(`load`), 그 뒤로는 메모리의 값을 고치고 모아 쓴다(`WRITE_DELAY_MS`). 창을 닫기 전(창의 닫기 확인,
//   project-window.tsx)에 `flush` 로 기다리던 쓰기를 끝낸다 — 닫기는 `CLOSE_FLUSH_LIMIT_MS` 까지만 기다린다(Host 가 답하지 않아도
//   창 닫기 · 종료 · 업데이트 재시작이 멈추지 않게).
// - 읽지 못했으면(Host 오류 · 메서드 없음 · `LOAD_LIMIT_MS` 안에 답이 없음 · Host 에 닿지 못함 `giveUp`) 기본값으로 열고 **이 창은
//   쓰지 않는다** — 일시적인 오류로 저장본을 기본값으로 덮지 않게. 늦게 온 답은 버린다.
//   파일이 깨졌으면(Host 가 `state: null` 과 `problem` 을 준다) 기본값으로 열고 다음 쓰기가 덮는다.
// - 레이아웃은 `LayoutStorage`(project-layout.ts 의 동기 저장소 꼴)로 내놓는다 — 레이아웃 코드는 저장소가 무엇인지 모른다.
// - 옛 저장본: 예전에는 웹뷰 localStorage 한 키(`LAYOUT_STORAGE_KEY`)를 모든 프로젝트 창이 함께 썼다. 이 프로젝트에 저장된 레이아웃이
//   없으면 처음 연 창이 그것을 옮기고, 쓰기가 끝나면 지운다 — 대개 한 번만 옮겨진다(다음 프로젝트는 기본 레이아웃). 두 창이
//   동시에 처음 열리면(첫 쓰기 전에 둘 다 읽으면) 둘 다 옮긴다 — 서로 덮지는 않는다.
// - 워크트리 묶음은 최근에 쓴 `MAX_WORKTREES` 개만 남긴다(메인 체크아웃은 늘 남는다) — 지운 워크트리의 묶음이 쌓이지 않게.

import type { JsonValue, RpcClient } from "@pitwall/rpc-client";

import type { Cursor } from "../editor/editor-workspace";
import { LAYOUT_STORAGE_KEY, type LayoutStorage } from "./project-layout";

/** 형식이 호환되지 않게 바뀌면 올린다. 다른 버전의 저장본은 버리고 기본값으로 연다. */
export const STATE_VERSION = 1;
export const WRITE_DELAY_MS = 300;
export const MAX_WORKTREES = 20;
/** 저장본 읽기를 기다리는 시간. 넘으면 기본값으로 연다(머리 주석). */
export const LOAD_LIMIT_MS = 2000;
/** 창을 닫을 때 마지막 쓰기를 기다리는 시간. */
export const CLOSE_FLUSH_LIMIT_MS = 1500;

/** 탭 하나. 경로는 정규화한 실제 경로(탭의 키, editor-tabs.ts). */
export interface TabState {
  path: string;
  /**
   * 탭을 연 워크트리가 묶음의 워크트리와 다를 때만(⌘E · Search Everywhere 로 다른 워크트리의 파일을 연 탭): 그 워크트리 꼭대기,
   * 메인 체크아웃이면 `null`. 없으면 묶음의 워크트리다(옛 저장본도 그렇게 읽는다).
   */
  worktree?: string | null;
  preview?: boolean;
  pinned?: boolean;
  cursor?: Cursor;
  /** 화면 맨 위 줄(1부터). */
  top?: number;
}

/** 워크트리 하나의 탭 묶음(04-features.md 「워크트리 경로 처리 (F2-5)」 의 「워크트리별 상태」). */
export interface WorktreeState {
  tabs: TabState[];
  /** 활성 탭의 `tabs` 위치. */
  active?: number;
  /** 파일 트리의 펼침. `root` 가 그때의 트리 루트다 — 루트가 같을 때만 되살린다. */
  tree?: { root: string; expanded: string[] };
  /** 마지막으로 이 묶음을 쓴 시각(ms). 오래된 묶음부터 버린다. */
  usedAt: number;
}

export interface ProjectStateFile {
  version: typeof STATE_VERSION;
  /** project-layout.ts 의 저장본(`{ version, layout, hidden }`). */
  layout?: unknown;
  /** 메인 체크아웃의 묶음. */
  main?: WorktreeState;
  /** 워크트리 꼭대기 경로(`ActiveWorktree.top`) → 묶음. */
  worktrees?: Record<string, WorktreeState>;
  /** 이 창에서 최근에 본 파일(⌘E, search-everywhere/recent-files.ts), 앞이 최근. 워크트리를 가리지 않는 창 하나의 목록이다. */
  recentFiles?: RecentFileState[];
}

/** 최근 파일 하나(search-everywhere/recent-files.ts 의 `RecentFile` 과 같은 꼴). */
export interface RecentFileState {
  path: string;
  worktree?: string;
}

/** 묶음의 열쇠: 워크트리 꼭대기, 메인 체크아웃이면 `undefined`. */
export type WorktreeKey = string | undefined;

export type StateLoad = "loaded" | "empty" | "broken" | "unavailable";

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 저장본의 묶음 하나를 읽는다. 틀린 탭은 그 탭만 버린다. */
function readWorktree(value: unknown): WorktreeState | undefined {
  if (!isObject(value) || !Array.isArray(value.tabs)) return undefined;
  const tabs: TabState[] = [];
  for (const tab of value.tabs as unknown[]) {
    if (!isObject(tab) || typeof tab.path !== "string" || tab.path === "") continue;
    const cursor = tab.cursor;
    tabs.push({
      path: tab.path,
      ...((typeof tab.worktree === "string" || tab.worktree === null) && {
        worktree: tab.worktree,
      }),
      ...(tab.preview === true && { preview: true }),
      ...(tab.pinned === true && { pinned: true }),
      ...(isObject(cursor) &&
        Number.isInteger(cursor.line) &&
        Number.isInteger(cursor.column) && {
          cursor: { line: cursor.line as number, column: cursor.column as number },
        }),
      ...(Number.isInteger(tab.top) && { top: tab.top as number }),
    });
  }
  const tree = value.tree;
  return {
    tabs,
    ...(Number.isInteger(value.active) && { active: value.active as number }),
    ...(isObject(tree) &&
      typeof tree.root === "string" &&
      Array.isArray(tree.expanded) && {
        tree: {
          root: tree.root,
          expanded: (tree.expanded as unknown[]).filter((p): p is string => typeof p === "string"),
        },
      }),
    usedAt: typeof value.usedAt === "number" ? value.usedAt : 0,
  };
}

/** Host 가 준 값을 이 창이 쓸 수 있는 꼴로 읽는다. 버전이 다르거나 객체가 아니면 `undefined`(기본값으로 연다). */
export function readStateFile(value: unknown): ProjectStateFile | undefined {
  if (!isObject(value) || value.version !== STATE_VERSION) return undefined;
  const worktrees: Record<string, WorktreeState> = {};
  if (isObject(value.worktrees))
    for (const [top, bundle] of Object.entries(value.worktrees)) {
      const read = readWorktree(bundle);
      if (read) worktrees[top] = read;
    }
  const main = readWorktree(value.main);
  const recentFiles = Array.isArray(value.recentFiles)
    ? (value.recentFiles as unknown[]).flatMap((f): RecentFileState[] =>
        isObject(f) && typeof f.path === "string" && f.path !== ""
          ? [{ path: f.path, ...(typeof f.worktree === "string" && { worktree: f.worktree }) }]
          : [],
      )
    : undefined;
  return {
    version: STATE_VERSION,
    ...(value.layout !== undefined && { layout: value.layout }),
    ...(main && { main }),
    worktrees,
    ...(recentFiles && { recentFiles }),
  };
}

/** 저장할 것. 멈춘 쓰기 · 시험용 메모리 저장도 이 꼴이다. */
export interface StateBackend {
  get(): Promise<{ state: unknown; problem: string | null }>;
  set(state: ProjectStateFile): Promise<void>;
}

export function hostStateBackend(client: () => RpcClient, projectId: string): StateBackend {
  return {
    get: () => client().call("project.state.get", { projectId }),
    set: async (state) => {
      await client().call("project.state.set", {
        projectId,
        state: state as unknown as JsonValue,
      });
    },
  };
}

/**
 * 창 하나의 프로젝트 상태. `legacy` 는 옛 레이아웃이 있던 웹뷰 저장소(머리 주석)다.
 */
export function createProjectState({
  backend,
  legacy,
  delayMs = WRITE_DELAY_MS,
  loadLimitMs = LOAD_LIMIT_MS,
  now = Date.now,
  log = (message) => console.warn("[state]", message),
}: {
  /** 없으면 메모리에만 둔다(프로젝트가 없는 창 · 시험). */
  backend?: StateBackend;
  legacy?: LayoutStorage;
  delayMs?: number;
  loadLimitMs?: number;
  now?: () => number;
  log?: (message: string) => void;
}) {
  let state: ProjectStateFile = { version: STATE_VERSION, worktrees: {} };
  let writable = false;
  /** 마지막 쓰기 뒤로 바뀐 것이 있다. */
  let dirty = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let writing: Promise<void> = Promise.resolve();
  /** 쓰기가 끝나면 지울 옛 레이아웃. */
  let migrated = false;
  let loading: Promise<StateLoad> | undefined;

  function prune() {
    const entries = Object.entries(state.worktrees ?? {});
    if (entries.length <= MAX_WORKTREES) return;
    entries.sort(([, a], [, b]) => b.usedAt - a.usedAt);
    state.worktrees = Object.fromEntries(entries.slice(0, MAX_WORKTREES));
  }

  function write(): Promise<void> {
    if (timer !== undefined) {
      clearTimeout(timer);
      timer = undefined;
    }
    if (!backend || !writable || !dirty) return writing;
    dirty = false;
    const snapshot = structuredClone(state);
    const clearLegacy = migrated;
    writing = writing
      .then(() => backend.set(snapshot))
      .then(() => {
        if (!clearLegacy) return;
        migrated = false;
        try {
          legacy?.removeItem(LAYOUT_STORAGE_KEY);
        } catch {
          // 지우지 못하면 다음 창이 다시 옮길 뿐이다 — 그 프로젝트에 저장본이 없을 때만.
        }
      })
      .catch((e: unknown) => {
        // 다음 쓰기(다음 변경 · 창을 닫을 때)에서 다시 쓴다.
        dirty = true;
        log(`could not save the project state: ${String(e)}`);
      });
    return writing;
  }

  function schedule() {
    dirty = true;
    timer ??= setTimeout(() => void write(), delayMs);
  }

  async function load(): Promise<StateLoad> {
    if (!backend) return "empty";
    let got: { state: unknown; problem: string | null } | "late";
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      got = await Promise.race([
        backend.get(),
        new Promise<"late">((resolve) => (timer = setTimeout(() => resolve("late"), loadLimitMs))),
      ]);
    } catch (e) {
      log(`could not read the project state, not saving it from this window: ${String(e)}`);
      return "unavailable";
    } finally {
      clearTimeout(timer);
    }
    if (got === "late") {
      log(`the project state did not arrive in ${loadLimitMs} ms, not saving it from this window`);
      return "unavailable";
    }
    writable = true;
    let result: StateLoad = "empty";
    if (got.problem) log(`the saved project state is unreadable: ${got.problem}`);
    if (got.state !== null && got.state !== undefined) {
      const read = readStateFile(got.state);
      if (read) {
        state = read;
        result = "loaded";
      } else log("the saved project state has another version or shape, starting over");
    }
    if (got.problem || (got.state != null && result !== "loaded")) result = "broken";
    if (state.layout === undefined && legacy) {
      let raw: string | null = null;
      try {
        raw = legacy.getItem(LAYOUT_STORAGE_KEY);
      } catch {
        raw = null;
      }
      if (raw !== null) {
        try {
          state.layout = JSON.parse(raw) as unknown;
          migrated = true;
          schedule();
        } catch {
          // 읽을 수 없는 옛 저장본은 옮기지 않는다 — 레이아웃 복원이 기본 레이아웃으로 연다.
        }
      }
    }
    return result;
  }

  function bundleOf(key: WorktreeKey): WorktreeState | undefined {
    return key === undefined ? state.main : state.worktrees?.[key];
  }

  return {
    /** 저장본을 읽는다. 두 번째부터는 첫 결과를 돌려준다(StrictMode 의 effect 다시 걸기). */
    load(): Promise<StateLoad> {
      loading ??= load();
      return loading;
    },

    /** Host 에 닿지 못해 읽기를 포기한다(머리 주석). 이미 읽기를 시작했으면 그 결과를 돌려준다. */
    giveUp(): Promise<StateLoad> {
      loading ??= Promise.resolve("unavailable");
      return loading;
    },

    /** 레이아웃 저장소(project-layout.ts 의 `restoreLayout` · `persistLayout`). */
    layout: {
      getItem: (key: string) =>
        key === LAYOUT_STORAGE_KEY && state.layout !== undefined
          ? JSON.stringify(state.layout)
          : null,
      setItem(key: string, value: string) {
        if (key !== LAYOUT_STORAGE_KEY) return;
        state.layout = JSON.parse(value) as unknown;
        schedule();
      },
      removeItem(key: string) {
        if (key !== LAYOUT_STORAGE_KEY) return;
        delete state.layout;
        schedule();
      },
    } satisfies LayoutStorage,

    bundle: (key: WorktreeKey): WorktreeState | undefined => bundleOf(key),

    /** 워크트리의 묶음을 바꾼다. */
    setBundle(key: WorktreeKey, bundle: Omit<WorktreeState, "usedAt">) {
      const next: WorktreeState = { ...bundle, usedAt: now() };
      if (key === undefined) state.main = next;
      else {
        state.worktrees = { ...state.worktrees, [key]: next };
        prune();
      }
      schedule();
    },

    /** 최근 파일(⌘E). */
    recentFiles: (): readonly RecentFileState[] => state.recentFiles ?? [],
    setRecentFiles(files: readonly RecentFileState[]) {
      state.recentFiles = files.map((f) => ({
        path: f.path,
        ...(f.worktree !== undefined && { worktree: f.worktree }),
      }));
      schedule();
    },

    /** 기다리던 쓰기를 지금 하고 끝날 때까지 기다린다. `limitMs` 를 주면 그만큼만 기다린다(쓰기는 뒤에서 마저 돈다). */
    flush(limitMs?: number): Promise<void> {
      const done = write();
      if (limitMs === undefined) return done;
      return new Promise<void>((resolve) => {
        const timer = setTimeout(() => {
          log(`the project state did not finish saving in ${limitMs} ms, going on`);
          resolve();
        }, limitMs);
        void done.then(() => {
          clearTimeout(timer);
          resolve();
        });
      });
    },

    /** 지금 메모리의 상태(시험 · 진단). */
    snapshot: (): ProjectStateFile => structuredClone(state),
  };
}

export type ProjectState = ReturnType<typeof createProjectState>;
