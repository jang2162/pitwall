// 디버그 화면이 보이는 문자열(17-m6-plan M6-18 — 「디버그」 툴 윈도우 · 거터 · 브레이크포인트 편집 · 목록 · 액션). 내용은
// extensions/i18n-default/i18n/*/run.ftl 의 「디버그」 묶음이다(실행 화면과 한 파일 — src/run/strings.ts 머리 주석).

import { useLocalization } from "@fluent/react";
import { useMemo } from "react";

import { type Translate, translator } from "../i18n/localization";

export function debugStrings(t: Translate) {
  return {
    noHost: t("debug-no-host"),
    noSession: t("debug-no-session"),
    sessions: t("debug-sessions-label"),
    state: (state: "running" | "stopped" | "ended") => t("debug-state", { state }),
    stoppedReason: (reason: string) => t("debug-stopped-reason", { reason }),
    toolbar: {
      resume: t("debug-resume"),
      pause: t("debug-pause"),
      stepOver: t("debug-step-over"),
      stepInto: t("debug-step-into"),
      stepOut: t("debug-step-out"),
      stop: t("debug-stop"),
    },
    panes: {
      debugger: t("debug-debugger"),
      frames: t("debug-frames"),
      threads: t("debug-threads"),
      variables: t("debug-variables"),
      watches: t("debug-watches"),
      console: t("debug-console"),
    },
    noFrames: t("debug-no-frames"),
    noVariables: t("debug-no-variables"),
    outsideFrame: t("debug-outside-frame"),
    addWatch: t("debug-add-watch"),
    removeWatch: t("debug-remove-watch"),
    consoleInput: t("debug-console-input"),
    consoleHint: t("debug-console-hint"),
    more: (count: number) => t("debug-more", { count }),
    cut: t("debug-value-cut"),
    tooDeep: t("debug-too-deep"),
    readFailed: (message: string) => t("debug-read-failed", { message }),
    startFailed: t("debug-start-failed"),
    controlFailed: t("debug-control-failed"),
    /** 디버그 소스 탭(루트 밖 프레임 — 리뷰 F1). */
    source: {
      outside: t("debug-source-outside"),
      held: t("debug-source-held"),
      adapter: (adapter: string, name: string) => t("debug-source-adapter", { adapter, name }),
      reported: (path: string) => t("debug-source-reported", { path }),
    },
    breakpoint: {
      title: (file: string, line: number) => t("debug-bp-title", { file, line }),
      enabled: t("debug-bp-enabled"),
      condition: t("debug-bp-condition"),
      logMessage: t("debug-bp-log-message"),
      hitCondition: t("debug-bp-hit-condition"),
      remove: t("debug-bp-remove"),
      done: t("debug-bp-done"),
      plain: t("debug-bp-plain"),
      describeCondition: (text: string) => t("debug-bp-describe-condition", { text }),
      describeLog: (text: string) => t("debug-bp-describe-log", { text }),
      describeHit: (text: string) => t("debug-bp-describe-hit", { text }),
      disabled: t("debug-bp-disabled"),
      unverified: (message: string) => t("debug-bp-unverified", { message }),
      agent: t("debug-bp-agent"),
      agentExpression: t("debug-bp-agent-expression"),
    },
    list: {
      title: t("debug-bp-list-title"),
      empty: t("debug-bp-list-empty"),
      unsaved: t("debug-bp-list-unsaved"),
      problem: (message: string) => t("debug-bp-list-problem", { message }),
    },
    actions: {
      stepOver: t("action-debug-step-over"),
      stepInto: t("action-debug-step-into"),
      stepOut: t("action-debug-step-out"),
      resume: t("action-debug-resume"),
      pause: t("action-debug-pause"),
      toggleBreakpoint: t("action-debug-toggle-breakpoint"),
      viewBreakpoints: t("action-debug-view-breakpoints"),
      evaluate: t("action-debug-evaluate"),
    },
  };
}

export type DebugStrings = ReturnType<typeof debugStrings>;

/** 지금 언어의 디버그 화면 문자열. 언어가 바뀌면 새 객체다. */
export function useDebugStrings(): DebugStrings {
  const { l10n } = useLocalization();
  return useMemo(() => debugStrings(translator(l10n)), [l10n]);
}
