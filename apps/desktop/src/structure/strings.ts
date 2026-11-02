// 구조 툴 윈도우 · 파일 구조 팝업(⌘F12)이 보여 주는 문자열(15-m4-plan M4-10). 내용은 Fluent 리소스
// (extensions/i18n-default/i18n/*/project.ftl 의 「구조 툴 윈도우」 절)에 있다. 여기 적은 id 가 ko · en 리소스에 모두 있는지는
// src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";
import type { OutlineKind } from "./outline";
import type { OutlineStatus } from "./outline-service";

const KINDS: readonly OutlineKind[] = [
  "module",
  "class",
  "method",
  "property",
  "field",
  "constructor",
  "enum",
  "interface",
  "function",
  "variable",
  "constant",
  "value",
  "enumMember",
  "struct",
  "event",
  "operator",
  "typeParameter",
  "trait",
  "typeAlias",
  "macro",
  "impl",
  "symbol",
];

export function structureStrings(t: Translate) {
  return {
    sourceLsp: (server: string) => t("structure-source-lsp", { server }),
    sourceIndex: t("structure-source-index"),
    sourceLabel: t("structure-source-label"),
    tree: t("structure-tree"),
    followCursor: t("structure-follow-cursor"),
    sortByName: t("structure-sort-name"),
    emptyNone: t("structure-empty-none"),
    emptyLoading: t("structure-empty-loading"),
    emptySymbols: t("structure-empty-symbols"),
    failed: t("structure-failed"),
    truncated: t("structure-truncated"),
    tooLarge: t("structure-too-large"),
    fileStructure: t("structure-action-file-structure"),
    popupPlaceholder: t("structure-popup-placeholder"),
    popupEmpty: t("structure-popup-empty"),
    popupMore: (count: number) => t("structure-popup-more", { count }),
    row: (name: string, kind: string) => t("structure-row", { name, kind }),
    kinds: Object.fromEntries(
      KINDS.map((k) => [
        k,
        t(`structure-kind-${k.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`),
      ]),
    ) as Record<OutlineKind, string>,
  };
}

export type StructureStrings = ReturnType<typeof structureStrings>;

export function useStructureStrings(): StructureStrings {
  const { l10n } = useLocalization();
  return useMemo(() => structureStrings(translator(l10n)), [l10n]);
}

/** 트리가 없을 때(또는 비었을 때) 그 자리에 적는 문구. */
export function emptyText(T: StructureStrings, status: OutlineStatus): string {
  switch (status) {
    case "none":
      return T.emptyNone;
    case "loading":
      return T.emptyLoading;
    case "failed":
      return T.failed;
    case "tooLarge":
      return T.tooLarge;
    case "ready":
      return T.emptySymbols;
  }
}
