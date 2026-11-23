// 실행 화면이 보이는 문자열(17-m6-plan M6-06 — 「실행」 툴 윈도우 · 콘솔 · 상태바 · 닫기 확인). 내용은
// extensions/i18n-default/i18n/*/run.ftl 이다. 실행 선택기 · 구성 편집(M6-07) · 포트(M6-08) · `sql` 콘솔(M6-09) · 디버그(M6-18)가
// 같은 파일에 묶음을 더한다. id 가 두 언어에 모두 있는지는 src/i18n/resources.test.ts 가 본다.

import { useLocalization } from "@fluent/react";
import type { RunExit, RunState } from "@pitwall/rpc-client";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";

export function runStrings(t: Translate) {
  return {
    tabs: t("run-tabs-label"),
    empty: t("run-empty"),
    noHost: t("run-no-host"),
    state: (state: RunState) => t("run-state", { state }),
    /** 탭 머리의 끝 표시. */
    exit: (exit: RunExit) =>
      typeof exit.code === "number"
        ? t("run-exit-code", { code: exit.code })
        : t("run-exit-signal", { signal: exit.signal ?? "?" }),
    launchedByWindow: t("run-launched-by-window"),
    launchedByAgent: (client: string | null) =>
      t("run-launched-by-agent", { client: client ?? t("run-agent-unknown") }),
    startedAt: (time: string) => t("run-started-at", { time }),
    closeTab: (name: string) => t("run-close-tab", { name }),
    toolbar: {
      rerun: t("run-rerun"),
      stop: t("run-stop"),
      scrollLock: t("run-scroll-lock"),
      clear: t("run-clear"),
      search: t("run-search"),
    },
    search: {
      input: t("run-search-input"),
      next: t("run-search-next"),
      previous: t("run-search-previous"),
      close: t("run-search-close"),
    },
    stopFailed: t("run-stop-failed"),
    rerunFailed: t("run-rerun-failed"),
    /** 콘솔에 덧붙이는 끝 줄. */
    exitedLine: (exit: RunExit) =>
      t("run-exited-line", { code: exit.code ?? "none", signal: exit.signal ?? "?" }),
    droppedLine: (bytes: number) => t("run-dropped-line", { bytes }),
    attachFailed: (message: string) => t("run-attach-failed", { message }),
    /** 포트 칩(탭 머리 · 상태바 — 내용은 M6-08 의 포트를 받아 채운다, run-ports.tsx). */
    port: {
      // 글로 넘긴다 — 숫자면 Fluent 가 자리수를 묶어 `:5,173` 이 된다.
      label: (port: number) => t("run-port-label", { port: String(port) }),
      open: (url: string) => t("run-port-open", { url }),
      allAddresses: t("run-port-all-addresses"),
      /** 다른 머신의 포트(18-m7-plan M7-14 ③ — 이 컴퓨터의 브라우저로 열지 않는다). */
      remote: (port: number, machine: string) =>
        t("run-port-remote", { port: String(port), machine }),
      forward: t("run-port-forward"),
      forwardOpening: t("run-port-forward-opening"),
      forwarded: (url: string) => t("run-port-forwarded", { url }),
      forwardStop: t("run-port-forward-stop"),
      forwardInUse: (port: number) => t("run-port-forward-in-use", { port: String(port) }),
      forwardOther: t("run-port-forward-other"),
      forwardOtherHint: t("run-port-forward-other-hint"),
      forwardRetry: t("run-port-forward-retry"),
      forwardFailed: (message: string) => t("run-port-forward-failed", { message }),
    },
    /** 포트 포워딩 목록(상태바 · 원격 호스트 시트 — forward-list.tsx). */
    forwards: {
      title: t("run-forwards-title"),
      empty: t("run-forwards-empty"),
      row: (local: string, remote: string, host: string) =>
        t("run-forwards-row", { local, remote, host }),
      origin: {
        settings: t("run-forwards-origin-settings"),
        run: t("run-forwards-origin-run"),
      },
      off: t("run-forwards-off"),
      notLoopback: (host: string) => t("run-forwards-not-loopback", { host }),
      portInUse: (port: number) => t("run-forwards-port-in-use", { port: String(port) }),
      failed: (message: string) => t("run-forwards-failed", { message }),
      status: (count: number) => t("run-forwards-status", { count }),
      statusHint: (count: number) => t("run-forwards-status-hint", { count }),
    },
    status: {
      label: (count: number) => t("run-status-label", { count }),
      hint: (count: number) => t("run-status-hint", { count }),
    },
    close: {
      title: (count: number) => t("run-close-title", { count }),
      body: t("run-close-body"),
      human: t("run-close-human"),
      agent: (client: string | null) =>
        t("run-close-agent", { client: client ?? t("run-agent-unknown") }),
      confirm: t("run-close-confirm"),
      cancel: t("run-close-cancel"),
    },
    /** 실행 선택기(M6-07 ① — run-picker.tsx). */
    picker: {
      label: t("run-picker-label"),
      none: t("run-picker-none"),
      withProfile: (name: string, profile: string) =>
        t("run-picker-with-profile", { name, profile }),
      choose: t("run-picker-choose"),
      target: t("run-target"),
      targetActive: (checkout: string) => t("run-target-active", { checkout }),
      checkout: (machine: string, worktree: string | undefined) =>
        t("run-target-checkout", { machine, worktree: worktree ?? "main" }),
      search: t("run-picker-search"),
      empty: t("run-picker-empty"),
      groupConfigs: t("run-picker-group-configs"),
      groupDetected: t("run-picker-group-detected"),
      groupUnusable: t("run-picker-group-unusable"),
      groupMore: t("run-picker-group-more"),
      detectedFrom: (detector: string, source: string) =>
        t("run-picker-detected-from", { detector, source }),
      saveDetected: t("run-picker-save-detected"),
      edit: t("run-picker-edit"),
      create: t("run-picker-new"),
      readTasks: (detector: string) => t("run-picker-read-tasks", { detector }),
      rereadTasks: (detector: string) => t("run-picker-reread-tasks", { detector }),
      readFailed: (detector: string, message: string) =>
        t("run-picker-read-failed", { detector, message }),
      run: t("run-picker-run"),
      debug: t("run-picker-debug"),
      debugNone: t("run-picker-debug-none"),
      debugActiveOnly: t("run-picker-debug-active-only"),
      debugInstallPath: (machine: string, adapter: string) =>
        t("run-picker-debug-install-path", { machine, adapter }),
      debugFailed: (message: string) => t("run-picker-debug-failed", { message }),
      stop: t("run-picker-stop"),
      unusable: (reason: string) => t("run-picker-unusable-reason", { reason }),
      listFailed: (message: string) => t("run-picker-list-failed", { message }),
      startFailed: (message: string) => t("run-start-failed", { message }),
    },
    /** 사슬(`before` · 복합 구성 — M6-21, run-chain.tsx). */
    chain: {
      failed: (config: string, step: string, message: string) =>
        t("run-chain-failed", { config, step, message }),
      stopped: (config: string, step: string) => t("run-chain-stopped", { config, step }),
    },
    /** 키맵 액션(M6-07 ② — run-actions.ts). */
    actions: {
      current: t("run-action-current"),
      debug: t("run-action-debug"),
      choose: t("run-action-choose"),
      chooseIn: t("run-action-choose-in"),
      stop: t("run-action-stop"),
      edit: t("run-action-edit"),
    },
    /** 첫 실행 확인(M6-07 ⑥ — run-confirm-dialog.tsx). */
    confirm: {
      title: (name: string) => t("run-confirm-title", { name }),
      body: t("run-confirm-body"),
      kind: t("run-confirm-kind"),
      command: t("run-confirm-command"),
      definition: t("run-confirm-definition"),
      cwd: t("run-confirm-cwd"),
      profile: t("run-confirm-profile"),
      env: t("run-confirm-env"),
      envNone: t("run-confirm-env-none"),
      envFiles: t("run-confirm-env-files"),
      masked: t("run-confirm-masked"),
      secretWarning: t("run-confirm-secret-warning"),
      notRecorded: t("run-confirm-not-recorded"),
      order: t("run-confirm-order"),
      run: t("run-confirm-run"),
      cancel: t("run-confirm-cancel"),
    },
    /** 구성 편집(M6-07 ③ — config-editor.tsx). */
    editor: {
      titleNew: t("run-editor-title-new"),
      titleEdit: (name: string) => t("run-editor-title-edit", { name }),
      id: t("run-editor-id"),
      idHint: t("run-editor-id-hint"),
      name: t("run-editor-name"),
      type: t("run-editor-type"),
      cwd: t("run-editor-cwd"),
      cwdHint: t("run-editor-cwd-hint"),
      env: t("run-editor-env"),
      envName: t("run-editor-env-name"),
      envValue: t("run-editor-env-value"),
      envAdd: t("run-editor-env-add"),
      envRemove: (name: string) => t("run-editor-env-remove", { name }),
      envFile: t("run-editor-env-file"),
      envFileHint: t("run-editor-env-file-hint"),
      before: t("run-editor-before"),
      beforeHint: t("run-editor-before-hint"),
      listHint: t("run-editor-list-hint"),
      location: t("run-editor-location"),
      locationShared: t("run-editor-location-shared"),
      locationLocal: t("run-editor-location-local"),
      form: t("run-editor-form"),
      toml: t("run-editor-toml"),
      tomlHint: t("run-editor-toml-hint"),
      tomlPath: (path: string) => t("run-editor-toml-path", { path }),
      tomlFailed: (message: string) => t("run-editor-toml-failed", { message }),
      required: (field: string) => t("run-editor-required", { field }),
      save: t("run-editor-save"),
      cancel: t("run-editor-cancel"),
      remove: t("run-editor-delete"),
      saveFailed: (message: string) => t("run-editor-save-failed", { message }),
      deleteFailed: (message: string) => t("run-editor-delete-failed", { message }),
      kept: t("run-editor-kept"),
    },
    /** 설정 창 「실행」 페이지(M6-07 ③). */
    settings: {
      title: t("run-settings-title"),
      body: t("run-settings-body"),
    },
  };
}

export type RunStrings = ReturnType<typeof runStrings>;

/** 지금 언어의 실행 화면 문자열. 언어가 바뀌면 새 객체다. */
export function useRunStrings(): RunStrings {
  const { l10n } = useLocalization();
  return useMemo(() => runStrings(translator(l10n)), [l10n]);
}
