// 언어 서버 상태 항목 · 상태 팝업 · 탭 위 배너 · 도구 관리(설정 창 「도구」) · 라이선스 · 출처 확인 다이얼로그가 보이는 문자열
// (15-m4-plan M4-17). 내용은 extensions/i18n-default/i18n/*/app.ftl 의 끝 묶음(`lsp-…` · `tools-…`)이다 — 프로젝트 창과 설정 창이 함께 쓰므로 창 문자열
// (layout/strings.ts · settings/strings.ts)에 넣지 않고 여기 둔다. id 가 두 언어에 모두 있는지는 src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";

export function toolsStrings(t: Translate) {
  return {
    status: {
      starting: t("lsp-status-starting"),
      ready: t("lsp-status-ready"),
      progress: (title: string, percent: number | null) =>
        percent === null
          ? t("lsp-status-progress-unknown", { title })
          : t("lsp-status-progress", { title, percent }),
      error: t("lsp-status-error"),
      crashed: t("lsp-status-crashed"),
      restartNeeded: t("lsp-status-restart-needed"),
      restartSuggested: t("lsp-status-restart-suggested"),
      notInstalled: t("lsp-status-not-installed"),
      runtimeMissing: t("lsp-status-runtime-missing"),
      untrusted: t("lsp-status-untrusted"),
      disabled: t("lsp-status-disabled"),
      stopped: t("lsp-status-stopped"),
      more: (count: number) => t("lsp-status-more", { count }),
      hint: (server: string) => t("lsp-status-hint", { server }),
    },
    popup: {
      title: t("lsp-status-popup-title"),
      root: t("lsp-status-root"),
      version: t("lsp-status-version"),
      found: t("lsp-status-found"),
      foundIn: {
        settings: t("lsp-status-found-settings"),
        projectLocal: t("lsp-status-found-project-local"),
        path: t("lsp-status-found-path"),
        installed: t("lsp-status-found-installed"),
      },
      unknown: t("lsp-status-unknown"),
      restart: t("lsp-status-restart"),
      stop: t("lsp-status-stop"),
      log: t("lsp-status-log"),
      disable: t("lsp-status-disable"),
      install: t("lsp-status-install"),
      close: t("lsp-status-close"),
      disabledNote: (file: string) => t("lsp-status-disabled-note", { file }),
      actionFailed: t("lsp-status-action-failed"),
      logTitle: (server: string) => t("lsp-status-log-title", { server }),
      logEmpty: t("lsp-status-log-empty"),
      logTruncated: t("lsp-status-log-truncated"),
      failedDetail: (message: string) => t("lsp-status-failed-detail", { message }),
      crashedDetail: t("lsp-status-crashed-detail"),
    },
    runtimeMissing: (tool: string, needed: string, install: string) =>
      t("lsp-runtime-missing", { tool, needed, install }),
    untrusted: t("lsp-untrusted"),
    trust: t("lsp-trust"),
    banner: {
      notInstalled: (language: string, server: string, license: string | undefined) =>
        license
          ? t("lsp-banner-not-installed", { language, server, license })
          : t("lsp-banner-not-installed-plain", { language, server }),
      install: t("lsp-banner-install"),
      settings: t("lsp-banner-settings"),
      dontAsk: t("lsp-banner-dont-ask"),
      installing: (server: string) => t("lsp-banner-installing", { server }),
      failed: (message: string) => t("lsp-banner-failed", { message }),
    },
    confirm: {
      title: (name: string, version: string) => t("tools-confirm-title", { name, version }),
      updateTitle: (name: string, version: string) =>
        t("tools-confirm-update-title", { name, version }),
      intro: t("tools-confirm-intro"),
      changed: t("tools-confirm-changed"),
      description: t("tools-confirm-description"),
      version: t("tools-confirm-version"),
      license: t("tools-confirm-license"),
      source: t("tools-confirm-source"),
      integrity: t("tools-confirm-integrity"),
      integrityPinned: t("tools-confirm-integrity-pinned"),
      integrityNone: t("tools-confirm-integrity-none"),
      size: t("tools-confirm-size"),
      sizeUnknown: t("tools-confirm-size-unknown"),
      note: t("tools-confirm-note"),
      homepage: t("tools-confirm-homepage"),
      cancel: t("tools-confirm-cancel"),
      install: t("tools-confirm-install"),
      update: t("tools-confirm-update"),
    },
    manage: {
      title: t("tools-title"),
      intro: t("tools-intro"),
      loading: t("tools-loading"),
      loadFailed: (error: string) => t("tools-load-failed", { error }),
      installed: t("tools-installed"),
      installedEmpty: t("tools-installed-empty"),
      columns: {
        name: t("tools-col-name"),
        version: t("tools-col-version"),
        language: t("tools-col-language"),
        category: t("tools-col-category"),
        license: t("tools-col-license"),
        size: t("tools-col-size"),
        checksum: t("tools-col-checksum"),
        servers: t("tools-col-servers"),
        actions: t("tools-col-actions"),
      },
      checksumPinned: t("tools-checksum-pinned"),
      checksumNone: t("tools-checksum-none"),
      previous: (version: string) => t("tools-previous", { version }),
      problem: (problem: string) => t("tools-problem", { problem }),
      updateTo: (version: string) => t("tools-update-to", { version }),
      rollback: t("tools-rollback"),
      uninstall: t("tools-uninstall"),
      log: t("tools-log"),
      logEmpty: t("tools-log-empty"),
      logClose: t("tools-log-close"),
      search: t("tools-search"),
      searchPlaceholder: t("tools-search-placeholder"),
      filterCategory: t("tools-filter-category"),
      filterLanguage: t("tools-filter-language"),
      filterAll: t("tools-filter-all"),
      install: t("tools-install"),
      installedBadge: t("tools-installed-badge"),
      bundled: t("tools-bundled"),
      deprecated: t("tools-deprecated"),
      unsupported: (reason: string) => t("tools-unsupported", { reason }),
      truncated: (count: number) => t("tools-truncated", { count }),
      searchEmpty: t("tools-search-empty"),
      registry: t("tools-registry"),
      registryVersion: (version: string, pinned: string) =>
        t("tools-registry-version", { version, pinned }),
      registryChecked: (time: string) => t("tools-registry-checked", { time }),
      registrySnapshot: t("tools-registry-snapshot"),
      registryLatest: t("tools-registry-latest"),
      registryFetching: t("tools-registry-fetching"),
      registryError: (error: string) => t("tools-registry-error", { error }),
      busy: (name: string) => t("tools-busy", { name }),
      failed: (name: string, message: string) => t("tools-failed", { name, message }),
      done: (name: string, version: string) => t("tools-done", { name, version }),
      runtimeMissing: (runtime: string) => t("tools-runtime-missing", { runtime }),
    },
    manageAction: t("action-tools-manage"),
    updatesTitle: (count: number) => t("tools-updates-title", { count }),
  };
}

export type ToolsStrings = ReturnType<typeof toolsStrings>;

/** 지금 언어의 문자열. 언어가 바뀌면 새 객체다. */
export function useToolsStrings(): ToolsStrings {
  const { l10n } = useLocalization();
  return useMemo(() => toolsStrings(translator(l10n)), [l10n]);
}
