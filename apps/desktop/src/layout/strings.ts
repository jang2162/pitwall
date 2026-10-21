// 프로젝트 창(툴 윈도우 · 에디터 탭 · 상태바 · 백그라운드 작업 · 런타임 환경 · 신뢰 · 알림 · 파일 감시)이 보여 주는 문자열. 내용은 Fluent 리소스
// (extensions/i18n-default/i18n/*/project.ftl)에 있고, 이 파일은 화면 자리와 메시지 id 를 잇는다. 여기 적은 id 가 ko · en
// 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다 — 툴 윈도우 이름은 PANELS 의 모든 id 를 거기서 부른다.

import { useLocalization } from "@fluent/react";
import type { Platform } from "@pitwall/keymap";
import type { RuntimeProblem } from "@pitwall/rpc-client";
import { useMemo } from "react";

import type { ViewerKind } from "../editor/viewer-kinds";
import { type Translate, translator } from "../i18n/localization";
import { PANELS } from "./project-layout";

/** 툴 윈도우 id(`queryResults`) → 이름 메시지 id(`tool-window-query-results`). */
function toolWindowMessage(id: string): string {
  return `tool-window-${id.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}`;
}

export function projectStrings(t: Translate) {
  return {
    /** 툴 윈도우 id → 이름. 탭 머리 · 툴 윈도우 바 · 열기 액션이 쓴다. */
    toolWindows: Object.fromEntries(
      PANELS.map((p) => [p.id, t(toolWindowMessage(p.id))]),
    ) as Record<string, string>,
    toolWindowAction: (name: string) => t("action-tool-window", { name }),
    hideAllToolWindows: t("action-tool-window-hide-all"),
    toolWindowBar: {
      left: t("tool-window-bar-left"),
      right: t("tool-window-bar-right"),
    },
    placeholder: (name: string) => t("tool-window-placeholder", { name }),
    editor: {
      /** 가운데에 탭이 하나도 없을 때(빈 영역 표시). */
      empty: t("editor-empty"),
      loading: t("editor-loading"),
      failed: (message: string) => t("editor-read-failed", { message }),
      /** `size`: 「12.3 MiB」 처럼 단위까지 붙인 크기. */
      tooLarge: (size: string) => t("editor-too-large", { size }),
      notUtf8: t("editor-not-utf8"),
      readOnlyLarge: t("editor-read-only-large"),
      noViewer: (kind: ViewerKind) => t("editor-no-viewer", { kind }),
      openFailed: (path: string) => t("editor-open-failed", { path }),
      copyFailed: t("editor-copy-path-failed"),
      pinned: t("editor-tab-pinned"),
      menu: { pin: t("editor-tab-pin"), unpin: t("editor-tab-unpin") },
      switcher: t("editor-switcher-label"),
      actions: {
        closeTab: t("action-editor-close-tab"),
        closeTabOrWindow: t("action-editor-close-tab-or-window"),
        closeOtherTabs: t("action-editor-close-other-tabs"),
        splitRight: t("action-editor-split-right"),
        togglePin: t("action-editor-toggle-pin"),
        toggleMaximized: t("action-editor-toggle-maximized"),
        copyPath: t("action-editor-copy-path"),
        reveal: (platform: Platform) => t("action-editor-reveal", { platform }),
        switcher: t("action-editor-switcher"),
      },
    },
    status: {
      label: t("status-bar-label"),
      connecting: t("status-host-connecting"),
      hostFailed: t("status-host-failed"),
      appearance: (appearance: "light" | "dark") => t("status-appearance", { appearance }),
      notifications: (count: number) => t("status-notifications", { count }),
    },
    tasks: {
      label: t("task-list-label"),
      cancel: (title: string) => t("task-cancel", { title }),
      more: (count: number) => t("task-more", { count }),
      failed: (title: string) => t("task-failed", { title }),
      startFailed: t("task-start-failed"),
      cancelFailed: t("task-cancel-failed"),
      demo: t("action-task-demo"),
      demoInvalid: t("action-task-demo-invalid"),
    },
    runtime: {
      reload: t("action-runtime-reload"),
      reloadFailed: t("runtime-reload-failed"),
      mise: t("status-runtime-mise"),
      miseHint: (files: string) => t("status-runtime-mise-hint", { files }),
      fallback: (problem: RuntimeProblem) => t("status-runtime-fallback", { problem }),
      fallbackHint: (problem: RuntimeProblem, reason: string) =>
        t("status-runtime-fallback-hint", { problem, reason }),
    },
    trust: {
      status: t("status-trust-needed"),
      statusHint: t("status-trust-needed-hint"),
      unsavable: t("status-trust-unsavable"),
      unsavableHint: t("status-trust-unsavable-hint"),
      title: (name: string) => t("trust-dialog-title", { name }),
      body: t("trust-dialog-body"),
      trust: t("trust-dialog-trust"),
      distrust: t("trust-dialog-distrust"),
      trustAction: t("action-project-trust"),
      untrustAction: t("action-project-untrust"),
      saveFailed: t("trust-save-failed"),
    },
    notifications: {
      empty: t("notifications-empty"),
      clear: t("notifications-clear"),
      level: (level: "info" | "warning" | "error") => t("notification-level", { level }),
      /** 같은 제목의 알림이 떠 있는 토스트에 합쳐졌을 때의 토스트 제목. */
      repeated: (title: string, count: number) => t("notification-repeated", { title, count }),
    },
    fileWatch: {
      polling: t("status-file-watch-polling"),
      pollingHint: (roots: string) => t("status-file-watch-polling-hint", { roots }),
    },
  };
}

export type ProjectStrings = ReturnType<typeof projectStrings>;

/** 지금 언어의 프로젝트 창 문자열. 언어가 바뀌면 새 객체다. */
export function useProjectStrings(): ProjectStrings {
  const { l10n } = useLocalization();
  return useMemo(() => projectStrings(translator(l10n)), [l10n]);
}
