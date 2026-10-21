// 단일 파일 창(file-window.tsx)의 문자열. 내용은 Fluent 리소스(extensions/i18n-default/i18n/*/file-window.ftl)에 있다. 에디터 자체의
// 문자열은 프로젝트 창과 같은 것(layout/strings.ts 의 `editor`)을 쓴다. 여기 적은 id 가 ko · en 리소스에 모두 있는지는
// i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";

export function fileWindowStrings(t: Translate) {
  return {
    openAsProject: t("file-window-open-as-project"),
    openAsProjectFailed: t("file-window-open-as-project-failed"),
    statusLabel: t("file-window-status-label"),
    scopeHint: t("file-window-scope-hint"),
  };
}

export type FileWindowStrings = ReturnType<typeof fileWindowStrings>;

/** 지금 언어의 단일 파일 창 문자열. 언어가 바뀌면 새 객체다. */
export function useFileWindowStrings(): FileWindowStrings {
  const { l10n } = useLocalization();
  return useMemo(() => fileWindowStrings(translator(l10n)), [l10n]);
}
