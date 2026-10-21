// 문서 언어 상태: 문서마다 판별한 언어(`lang.detect`, documents.ts 의 `language`)와 사람이 고른 언어(M1-18 상태바의 언어 수동
// 변경, F4-12). 쓰는 언어 = 고른 것 ?? 판별값 ?? `PLAIN_TEXT` 이고 늘 Linguist 이름 문자열이다. 플레인 텍스트도 `null` 이 아니라
// `PLAIN_TEXT` 다 — `setDocLanguageOverride` 의 `null` 은 「판별값으로 되돌림」 이라 「사람이 Plain Text 를 고름」 과 갈라야 한다.
//
// 문서 id 는 문서 모델의 키(정규화한 실제 경로)다. 고른 값은 문서 단위다 — 같은 파일을 보이는 탭이 모두 따르고, 그 문서를 보는
// 마지막 탭을 닫아 문서가 사라지면 잊는다(다시 열면 판별값). 판별값은 문서 모델에서 받는다(`bindDocumentLanguages`) — 창의
// 에디터 작업 공간이 문서 모델을 붙일 때 묶는다(editor-workspace.tsx `attach`).
//
// 창(웹뷰) 하나에 상태 하나다. 모듈 함수(`getDocLanguage` 등)는 그 기본 상태를 쓰고, 시험은 `createDocLanguages` 로 따로 만든다.

import { useCallback, useSyncExternalStore } from "react";

import type { DocumentStore } from "./documents";

/** 플레인 텍스트의 언어 이름: 언어 판별(crates/pw-lang)이 마지막 단계에서 내는 Linguist 이름. 문법이 없어 강조하지 않는다. */
export const PLAIN_TEXT = "Text";

type Listener = (language: string) => void;

export interface DocLanguages {
  /** 쓰는 언어: 고른 것 ?? 판별값 ?? `PLAIN_TEXT`. 모르는 문서는 `PLAIN_TEXT`. */
  get(docId: string): string;
  /** 사람이 고른 언어(`PLAIN_TEXT` 도 고른 것이다). `null` 이면 고른 것을 지워 판별값으로 돌아간다. */
  setOverride(docId: string, language: string | null): void;
  /** 쓰는 언어가 바뀔 때 새 값으로 부른다. 해제 함수를 돌려준다. */
  onChange(docId: string, listener: Listener): () => void;
  /** 문서 모델의 판별값을 따르고, 문서가 사라지면 고른 값을 잊는다. 해제 함수를 돌려준다. */
  bind(documents: DocumentStore): () => void;
}

export function createDocLanguages(): DocLanguages {
  const detected = new Map<string, string | null>();
  const overrides = new Map<string, string>();
  const listeners = new Map<string, Set<Listener>>();

  const get = (docId: string) => overrides.get(docId) ?? detected.get(docId) ?? PLAIN_TEXT;

  /** `change` 를 하고, 쓰는 언어가 바뀌었으면 알린다. */
  const update = (docId: string, change: () => void) => {
    const before = get(docId);
    change();
    const after = get(docId);
    if (after === before) return;
    for (const listener of listeners.get(docId) ?? []) listener(after);
  };

  return {
    get,
    setOverride(docId, language) {
      update(docId, () => {
        if (language === null) overrides.delete(docId);
        else overrides.set(docId, language);
      });
    },
    onChange(docId, listener) {
      let set = listeners.get(docId);
      if (!set) listeners.set(docId, (set = new Set()));
      set.add(listener);
      return () => {
        set.delete(listener);
        if (set.size === 0 && listeners.get(docId) === set) listeners.delete(docId);
      };
    },
    bind(documents) {
      // 이 문서 모델에 있던 문서. 사라졌는지는 이것만 본다.
      const seen = new Set<string>();
      const sync = (docs: ReturnType<typeof documents.store.getState>["docs"]) => {
        for (const [docId, doc] of Object.entries(docs)) {
          seen.add(docId);
          if (doc.status === "text" && detected.get(docId) !== doc.language)
            update(docId, () => detected.set(docId, doc.language));
        }
        // 사라진 문서: 판별값과 고른 값을 함께 잊는다. 읽는 중(`loading`)인 문서는 아직 있다.
        for (const docId of seen)
          if (!(docId in docs)) {
            seen.delete(docId);
            update(docId, () => {
              detected.delete(docId);
              overrides.delete(docId);
            });
          }
      };
      sync(documents.store.getState().docs);
      return documents.store.subscribe((s) => sync(s.docs));
    },
  };
}

const languages = createDocLanguages();

/** 문서의 쓰는 언어(Linguist 이름 — 플레인 텍스트는 `PLAIN_TEXT`). */
export const getDocLanguage = (docId: string) => languages.get(docId);
/** 사람이 고른 언어를 둔다. `null` 은 판별값으로 돌아간다(`PLAIN_TEXT` 는 Plain Text 를 고른 것). */
export const setDocLanguageOverride = (docId: string, language: string | null) =>
  languages.setOverride(docId, language);
/** 문서의 쓰는 언어가 바뀔 때 부른다. 해제 함수를 돌려준다. */
export const onDocLanguageChange = (docId: string, listener: Listener) =>
  languages.onChange(docId, listener);
/** 창의 문서 모델을 기본 상태에 묶는다. 해제 함수를 돌려준다. */
export const bindDocumentLanguages = (documents: DocumentStore) => languages.bind(documents);

/** 컴포넌트가 문서의 쓰는 언어를 구독한다. */
export function useDocLanguage(docId: string, state: DocLanguages = languages): string {
  const subscribe = useCallback(
    (onStoreChange: () => void) => state.onChange(docId, onStoreChange),
    [state, docId],
  );
  return useSyncExternalStore(subscribe, () => state.get(docId));
}
