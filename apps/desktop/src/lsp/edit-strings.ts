// rename · 코드 액션 · 일괄 편집(15-m4-plan M4-12)이 보이는 문자열. 내용은 extensions/i18n-default/i18n/*/project.ftl 의
// `lsp-edit-…` 묶음이고, 프로젝트 창 문자열(src/layout/strings.ts)의 `editor.lspEdit` 로 들어간다.

import type { Translate } from "../i18n/localization";

export function lspEditStrings(t: Translate) {
  return {
    actions: {
      rename: t("lsp-edit-action-rename"),
      codeActions: t("lsp-edit-action-code-actions"),
      undoLast: t("lsp-edit-action-undo-last"),
    },
    quickFix: t("lsp-edit-quick-fix"),
    renameInput: t("lsp-edit-rename-input"),
    renameHint: t("lsp-edit-rename-hint"),
    renameInvalid: (message: string | null | undefined) =>
      message ? t("lsp-edit-rename-invalid-reason", { message }) : t("lsp-edit-rename-invalid"),
    renameFailed: (message: string) => t("lsp-edit-rename-failed", { message }),
    codeActions: t("lsp-edit-code-actions"),
    codeActionsNone: t("lsp-edit-code-actions-none"),
    codeActionFailed: (message: string) => t("lsp-edit-code-action-failed", { message }),
    stale: t("lsp-edit-stale"),
    foreign: (files: string) => t("lsp-edit-foreign", { files }),
    ended: t("lsp-edit-ended"),
    refused: (message: string) => t("lsp-edit-refused", { message }),
    failed: (message: string) => t("lsp-edit-failed", { message }),
    applied: (count: number) => t("lsp-edit-applied", { count }),
    editorOnly: (files: string) => t("lsp-edit-editor-only", { files }),
    notWritten: (files: string) => t("lsp-edit-not-written", { files }),
    undo: t("lsp-edit-undo"),
    reverted: (count: number) => t("lsp-edit-reverted", { count }),
    notReverted: (files: string) => t("lsp-edit-not-reverted", { files }),
    nothingToUndo: t("lsp-edit-nothing-to-undo"),
    preview: {
      title: (count: number) => t("lsp-edit-preview-title", { count }),
      apply: t("lsp-edit-preview-apply"),
      cancel: t("lsp-edit-preview-cancel"),
      editorOnly: t("lsp-edit-preview-editor-only"),
      lines: (lines: number) => t("lsp-edit-preview-lines", { lines }),
      create: t("lsp-edit-preview-create"),
      rename: (from: string) => t("lsp-edit-preview-rename", { from }),
      delete: t("lsp-edit-preview-delete"),
    },
  };
}

export type LspEditStrings = ReturnType<typeof lspEditStrings>;
