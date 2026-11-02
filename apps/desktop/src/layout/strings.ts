// 프로젝트 창(툴 윈도우 · 에디터 탭 · 상태바 · 백그라운드 작업 · 런타임 환경 · 신뢰 · 알림 · 파일 감시)이 보여 주는 문자열. 내용은 Fluent 리소스
// (extensions/i18n-default/i18n/*/project.ftl)에 있고, 이 파일은 화면 자리와 메시지 id 를 잇는다. 여기 적은 id 가 ko · en
// 리소스에 모두 있는지는 src/i18n/resources.test.ts 가 본다 — 툴 윈도우 이름은 PANELS 의 모든 id 를 거기서 부른다.

import { useLocalization } from "@fluent/react";
import type { Platform } from "@pitwall/keymap";
import type { GitChange, GitConflict, RuntimeProblem } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";
import { lspEditStrings } from "../lsp/edit-strings";
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
    layoutActions: {
      lastToolWindow: t("action-tool-window-last"),
      focusEditor: t("action-focus-editor"),
      preset: {
        default: t("action-layout-preset-default"),
        gitReview: t("action-layout-preset-git-review"),
        db: t("action-layout-preset-db"),
      },
      stretch: {
        left: t("action-tool-window-stretch-left"),
        right: t("action-tool-window-stretch-right"),
        up: t("action-tool-window-stretch-up"),
        down: t("action-tool-window-stretch-down"),
      },
    },
    toolWindowBar: {
      left: t("tool-window-bar-left"),
      right: t("tool-window-bar-right"),
    },
    placeholder: (name: string) => t("tool-window-placeholder", { name }),
    /** 진단 · 문제 툴 윈도우 · 상태바의 진단 수(15-m4-plan M4-06, src/lsp/problems.tsx · diagnostics.ts). */
    diagnostics: {
      title: t("lsp-problems-title"),
      currentFile: t("lsp-problems-current-file"),
      project: t("lsp-problems-project"),
      filter: t("lsp-problems-filter"),
      loading: t("lsp-problems-loading"),
      empty: t("lsp-problems-empty"),
      noFile: t("lsp-problems-no-file"),
      /** 상한에 걸려 일부만 보인다. */
      truncated: (perFile: number, perProject: number) =>
        t("lsp-problems-truncated", { perFile, perProject }),
      /** 파일 줄의 진단 수(상한에 걸렸으면 그렇다고). */
      fileCount: (count: number, truncated: boolean) =>
        t("lsp-problems-file-count", { count, truncated: truncated ? "yes" : "no" }),
      severity: {
        error: t("lsp-severity-error"),
        warning: t("lsp-severity-warning"),
        information: t("lsp-severity-information"),
        hint: t("lsp-severity-hint"),
      },
      /** 진단을 낸 서버와 출처 · 코드(`rustc E0308`). */
      source: (server: string, code: string | null) =>
        code ? t("lsp-diagnostic-source", { server, code }) : server,
      openDocs: t("lsp-diagnostic-open-docs"),
      related: t("lsp-diagnostic-related"),
      /** 상태바 항목의 이름(화면 읽기 · 툴팁). */
      statusLabel: (errors: number, warnings: number, truncated: boolean) =>
        t("lsp-problems-status", { errors, warnings, truncated: truncated ? "yes" : "no" }),
      nextError: t("action-editor-next-error"),
      previousError: t("action-editor-previous-error"),
    },
    editor: {
      /** 가운데에 탭이 하나도 없을 때(빈 영역 표시). */
      empty: t("editor-empty"),
      loading: t("editor-loading"),
      failed: (message: string) => t("editor-read-failed", { message }),
      /** `size`: 「12.3 MiB」 처럼 단위까지 붙인 크기. */
      tooLarge: (size: string) => t("editor-too-large", { size }),
      /** 일반 파일이 아니어서(FIFO · 장치 · 소켓) 열지 않았다. */
      special: t("editor-special-file"),
      /** 열지 않은 큰 파일의 [앞부분 보기]. `size`: 단위까지 붙인 크기. */
      showHead: (size: string) => t("editor-show-head", { size }),
      showHex: t("editor-show-hex"),
      /** [앞부분 보기] 의 위 막대. `shown` · `size`: 단위까지 붙인 보인 크기 · 파일 크기. */
      headNotice: (shown: string, size: string) => t("editor-head-notice", { shown, size }),
      binaryAsText: t("editor-binary-as-text"),
      readOnlyLarge: t("editor-read-only-large"),
      lspPopupActions: t("lsp-popup-actions"),
      /** rename · 코드 액션 · 일괄 편집(M4-12). */
      lspEdit: lspEditStrings(t),
      /** 내비게이션(M4-08 — src/lsp/navigation.ts): 액션 · 위치 팝업 · 외부 소스 탭. */
      lsp: {
        gotoDeclaration: t("action-lsp-goto-declaration"),
        gotoImplementation: t("action-lsp-goto-implementation"),
        findUsages: t("action-lsp-find-usages"),
        quickDocumentation: t("action-lsp-quick-documentation"),
        definitions: (count: number) => t("lsp-locations-definitions", { count }),
        implementations: (count: number) => t("lsp-locations-implementations", { count }),
        usages: (name: string, count: number) => t("lsp-locations-usages", { name, count }),
        none: t("lsp-locations-none"),
        pin: (key: string) => t("lsp-locations-pin", { key }),
        outsideBar: (server: string) => t("lsp-outside-bar", { server }),
        outsideFailed: (message: string) => t("lsp-outside-failed", { message }),
      },
      /** UTF-8 이 아닌 텍스트(결정 M1-D12). `encoding`: WHATWG 이름(예: EUC-KR). */
      readOnlyEncoding: (encoding: string) => t("editor-read-only-encoding", { encoding }),
      /** UTF-8 이지만 깨진 바이트가 있다(판정의 `lossy`). */
      readOnlyLossy: t("editor-read-only-lossy"),
      /** hex 뷰어(M1-23). */
      hex: {
        label: t("editor-hex-label"),
        gotoTitle: t("editor-hex-goto-title"),
        gotoInput: t("editor-hex-goto-input"),
        gotoHint: (current: string, last: string) => t("editor-hex-goto-hint", { current, last }),
        gotoInvalid: t("editor-hex-goto-invalid"),
        failed: (message: string) => t("editor-hex-read-failed", { message }),
      },
      /** 이미지 뷰어(M1-22). */
      image: {
        fit: t("editor-image-fit"),
        actual: t("editor-image-actual"),
        zoom: t("editor-image-zoom"),
        dimensions: (width: number, height: number) =>
          t("editor-image-dimensions", { width, height }),
        failed: (message: string) => t("editor-image-read-failed", { message }),
      },
      /** Markdown 뷰어(M1-22). */
      markdown: {
        mode: t("editor-markdown-mode"),
        editor: t("editor-markdown-editor"),
        preview: t("editor-markdown-preview"),
        split: t("editor-markdown-split"),
        /** `size`: 「120.0 MiB」 처럼 단위까지 붙인 이미지 크기. */
        imageTooLarge: (size: string) => t("editor-markdown-image-too-large", { size }),
        imageLimit: t("editor-markdown-image-limit"),
      },
      openFailed: (path: string) => t("editor-open-failed", { path }),
      copyFailed: t("editor-copy-path-failed"),
      pinned: t("editor-tab-pinned"),
      menu: { pin: t("editor-tab-pin"), unpin: t("editor-tab-unpin") },
      switcher: t("editor-switcher-label"),
      unsaved: t("editor-tab-unsaved"),
      saveFailed: (path: string) => t("editor-save-failed", { path }),
      /** 미저장 편집이 있는 문서가 읽기 전용이 됐다(`saveFailed` 알림의 본문). */
      saveReadOnly: t("editor-save-read-only"),
      disk: {
        changed: t("editor-disk-changed"),
        take: t("editor-disk-take"),
        keep: t("editor-disk-keep"),
        compare: t("editor-disk-compare"),
        compareClose: t("editor-disk-compare-close"),
        compareMine: t("editor-disk-compare-mine"),
        compareDisk: t("editor-disk-compare-disk"),
        deleted: t("editor-disk-deleted"),
        renamed: (path: string) => t("editor-disk-renamed", { path }),
        openRenamed: t("editor-disk-open-renamed"),
        dismiss: t("editor-disk-dismiss"),
      },
      unsavedDialog: {
        title: t("editor-unsaved-title"),
        body: (...files: string[]) =>
          t("editor-unsaved-body", { count: files.length, files: files.join("\n") }),
        save: t("editor-unsaved-save"),
        discard: t("editor-unsaved-discard"),
        cancel: t("editor-unsaved-cancel"),
        incomplete: t("editor-unsaved-save-incomplete"),
      },
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
        save: t("action-editor-save"),
        saveAll: t("action-editor-save-all"),
        find: t("action-editor-find"),
        replace: t("action-editor-replace"),
        gotoLine: t("action-editor-goto-line"),
        undo: t("action-editor-undo"),
        redo: t("action-editor-redo"),
        duplicateLine: t("action-editor-duplicate-line"),
        deleteLine: t("action-editor-delete-line"),
        moveLineUp: t("action-editor-move-line-up"),
        moveLineDown: t("action-editor-move-line-down"),
        commentLine: t("action-editor-comment-line"),
        selectNextOccurrence: t("action-editor-select-next-occurrence"),
        unselectOccurrence: t("action-editor-unselect-occurrence"),
        selectAllOccurrences: t("action-editor-select-all-occurrences"),
        addCursorAbove: t("action-editor-add-cursor-above"),
        addCursorBelow: t("action-editor-add-cursor-below"),
        toggleColumnMode: t("action-editor-toggle-column-mode"),
        singleCursor: t("action-editor-single-cursor"),
        /** 모든 일치 선택이 상한에 걸렸다(M4-14). */
        occurrencesCapped: (cap: number) => t("editor-occurrences-capped", { cap }),
        openAsText: t("action-editor-open-as-text"),
        openAsHex: t("action-editor-open-as-hex"),
        openAsDefault: t("action-editor-open-as-default"),
        gotoOffset: t("action-hex-goto-offset"),
      },
      find: {
        label: t("editor-find-label"),
        search: t("editor-find-search"),
        replace: t("editor-find-replace"),
        caseSensitive: t("editor-find-case"),
        wholeWord: t("editor-find-word"),
        regexp: t("editor-find-regexp"),
        previous: t("editor-find-previous"),
        next: t("editor-find-next"),
        replaceOne: t("editor-find-replace-one"),
        replaceAll: t("editor-find-replace-all"),
        close: t("editor-find-close"),
        invalid: t("editor-find-invalid"),
        none: t("editor-find-none"),
        counting: t("editor-find-counting"),
        count: (total: number | string) => t("editor-find-count", { total }),
        countCurrent: (current: number, total: number | string) =>
          t("editor-find-count-current", { current, total }),
      },
      /** 내비게이션 기록(src/editor/navigation-ui.tsx, M4-09). */
      navigation: {
        back: t("nav-action-back"),
        forward: t("nav-action-forward"),
        recentLocations: t("nav-action-recent-locations"),
        empty: t("nav-recent-empty"),
      },
      gotoLine: {
        title: t("editor-goto-title"),
        input: t("editor-goto-input"),
        hint: (cursor: string, lines: number) => t("editor-goto-hint", { cursor, lines }),
        invalid: t("editor-goto-invalid"),
        go: t("editor-goto-go"),
        cancel: t("editor-goto-cancel"),
      },
      status: {
        cursor: (line: number, column: number) => t("status-editor-cursor", { line, column }),
        selection: (chars: number, breaks: number) =>
          t("status-editor-selection", { chars, breaks }),
        cursorHint: t("status-editor-cursor-hint"),
        /** 커서가 여럿일 때 줄:열 대신(M4-14). */
        cursors: (count: number) => t("status-editor-cursors", { count }),
        /** 컬럼 선택 모드 표지(M4-14). */
        columnMode: t("status-editor-column-mode"),
        columnModeHint: t("status-editor-column-mode-hint"),
        lineEnding: (ending: "lf" | "crlf" | "cr" | "mixed") =>
          t("status-editor-line-ending", { ending }),
        lineEndingHint: t("status-editor-line-ending-hint"),
        encodingHint: t("status-editor-encoding-hint"),
        /** 깨진 바이트를 U+FFFD 로 푼 UTF-8(판정의 `lossy`). */
        encodingLossy: (encoding: string) => t("status-editor-encoding-lossy", { encoding }),
        languageHint: t("status-editor-language-hint"),
      },
      language: {
        plainText: t("language-plain-text"),
        label: t("language-popup-label"),
        placeholder: t("language-popup-placeholder"),
        empty: t("language-popup-empty"),
        detect: (language: string) => t("language-popup-detect", { language }),
        popular: t("language-popup-popular"),
        all: t("language-popup-all"),
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
      /** 작업 제목. Host 는 `task-title-` 로 시작하는 Fluent id 를 보낸다 — 그 밖의 제목(데모 · 확장)은 그대로 보인다. */
      title: (title: string) => (title.startsWith("task-title-") ? t(title) : title),
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
    git: {
      askpass: {
        title: t("git-askpass-title"),
        confirmTitle: t("git-askpass-confirm-title"),
        noticeTitle: t("git-askpass-notice-title"),
        input: t("git-askpass-input"),
        submit: t("git-askpass-submit"),
        cancel: t("git-askpass-cancel"),
        yes: t("git-askpass-yes"),
        no: t("git-askpass-no"),
        dismiss: t("git-askpass-dismiss"),
      },
      /** 트리 · 탭의 git 상태(src/git/git-status-mark.tsx). */
      status: {
        change: {
          added: t("git-status-added"),
          modified: t("git-status-modified"),
          deleted: t("git-status-deleted"),
          renamed: t("git-status-renamed"),
          copied: t("git-status-copied"),
          typeChanged: t("git-status-type-changed"),
          untracked: t("git-status-untracked"),
        } satisfies Record<GitChange, string>,
        conflict: {
          bothModified: t("git-status-conflict-both-modified"),
          bothAdded: t("git-status-conflict-both-added"),
          bothDeleted: t("git-status-conflict-both-deleted"),
          addedByUs: t("git-status-conflict-added-by-us"),
          addedByThem: t("git-status-conflict-added-by-them"),
          deletedByUs: t("git-status-conflict-deleted-by-us"),
          deletedByThem: t("git-status-conflict-deleted-by-them"),
        } satisfies Record<GitConflict, string>,
        conflicted: t("git-status-conflicted"),
        staged: (change: string) => t("git-status-staged", { change }),
        unstaged: (change: string) => t("git-status-unstaged", { change }),
        folder: t("git-status-folder"),
      },
      /** git 문서 · diff 뷰(src/git/documents · src/git/diff, M2-15). */
      documents: {
        actions: {
          showCommit: t("action-git-show-commit"),
          compareCommits: t("action-git-compare-commits"),
          openRevisionFile: t("action-git-open-revision-file"),
          nextChange: t("action-diff-next-change"),
          previousChange: t("action-diff-previous-change"),
        },
        commitTitle: (hash: string) => t("git-doc-commit-title", { hash }),
        compareTitle: (a: string, b: string) => t("git-doc-compare-title", { a, b }),
        revisionTitle: (name: string, hash: string) => t("git-doc-revision-title", { name, hash }),
        blameTitle: (name: string, hash: string) => t("git-doc-blame-title", { name, hash }),
        loading: t("git-doc-loading"),
        failed: (message: string) => t("git-doc-failed", { message }),
        badArgs: (action: string) => t("git-doc-bad-args", { action }),
        author: t("git-doc-author"),
        committer: t("git-doc-committer"),
        parents: t("git-doc-parents"),
        noParents: t("git-doc-no-parents"),
        shallow: t("git-doc-shallow"),
        files: (count: number) => t("git-doc-files", { count }),
        noFiles: t("git-doc-no-files"),
        compareHeading: (a: string, b: string) => t("git-doc-compare-heading", { a, b }),
        revisionBar: (hash: string) => t("git-doc-revision-bar", { hash }),
        pickFile: t("git-doc-pick-file"),
        diff: {
          mode: t("git-diff-mode"),
          split: t("git-diff-split"),
          unified: t("git-diff-unified"),
          ignoreWhitespace: t("git-diff-ignore-whitespace"),
          next: t("git-diff-next"),
          previous: t("git-diff-previous"),
          changes: (count: number) => t("git-diff-changes", { count }),
          unchangedLines: t("git-diff-unchanged-lines"),
          generated: (added: number, removed: number) =>
            t("git-diff-generated", { added, removed }),
          expand: t("git-diff-expand"),
          tooLarge: t("git-diff-too-large"),
          binary: t("git-diff-binary"),
          submodule: (old: string, next: string) => t("git-diff-submodule", { old, new: next }),
          submoduleNone: t("git-diff-submodule-none"),
          error: (message: string) => t("git-diff-error", { message }),
          noContentChange: t("git-diff-no-content-change"),
          modeChange: (old: string, next: string) => t("git-diff-mode-change", { old, new: next }),
          renamed: (old: string) => t("git-diff-renamed", { old }),
          sideUnreadable: t("git-diff-side-unreadable"),
        },
      },
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
    terminal: {
      tabs: t("terminal-tabs-label"),
      tabName: (index: number) => t("terminal-tab-name", { index }),
      exited: t("terminal-tab-exited"),
      newTab: t("terminal-new-tab"),
      closeTab: (name: string) => t("terminal-close-tab", { name }),
      renameInput: t("terminal-rename-input"),
      /** 화면에 덧붙이는 끝 줄. `code` 가 `null` 이면 신호로 끝났다. */
      exitedLine: (code: number | null) => t("terminal-exited", { code: code ?? "none" }),
      failedLine: (message: string) => t("terminal-start-failed", { message }),
      noHost: t("terminal-no-host"),
      copyFailed: t("terminal-copy-failed"),
      pasteFailed: t("terminal-paste-failed"),
      dropRefused: t("terminal-drop-refused"),
      actions: {
        newTab: t("action-terminal-new-tab"),
        closeTab: t("action-terminal-close-tab"),
        renameTab: t("action-terminal-rename-tab"),
        copy: t("action-terminal-copy"),
        paste: t("action-terminal-paste"),
      },
    },
  };
}

export type ProjectStrings = ReturnType<typeof projectStrings>;

/** 지금 언어의 프로젝트 창 문자열. 언어가 바뀌면 새 객체다. */
export function useProjectStrings(): ProjectStrings {
  const { l10n } = useLocalization();
  return useMemo(() => projectStrings(translator(l10n)), [l10n]);
}
