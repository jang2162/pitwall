// 문서 모델(12-m1-plan M1-09 · M1-13). 열린 파일 하나 = 문서 하나이고 키는 정규화한 실제 경로(`fs.stat` · `fs.read` 의 `path`)다.
// 여러 탭(그룹)이 같은 파일을 보이면 문서는 하나이고, 탭마다 따로인 것은 뷰(커서 · 스크롤)뿐이다. 탭이 문서를 쓰는 동안 `acquire` 로
// 잡고, 마지막 탭이 놓으면 문서를 버린다 — 다음에 열면 디스크에서 다시 읽는다.
//
// 읽기 순서: `fs.stat` 으로 크기 등급(결정 M1-D9)을 먼저 본다. 뷰어 종류(viewer-kinds.ts)가 바이트를 스스로 읽는 뷰어(이미지)면
// 고르면 여기서 내용을 읽지 않는다. 50 MiB 를 넘으면 열지 않는다(`tooLarge`). 나머지는 앞부분으로 바이너리인지 보고(큰 파일을
// 통째로 base64 로 받지 않게), 텍스트면 전체를 읽는다. UTF-8 로 읽히지 않는 텍스트는 `notUtf8` 이다 — 인코딩 짐작 · 읽기 전용
// 표시는 M1-23 몫이다(결정 M1-D12). 텍스트로 읽은 문서는 앞부분으로 언어를 정한다(`lang.detect`, M1-02) — 못 정하면
// 플레인 텍스트(`null`)로 열고 오류로 보지 않는다.
//
// 편집(M1-13): 텍스트 문서의 편집 중인 텍스트 · undo 기록 · 미저장 여부는 `LiveText`(live-text.ts)가 갖는다 — 탭을 옮기거나 뷰를
// 다시 만들어도 문서가 사는 동안 남는다. 저장은 `⌘S` 로만 한다(결정 M1-D13, 자동 저장 없음). 저장은 늘 읽을 때(또는 마지막
// 저장)의 `version` 을 기대 값으로 넘겨 그사이 디스크가 바뀌었으면 덮지 않는다(`CONFLICT` → 「디스크에서 바뀜」).
//
// 디스크와 맞추기(F3-6): 창이 받은 `fs.changed`(`changed`)에 열린 문서의 경로가 있으면 디스크를 본다. 크기 · 수정 시각이 이 문서가
// 마지막으로 본 것과 같으면 아무것도 하지 않는다 — 자기 저장(`fs.write`)이 돌아온 이벤트가 여기서 걸러진다. 다르면 다시 읽어
// `version` 을 견주고, 바뀌었으면:
//   - 미저장이 없으면 새 텍스트를 바로 넣는다(`LiveText.replace` — 차이만 바꿔 커서 · 스크롤이 남는다).
//   - 미저장이 있으면 넣지 않고 `disk` 를 `changed` 로 둔다 — 탭이 배너 「디스크에서 바뀜」 을 띄운다([디스크 것 받기]
//     `takeDisk` · [내 것 유지] `keepMine` · [비교]).
//   - 내용이 이 문서가 디스크에 있다고 아는 것과 바이트로 같으면(`touch` · 같은 내용을 다시 쓴 포매터 · `git checkout`) 크기 ·
//     시각 · `version` 만 바꾼다 — 배너가 없다.
//   - 지워졌으면 `deleted`, 같은 이벤트에 생기거나 바뀐 다른 파일 중 내용(`version`)이 같은 것이 있으면 `renamed`. 새 경로는
//     `created` 가 아니라 `modified` 로 올 수도 있다(rename 은 감시 백엔드마다 다르게 보고된다 · 원자 쓰기).
// 문서 하나의 저장과 맞추기는 차례로 돈다(`queue`) — 저장하는 중에 온 자기 쓰기의 이벤트는 저장이 끝난 뒤의 `version` 으로 본다.
//
// 미저장 편집은 사람이 고르지 않으면 버리지 않는다. 줄 구분이 바뀌어 문서를 통째로 다시 읽는(`reload`) 동안 RPC 를 기다리므로
// 그사이 친 글자는 옛 `LiveText` 에만 있다 — 다 읽은 뒤 그사이 고친 것이 있으면 바꾸지 않고 다시 맞춰 배너로 둔다.
//
// 바뀜 구독(`onChange`, M1-17 의 `syntax.edit`): 문서마다 한 번, 바뀐 범위(`ChangeSet`)와 바뀌기 전 텍스트를 준다. 같은 파일을
// 두 그룹이 보여도 한 번이다. 문서를 통째로 다시 읽으면(처음 읽음 포함) `reset` 으로 새 텍스트 전체를 준다.
//
// 접근 범위: 문서는 처음 잡은 탭의 워크트리(`EditorTabParams.worktree`)로 읽는다. 창의 활성 워크트리는 열기 요청마다 바뀌므로
// 읽는 순간의 값을 쓰면 다른 워크트리의 요청이 뒤섞인다.

import {
  ERROR_CODE,
  type FsRead,
  type FsReadParams,
  type FsStat,
  type FsWriteParams,
  type FsWritten,
  type Judgment,
} from "@pitwall/rpc-client";
import type { ChangeSet, Text } from "@codemirror/state";
import { useEffect } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { LiveText } from "./live-text";
import { VIEWER_READS, type ViewerKind, viewerKindByName, viewerKindOf } from "./viewer-kinds";

/** 바이너리 판정에 앞부분만 먼저 읽는 크기 경계. 이보다 작은 파일은 한 번에 읽는다. */
export const SNIFF_ABOVE = 64 * 1024;
/** 앞부분 읽기의 길이(crates/pw-fs judge.rs 의 `SNIFF_BYTES`). */
const SNIFF_BYTES = 8 * 1024;
/** 이름 바뀜을 찾을 때 열어 보는 새 파일의 수 상한(한 이벤트에서). */
const RENAME_CANDIDATES = 8;

/** 프로젝트 범위 이벤트 `fs.changed` 의 토픽(crates/pw-fs/src/watch/mod.rs 의 `TOPIC_CHANGED`). */
export const FS_CHANGED_TOPIC = "fs.changed";

/** `fs.changed` 의 내용(crates/pw-fs/src/watch/mod.rs 의 `FsChanged`). */
export interface FsChangedEvent {
  root: string;
  worktree: string | null;
  /** 바뀐 절대 경로. ignore 된 폴더 안의 변경은 그 폴더 하나로 접혀 올 수 있다. */
  paths: string[];
  kinds: ("created" | "modified" | "removed")[];
  /** 놓친 변경이 있을 수 있다 — 루트 아래 전부를 다시 본다. */
  rescan: boolean;
}

/** 디스크 쪽 상태. 탭의 배너가 이것을 그린다. */
export type DiskState =
  /** 디스크가 이 문서가 아는 것과 같다(미저장 편집은 있을 수 있다). */
  | { kind: "same" }
  /** 디스크가 바뀌었는데 미저장이 있어 받지 않았거나, 저장이 `CONFLICT` 로 멈췄다. `read` 는 그때 읽은 디스크. */
  | { kind: "changed"; read: FsRead }
  | { kind: "deleted" }
  /** 지워진 것과 같은 때 같은 내용의 파일이 생겼다. */
  | { kind: "renamed"; to: string };

export type DocumentState =
  | { status: "loading" }
  | { status: "failed"; message: string }
  /** 50 MiB 초과(결정 M1-D9). 내용을 읽지 않았다. */
  | { status: "tooLarge"; size: number }
  /** UTF-8 로 읽히지 않는 텍스트(결정 M1-D12). */
  | { status: "notUtf8"; size: number }
  /** 내용 없이 뷰어가 직접 읽는 문서(이미지 · hex). */
  | { status: "external"; kind: ViewerKind; stat: FsStat }
  | {
      status: "text";
      kind: ViewerKind;
      /** 편집 중인 텍스트 · undo 기록 · 뷰 허브. 문서가 사는 동안 같은 객체다(디스크에서 통째로 다시 읽으면 바뀐다). */
      live: LiveText;
      judgment: Judgment;
      /** 디스크에 있다고 아는 내용의 `version`. 다음 `fs.write` 의 `expectedVersion`. */
      version: string | null;
      size: number;
      mtimeMs: number | null;
      /** `lang.detect` 가 정한 Linguist 언어 이름. `null` 이면 플레인 텍스트. 사람이 고른 언어는 `language.ts` 가 따로 든다. */
      language: string | null;
      /** 이 앱이 고칠 수 없다: 크기 등급 · 인코딩 · 쓰기 권한. */
      readOnly: boolean;
      /** 저장하지 않은 편집이 있다(`live.dirty`). */
      dirty: boolean;
      disk: DiskState;
      /** `fs.write` 를 기다리는 중. */
      saving: boolean;
    };

export type TextDocumentState = Extract<DocumentState, { status: "text" }>;

/**
 * 문서를 읽고 쓰는 길. 창은 Host RPC(`fs.stat` · `fs.read` · `fs.write` · `lang.detect`)를 넘기고 시험은 가짜를 넘긴다. `worktree` 는
 * 접근 범위다(없으면 메인 체크아웃).
 */
export interface DocumentSource {
  stat(path: string, worktree: string | undefined): Promise<FsStat>;
  read(
    params: Pick<FsReadParams, "path" | "offset" | "length">,
    worktree: string | undefined,
  ): Promise<FsRead>;
  /** 없으면 문서를 저장할 수 없다(늘 `failed`). */
  write?(
    params: Pick<FsWriteParams, "path" | "content" | "bom" | "expectedVersion">,
    worktree: string | undefined,
  ): Promise<FsWritten>;
  /** 언어. 없으면 늘 플레인 텍스트다. */
  detect?(path: string, head: string, worktree: string | undefined): Promise<string | null>;
}

/** `DocumentStore.onChange` 가 주는 바뀜. */
export type DocumentChange =
  /** 편집 · undo · 디스크에서 받은 차이. `changes` 의 `fromA` · `toA` 는 `before`, `fromB` · `toB` 는 `after` 의 위치다. */
  | { kind: "edit"; changes: ChangeSet; before: Text; after: Text }
  /** 문서를 (다시) 읽었다 — 앞의 것과 이어지지 않는 새 텍스트. */
  | { kind: "reset"; text: Text };

export type SaveOutcome =
  /** 썼다. */
  | { kind: "saved" }
  /** 쓸 것이 없다(미저장 없음 · 읽기 전용 · 텍스트가 아님). */
  | { kind: "unchanged" }
  /** 디스크가 그사이 바뀌어 쓰지 않았다. 문서의 `disk` 가 배너 상태가 됐다. */
  | { kind: "conflict" }
  | { kind: "failed"; message: string };

interface DocumentsState {
  docs: Readonly<Record<string, DocumentState>>;
}

export interface DocumentStore {
  /** 문서를 쓰기 시작한다. 처음 잡는 것이면 `worktree` 를 접근 범위로 읽기 시작한다. 돌려준 함수로 놓는다. */
  acquire(path: string, worktree?: string): () => void;
  get(path: string): DocumentState | undefined;
  /** 잡고 있는 탭 수(시험 · 진단). */
  holders(path: string): number;
  /** 미저장 편집이 있는 문서의 경로. */
  unsaved(): string[];
  /** 저장한다(`⌘S`). 같은 문서의 저장이 이미 돌고 있으면 그것이 끝난 뒤에 다시 본다. */
  save(path: string): Promise<SaveOutcome>;
  /** 배너 [디스크 것 받기]: 미저장 편집을 버리고 디스크의 내용으로 바꾼다(undo 로 되돌릴 수 있다). */
  takeDisk(path: string): Promise<void>;
  /** 배너 [내 것 유지] · 닫기: 편집을 남기고 배너를 걷는다. 다음 저장이 지금 디스크를 덮는다(지워졌으면 다시 만든다). */
  keepMine(path: string): void;
  /** 창이 받은 `fs.changed`. 열린 문서에 닿으면 디스크와 맞춘다. 돌려준 약속은 맞추기가 끝나면 풀린다(시험). */
  changed(event: FsChangedEvent): Promise<void>;
  /**
   * 텍스트 문서 `path` 의 바뀜을 문서마다 한 번 듣는다(`DocumentChange`). 지금 텍스트 문서면 곧바로 `reset` 을 한 번 부른다.
   * 문서를 다시 읽어도 · 닫았다 다시 열어도 해제할 때까지 이어 듣는다. 해제 함수를 돌려준다.
   */
  onChange(path: string, listener: (change: DocumentChange) => void): () => void;
  readonly store: StoreApi<DocumentsState>;
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function errorCode(e: unknown): number | undefined {
  const code = (e as { code?: unknown } | null)?.code;
  return typeof code === "number" ? code : undefined;
}

/** `lang.detect` 에 넘기는 앞부분의 길이(그 RPC 가 보는 상한, crates/pw-lang). */
const DETECT_HEAD_CHARS = 50 * 1024;

async function detectLanguage(
  source: DocumentSource,
  path: string,
  text: string,
  worktree: string | undefined,
): Promise<string | null> {
  if (!source.detect) return null;
  try {
    return await source.detect(path, text.slice(0, DETECT_HEAD_CHARS), worktree);
  } catch (e) {
    console.warn("[editor] lang.detect failed, opening as plain text", path, e);
    return null;
  }
}

/** 읽은 텍스트를 `LiveText` 에 넣는 값. */
function liveInit(read: FsRead & { content: { kind: "text" } }) {
  return {
    text: read.content.text,
    lineEnding: read.judgment.lineEnding,
    bom: read.judgment.bom === "utf8",
  };
}

function isText(read: FsRead): read is FsRead & { content: { kind: "text"; text: string } } {
  return !read.judgment.binary && read.content.kind === "text";
}

/** 읽은 것에서 다시 읽을 때마다 바뀌는 문서 정보(크기 · 시각 · 판정 · 읽기 전용). */
function readFacts(read: FsRead, stat: Pick<FsStat, "readonly">) {
  return {
    judgment: read.judgment,
    version: read.version,
    size: read.size,
    mtimeMs: read.mtimeMs,
    readOnly: !read.judgment.editable || stat.readonly,
  };
}

async function loadDocument(
  source: DocumentSource,
  path: string,
  worktree: string | undefined,
): Promise<DocumentState> {
  const stat = await source.stat(path, worktree);
  const byName = viewerKindByName(stat.path);
  if (byName && VIEWER_READS[byName] === "self") return { status: "external", kind: byName, stat };
  if (stat.sizeClass === "tooLarge") return { status: "tooLarge", size: stat.size };
  if (stat.size > SNIFF_ABOVE) {
    const head = await source.read({ path: stat.path, offset: 0, length: SNIFF_BYTES }, worktree);
    if (head.judgment.binary) return { status: "external", kind: "hex", stat };
  }
  const read = await source.read({ path: stat.path }, worktree);
  const kind = viewerKindOf(read.path, read.judgment);
  if (read.judgment.binary) return { status: "external", kind, stat };
  if (!isText(read)) return { status: "notUtf8", size: read.size };
  return {
    status: "text",
    kind,
    live: new LiveText(liveInit(read)),
    ...readFacts(read, stat),
    language: await detectLanguage(source, read.path, read.content.text, worktree),
    dirty: false,
    disk: { kind: "same" },
    saving: false,
  };
}

/** `ancestor` 가 `path` 이거나 그 위 폴더다. */
function covers(ancestor: string, path: string): boolean {
  return path === ancestor || path.startsWith(ancestor.endsWith("/") ? ancestor : `${ancestor}/`);
}

export function createDocumentStore(source: DocumentSource): DocumentStore {
  const store = createStore<DocumentsState>()(() => ({ docs: {} }));
  const counts = new Map<string, number>();
  const worktrees = new Map<string, string | undefined>();
  /** 문서마다 저장 · 맞추기를 차례로 돌리는 줄. */
  const queues = new Map<string, Promise<unknown>>();
  /** `LiveText` 의 미저장 · 바뀜 구독 해제. */
  const unwatch = new Map<string, () => void>();
  const changeListeners = new Map<string, Set<(change: DocumentChange) => void>>();
  const emit = (path: string, change: DocumentChange) => {
    for (const listener of changeListeners.get(path) ?? []) listener(change);
  };

  const get = (path: string) => store.getState().docs[path];
  const text = (path: string) => {
    const doc = get(path);
    return doc?.status === "text" ? doc : undefined;
  };
  const set = (path: string, state: DocumentState | undefined) => {
    const before = get(path);
    if (before?.status === "text" && (state?.status !== "text" || state.live !== before.live)) {
      unwatch.get(path)?.();
      unwatch.delete(path);
    }
    // 새 `LiveText` 다(처음 읽음 · 통째로 다시 읽음).
    const fresh = state?.status === "text" && !unwatch.has(path) ? state.live : undefined;
    if (fresh) {
      const offDirty = fresh.onDirtyChange(() => {
        const now = text(path);
        if (now?.live === fresh && now.dirty !== fresh.dirty)
          set(path, { ...now, dirty: fresh.dirty });
      });
      const offChange = fresh.onChange((changes, before) =>
        emit(path, { kind: "edit", changes, before, after: fresh.text }),
      );
      unwatch.set(path, () => {
        offDirty();
        offChange();
      });
    }
    store.setState((s) => {
      const docs = { ...s.docs };
      if (state) docs[path] = state;
      else delete docs[path];
      return { docs };
    });
    if (fresh) emit(path, { kind: "reset", text: fresh.text });
  };
  /** 지금 텍스트 문서에 바뀐 값을 더한다. 그사이 문서가 바뀌었으면(다시 읽음 · 버림) 하지 않는다. */
  const patch = (path: string, live: LiveText, change: Partial<TextDocumentState>) => {
    const now = text(path);
    if (now?.live === live) set(path, { ...now, ...change, dirty: live.dirty });
  };

  /** 문서 하나의 일을 차례로 돌린다. 앞의 일이 실패해도 다음 일은 돈다. */
  function queue<T>(path: string, task: () => Promise<T>): Promise<T> {
    const run = (queues.get(path) ?? Promise.resolve()).then(task, task);
    const tail = run.catch(() => {});
    queues.set(path, tail);
    void tail.then(() => queues.get(path) === tail && queues.delete(path));
    return run;
  }

  /**
   * 그사이 사람이 고쳤다: `agreed`(부른 쪽이 버려도 된다고 본 텍스트 — 미저장이 없을 때의 텍스트, 또는 [디스크 것 받기] 를 누를
   * 때의 텍스트)에서 더 고친 미저장 편집이 있다.
   */
  const editedSince = (live: LiveText, agreed: Text) => live.dirty && !live.text.eq(agreed);

  /**
   * 문서를 디스크에서 통째로 다시 읽는다(종류가 바뀜 · 줄 구분이 바뀜). `guard` 가 있으면 다 읽은 뒤 그 문서에 `agreed` 밖의
   * 편집이 생겼는지 보고, 있으면 바꾸지 않고 다시 맞춘다(배너, 머리 주석).
   */
  async function reload(path: string, guard?: { live: LiveText; agreed: Text }) {
    const state = await loadDocument(source, path, worktrees.get(path)).catch(
      (e: unknown): DocumentState => ({ status: "failed", message: errorText(e) }),
    );
    if (!counts.has(path)) return;
    if (guard && text(path)?.live === guard.live && editedSince(guard.live, guard.agreed))
      return reconcile(path, []);
    set(path, state);
  }

  /**
   * 디스크에서 읽은 텍스트를 문서에 넣는다. `agreed` 까지의 미저장 편집은 사라진다(undo 로 되돌릴 수 있다 — 통째로 다시 읽을
   * 때는 못 한다). 그 뒤에 고친 것이 있으면 넣지 않고 다시 맞춘다.
   */
  async function adopt(
    path: string,
    doc: TextDocumentState,
    read: FsRead,
    stat: FsStat,
    agreed: Text,
  ): Promise<void> {
    if (editedSince(doc.live, agreed)) return reconcile(path, []);
    if (isText(read) && doc.live.replace(liveInit(read)))
      patch(path, doc.live, { ...readFacts(read, stat), disk: { kind: "same" } });
    else await reload(path, { live: doc.live, agreed });
  }

  /** 지워진 문서와 같은 내용(`version`)인 새 파일. `candidates` 는 같은 이벤트에서 생기거나 바뀐, 열린 문서가 아닌 경로. */
  async function renamedTo(
    path: string,
    doc: TextDocumentState,
    candidates: readonly string[],
  ): Promise<string | undefined> {
    const worktree = worktrees.get(path);
    for (const candidate of candidates.slice(0, RENAME_CANDIDATES)) {
      try {
        const stat = await source.stat(candidate, worktree);
        if (stat.kind !== "file" || stat.size !== doc.size) continue;
        const read = await source.read({ path: stat.path }, worktree);
        if (read.version !== null && read.version === doc.version) return stat.path;
      } catch {
        // 그사이 또 바뀐 파일은 후보가 아니다.
      }
    }
    return undefined;
  }

  /** 열린 문서 하나를 디스크와 맞춘다. `candidates` 는 이름 바뀜 찾기의 후보(`renamedTo`). */
  async function reconcile(path: string, candidates: readonly string[]): Promise<void> {
    const doc = get(path);
    if (!doc || doc.status === "loading" || !counts.has(path)) return;
    if (doc.status !== "text") return reload(path);
    const worktree = worktrees.get(path);
    let stat: FsStat;
    try {
      stat = await source.stat(path, worktree);
    } catch (e) {
      if (errorCode(e) !== ERROR_CODE.NOT_FOUND) {
        console.warn("[editor] could not check a changed file", path, e);
        return;
      }
      if (doc.disk.kind === "deleted" || doc.disk.kind === "renamed") return;
      const to = await renamedTo(path, doc, candidates);
      patch(path, doc.live, { disk: to ? { kind: "renamed", to } : { kind: "deleted" } });
      return;
    }
    // 마지막으로 본 것과 크기 · 시각이 같으면 그대로다(자기 저장이 돌아온 이벤트가 여기서 걸린다).
    if (doc.disk.kind === "same" && stat.size === doc.size && stat.mtimeMs === doc.mtimeMs) return;
    const read = await source.read({ path }, worktree);
    const now = text(path);
    if (now?.live !== doc.live) return;
    if (read.version === now.version) {
      if (now.disk.kind !== "same") patch(path, now.live, { disk: { kind: "same" } });
      return;
    }
    // 시각만 바뀌었다 — 아는 `version` 만 바꾼다(다음 저장의 기대 값).
    if (isText(read) && now.live.holds(liveInit(read))) {
      patch(path, now.live, { ...readFacts(read, stat), disk: { kind: "same" } });
      return;
    }
    if (!now.live.dirty) return adopt(path, now, read, stat, now.live.text);
    patch(path, now.live, { disk: { kind: "changed", read } });
  }

  async function save(path: string): Promise<SaveOutcome> {
    const doc = text(path);
    if (!doc || doc.readOnly || !source.write) return { kind: "unchanged" };
    if (!doc.live.dirty && doc.disk.kind === "same") return { kind: "unchanged" };
    const { live } = doc;
    const snapshot = live.text;
    // 지워진 파일은 새로 만든다 — 기대 `version` 이 없으면 `fs.write` 는 파일이 없을 때만 쓴다.
    const expectedVersion =
      doc.disk.kind === "deleted" || doc.disk.kind === "renamed" ? null : doc.version;
    patch(path, live, { saving: true });
    try {
      const written = await source.write(
        {
          path,
          content: { kind: "text", text: live.serialize(snapshot) },
          bom: live.bom,
          expectedVersion,
        },
        worktrees.get(path),
      );
      live.markClean(snapshot);
      patch(path, live, {
        saving: false,
        version: written.version,
        size: written.size,
        mtimeMs: written.mtimeMs,
        disk: { kind: "same" },
      });
      return { kind: "saved" };
    } catch (e) {
      if (errorCode(e) !== ERROR_CODE.CONFLICT) {
        patch(path, live, { saving: false });
        return { kind: "failed", message: errorText(e) };
      }
      let disk: DiskState;
      try {
        disk = { kind: "changed", read: await source.read({ path }, worktrees.get(path)) };
      } catch (readError) {
        if (errorCode(readError) !== ERROR_CODE.NOT_FOUND) {
          patch(path, live, { saving: false });
          return { kind: "failed", message: errorText(readError) };
        }
        disk = { kind: "deleted" };
      }
      patch(path, live, { saving: false, disk });
      return { kind: "conflict" };
    }
  }

  return {
    store,
    get,
    holders: (path) => counts.get(path) ?? 0,
    unsaved: () =>
      Object.entries(store.getState().docs)
        .filter(([, doc]) => doc.status === "text" && doc.live.dirty)
        .map(([path]) => path),
    acquire(path, worktree) {
      const count = counts.get(path) ?? 0;
      counts.set(path, count + 1);
      if (count === 0) {
        worktrees.set(path, worktree);
        set(path, { status: "loading" });
        loadDocument(source, path, worktree)
          .catch((e: unknown): DocumentState => ({ status: "failed", message: errorText(e) }))
          // 다 읽기 전에 모두 놓았으면 버린다. 그사이 다시 잡았으면 그 읽기가 이것이다(같은 경로 · 같은 디스크).
          .then((state) => counts.has(path) && set(path, state));
      }
      let released = false;
      return () => {
        if (released) return;
        released = true;
        const left = (counts.get(path) ?? 1) - 1;
        if (left > 0) {
          counts.set(path, left);
          return;
        }
        counts.delete(path);
        worktrees.delete(path);
        set(path, undefined);
      };
    },
    save: (path) => queue(path, () => save(path)),
    takeDisk: (path) =>
      queue(path, async () => {
        const doc = text(path);
        if (!doc) return;
        // 누른 때의 텍스트까지만 버린다 — 디스크를 읽는 동안 친 글자는 남긴다.
        const agreed = doc.live.text;
        const worktree = worktrees.get(path);
        const stat = await source.stat(path, worktree);
        const read =
          doc.disk.kind === "changed" ? doc.disk.read : await source.read({ path }, worktree);
        const now = text(path);
        if (now) await adopt(path, now, read, stat, now.live === doc.live ? agreed : now.live.text);
      }),
    keepMine(path) {
      const doc = text(path);
      if (!doc || doc.disk.kind === "same") return;
      const version = doc.disk.kind === "changed" ? doc.disk.read.version : null;
      patch(path, doc.live, {
        disk: { kind: "same" },
        version,
        ...(doc.disk.kind === "changed" && {
          size: doc.disk.read.size,
          mtimeMs: doc.disk.read.mtimeMs,
        }),
      });
    },
    onChange(path, listener) {
      let listeners = changeListeners.get(path);
      if (!listeners) changeListeners.set(path, (listeners = new Set()));
      listeners.add(listener);
      const now = text(path);
      if (now) listener({ kind: "reset", text: now.live.text });
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0 && changeListeners.get(path) === listeners)
          changeListeners.delete(path);
      };
    },
    async changed(event) {
      const candidates = event.paths.filter(
        (p, i) => event.kinds[i] !== "removed" && !counts.has(p),
      );
      const open = [...counts.keys()].filter((path) =>
        event.rescan ? covers(event.root, path) : event.paths.some((p) => covers(p, path)),
      );
      await Promise.all(
        open.map((path) =>
          queue(path, () => reconcile(path, candidates)).catch((e: unknown) =>
            console.warn("[editor] could not reconcile a changed file", path, e),
          ),
        ),
      );
    },
  };
}

/** 이 컴포넌트가 마운트돼 있는 동안 문서를 잡고, 지금 상태를 돌려준다. */
export function useDocument(
  documents: DocumentStore,
  path: string,
  worktree?: string,
): DocumentState {
  useEffect(() => documents.acquire(path, worktree), [documents, path, worktree]);
  return useStore(documents.store, (s) => s.docs[path]) ?? { status: "loading" };
}
