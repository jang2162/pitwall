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
  };
}

export type RunStrings = ReturnType<typeof runStrings>;

/** 지금 언어의 실행 화면 문자열. 언어가 바뀌면 새 객체다. */
export function useRunStrings(): RunStrings {
  const { l10n } = useLocalization();
  return useMemo(() => runStrings(translator(l10n)), [l10n]);
}
