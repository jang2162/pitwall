// 문서 모델(12-m1-plan M1-09). 열린 파일 하나 = 문서 하나이고 키는 정규화한 실제 경로(`fs.stat` · `fs.read` 의 `path`)다. 여러
// 탭(그룹)이 같은 파일을 보이면 문서는 하나이고, 탭마다 따로인 것은 뷰(커서 · 스크롤)뿐이다. 탭이 문서를 쓰는 동안 `acquire` 로
// 잡고, 마지막 탭이 놓으면 문서를 버린다 — 다음에 열면 디스크에서 다시 읽는다.
//
// 읽기 순서: `fs.stat` 으로 크기 등급(결정 M1-D9)을 먼저 본다. 뷰어 종류(viewer-kinds.ts)가 바이트를 스스로 읽는 뷰어(이미지)면
// 고르면 여기서 내용을 읽지 않는다. 50 MiB 를 넘으면 열지 않는다(`tooLarge`). 나머지는 앞부분으로 바이너리인지 보고(큰 파일을
// 통째로 base64 로 받지 않게), 텍스트면 전체를 읽는다. UTF-8 로 읽히지 않는 텍스트는 `notUtf8` 이다 — 인코딩 짐작 · 읽기 전용
// 표시는 M1-23 몫이다(결정 M1-D12). 텍스트로 읽은 문서는 앞부분으로 언어를 정한다(`lang.detect`, M1-02) — 못 정하면
// 플레인 텍스트(`null`)로 열고 오류로 보지 않는다.
//
// 접근 범위: 문서는 처음 잡은 탭의 워크트리(`EditorTabParams.worktree`)로 읽는다. 창의 활성 워크트리는 열기 요청마다 바뀌므로
// 읽는 순간의 값을 쓰면 다른 워크트리의 요청이 뒤섞인다.

import { Text } from "@codemirror/state";
import type { FsRead, FsReadParams, FsStat, Judgment } from "@pitwall/rpc-client";
import { useEffect } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";

import { VIEWER_READS, type ViewerKind, viewerKindByName, viewerKindOf } from "./viewer-kinds";

/** 바이너리 판정에 앞부분만 먼저 읽는 크기 경계. 이보다 작은 파일은 한 번에 읽는다. */
export const SNIFF_ABOVE = 64 * 1024;
/** 앞부분 읽기의 길이(crates/pw-fs judge.rs 의 `SNIFF_BYTES`). */
const SNIFF_BYTES = 8 * 1024;

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
      text: Text;
      judgment: Judgment;
      /** `fs.write` 의 `expectedVersion`(M1-13). */
      version: string | null;
      size: number;
      /** `lang.detect` 가 정한 Linguist 언어 이름. `null` 이면 플레인 텍스트. 탭이 덮어쓸 수 있다(`EditorTabParams.language`). */
      language: string | null;
      /** 이 앱이 고칠 수 없다: 크기 등급 · 인코딩 · 쓰기 권한. 지금 뷰어는 늘 읽기 전용이다(편집은 M1-13). */
      readOnly: boolean;
    };

/**
 * 문서를 읽는 길. 창은 Host RPC(`fs.stat` · `fs.read` · `lang.detect`)를 넘기고 시험은 가짜를 넘긴다. `worktree` 는 접근 범위다
 * (없으면 메인 체크아웃).
 */
export interface DocumentSource {
  stat(path: string, worktree: string | undefined): Promise<FsStat>;
  read(
    params: Pick<FsReadParams, "path" | "offset" | "length">,
    worktree: string | undefined,
  ): Promise<FsRead>;
  /** 언어. 없으면 늘 플레인 텍스트다. */
  detect?(path: string, head: string, worktree: string | undefined): Promise<string | null>;
}

interface DocumentsState {
  docs: Readonly<Record<string, DocumentState>>;
}

export interface DocumentStore {
  /** 문서를 쓰기 시작한다. 처음 잡는 것이면 `worktree` 를 접근 범위로 읽기 시작한다. 돌려준 함수로 놓는다. */
  acquire(path: string, worktree?: string): () => void;
  get(path: string): DocumentState | undefined;
  /** 잡고 있는 탭 수(시험 · 진단). */
  holders(path: string): number;
  readonly store: StoreApi<DocumentsState>;
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** `fs.read` 의 텍스트를 줄로 나눈다. CodeMirror 와 같은 줄바꿈(`\r\n` · `\r` · `\n`)으로 자른다. */
export function toText(text: string): Text {
  return Text.of(text.split(/\r\n?|\n/));
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
  if (read.content.kind !== "text") return { status: "notUtf8", size: read.size };
  return {
    status: "text",
    kind,
    text: toText(read.content.text),
    judgment: read.judgment,
    version: read.version,
    size: read.size,
    language: await detectLanguage(source, read.path, read.content.text, worktree),
    readOnly: !read.judgment.editable || stat.readonly,
  };
}

export function createDocumentStore(source: DocumentSource): DocumentStore {
  const store = createStore<DocumentsState>()(() => ({ docs: {} }));
  const counts = new Map<string, number>();
  const set = (path: string, state: DocumentState | undefined) =>
    store.setState((s) => {
      const docs = { ...s.docs };
      if (state) docs[path] = state;
      else delete docs[path];
      return { docs };
    });

  return {
    store,
    get: (path) => store.getState().docs[path],
    holders: (path) => counts.get(path) ?? 0,
    acquire(path, worktree) {
      const count = counts.get(path) ?? 0;
      counts.set(path, count + 1);
      if (count === 0) {
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
        set(path, undefined);
      };
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
