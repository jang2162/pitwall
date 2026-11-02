// 시험용 서버 관리자(웹뷰 쪽에서 본 `lsp.document.*` · `lsp.document.capabilities`): crates/pw-lsp 의 문서 규칙(methods.rs)을
// 흉내 낸다 — 편집마다 버전(앞 버전 + 1 이 아니면 conflict), 모르는 문서는 not found, 열 때 `off`(서버 정의 없음)를 고를 수 있다.
// 호출은 모두 기록한다(`called`). 능력 칸은 시험이 정한다(`features`).

import { ChangeSet, Text } from "@codemirror/state";
import {
  ERROR_CODE,
  type LspDocumentCapabilities,
  type LspDocumentChangeParams,
  type LspDocumentOff,
  type LspDocumentOpenParams,
  RpcCallError,
} from "@pitwall/rpc-client";
import { createStore } from "zustand/vanilla";

import type { DocumentChange, DocumentState } from "../editor/documents";
import type { LspSource } from "./lsp-sessions";

interface Doc {
  path: string;
  text: string;
  version: number;
}

export interface FakeLspOptions {
  /** 언어 → 열 때의 `off`. 없으면 연다. */
  off?: (language: string) => LspDocumentOff | null;
  /** 열기를 거절하는 오류 코드(루트 밖 등). */
  refuse?: number;
}

export function fakeLspServer(options: FakeLspOptions = {}) {
  const calls: { method: string; params: unknown }[] = [];
  const docs = new Map<string, Doc>();
  let seq = 0;
  let features: LspDocumentCapabilities["features"] = {};
  const notFound = (docId: string) =>
    new RpcCallError({ code: ERROR_CODE.NOT_FOUND, message: `no open document ${docId}` });
  const source: LspSource = {
    async open(params: LspDocumentOpenParams) {
      calls.push({ method: "open", params });
      if (options.refuse !== undefined)
        throw new RpcCallError({ code: options.refuse, message: "refused" });
      const off = options.off?.(params.language) ?? null;
      if (off) return { docId: "", off, instances: [] };
      const docId = `d${++seq}`;
      docs.set(docId, { path: params.path, text: params.text, version: params.version });
      return { docId, off: null, instances: [] };
    },
    async change(params: LspDocumentChangeParams) {
      calls.push({ method: "change", params });
      const doc = docs.get(params.docId);
      if (!doc) throw notFound(params.docId);
      if (params.version !== doc.version + 1)
        throw new RpcCallError({
          code: ERROR_CODE.CONFLICT,
          message: "version",
          data: { version: doc.version },
        });
      for (const { from, to, insert } of params.changes)
        doc.text = doc.text.slice(0, from) + insert + doc.text.slice(to);
      doc.version = params.version;
      return { off: null };
    },
    async save(params) {
      calls.push({ method: "save", params });
      if (!docs.has(params.docId)) throw notFound(params.docId);
      return null;
    },
    async close(params) {
      calls.push({ method: "close", params });
      docs.delete(params.docId);
      return null;
    },
    async capabilities(params) {
      calls.push({ method: "capabilities", params });
      if (!docs.has(params.docId)) throw notFound(params.docId);
      return { features };
    },
  };
  return {
    source,
    docs,
    called: (method: string) => calls.filter((c) => c.method === method).map((c) => c.params),
    setFeatures(next: LspDocumentCapabilities["features"]) {
      features = next;
    },
  };
}

/** 문서 모델의 바뀜 구독 · 저장 상태만 흉내 낸다. */
export function fakeLspDocuments(initial: Record<string, string>) {
  const texts = new Map(Object.entries(initial).map(([p, t]) => [p, Text.of(t.split("\n"))]));
  const listeners = new Map<string, Set<(change: DocumentChange) => void>>();
  const textState = (path: string, extra: { saving: boolean; version: string }) =>
    ({
      status: "text",
      ...extra,
      judgment: {},
    }) as unknown as DocumentState & { path?: string };
  const store = createStore<{ docs: Readonly<Record<string, DocumentState>> }>(() => ({
    docs: Object.fromEntries(
      [...texts.keys()].map((p) => [p, textState(p, { saving: false, version: "v0" })]),
    ),
  }));
  return {
    store,
    onChange(path: string, listener: (change: DocumentChange) => void) {
      let set = listeners.get(path);
      if (!set) listeners.set(path, (set = new Set()));
      set.add(listener);
      const text = texts.get(path);
      if (text) listener({ kind: "reset", text });
      return () => void set.delete(listener);
    },
    edit(path: string, from: number, to: number, insert: string) {
      const before = texts.get(path)!;
      const after = before.replace(from, to, Text.of(insert.split("\n")));
      texts.set(path, after);
      const changes = ChangeSet.of({ from, to, insert }, before.length);
      for (const l of listeners.get(path) ?? []) l({ kind: "edit", changes, before, after });
    },
    text: (path: string) => texts.get(path)!.toString(),
    /** `fs.write` 를 흉내 낸다: 쓰는 중 → 새 디스크 `version`. */
    save(path: string, version: string) {
      store.setState(({ docs }) => ({
        docs: { ...docs, [path]: textState(path, { saving: true, version: "v0" }) },
      }));
      store.setState(({ docs }) => ({
        docs: { ...docs, [path]: textState(path, { saving: false, version }) },
      }));
    },
  };
}
