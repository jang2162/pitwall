// 에디터 거터의 git 변경 표시 · 인라인 diff 팝업 · blame(13-m2-plan M2-17)이 보여 주는 문자열. 내용은 Fluent 리소스
// extensions/i18n-default/i18n/*/git-gutter.ftl 에 있다. 여기 적은 id 가 ko · en 리소스에 모두 있는지는 src/i18n/resources.test.ts 가
// 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../../i18n/localization";

export function gitGutterStrings(t: Translate) {
  return {
    actions: {
      nextChange: t("git-gutter-action-next-change"),
      previousChange: t("git-gutter-action-previous-change"),
      toggleBlame: t("git-gutter-action-toggle-blame"),
      blameRevision: t("git-gutter-action-blame-revision"),
    },
    marker: {
      added: t("git-gutter-marker-added"),
      modified: t("git-gutter-marker-modified"),
      deleted: t("git-gutter-marker-deleted"),
    },
    popup: {
      label: t("git-gutter-popup-label"),
      revert: t("git-gutter-popup-revert"),
      stage: t("git-gutter-popup-stage"),
      previous: t("git-gutter-popup-previous"),
      next: t("git-gutter-popup-next"),
      close: t("git-gutter-popup-close"),
      position: (index: number, count: number) => t("git-gutter-popup-position", { index, count }),
      stageDirty: t("git-gutter-stage-dirty"),
      readOnly: t("git-gutter-stage-read-only"),
      stagedAlready: t("git-gutter-staged-already"),
      stageInexact: t("git-gutter-stage-inexact"),
      diskChanged: t("git-gutter-stage-disk-changed"),
      stageFailed: (message: string) => t("git-gutter-stage-failed", { message }),
    },
    off: {
      filter: (driver: string) => t("git-gutter-off-filter", { driver }),
      dismiss: t("git-gutter-off-dismiss"),
    },
    blame: {
      uncommitted: t("git-blame-uncommitted"),
      inline: (author: string, when: string, subject: string) =>
        t("git-blame-inline", { author, when, subject }),
      tooltipAuthor: (author: string, email: string, date: string) =>
        t("git-blame-tooltip-author", { author, email, date }),
      boundary: t("git-blame-boundary"),
      click: t("git-blame-click"),
      noMoves: t("git-blame-no-moves"),
      loading: t("git-blame-loading"),
      failed: (message: string) => t("git-blame-failed", { message }),
      menu: {
        previous: t("git-blame-menu-previous"),
        close: t("git-blame-menu-close"),
        history: t("git-blame-menu-history"),
      },
    },
  };
}

export type GitGutterStrings = ReturnType<typeof gitGutterStrings>;

/** 지금 언어의 거터 · blame 문자열. 언어가 바뀌면 새 객체다. */
export function useGitGutterStrings(): GitGutterStrings {
  const { l10n } = useLocalization();
  return useMemo(() => gitGutterStrings(translator(l10n)), [l10n]);
}
