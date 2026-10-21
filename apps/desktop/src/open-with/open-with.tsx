// 외부 도구로 열기(04 F8-4, 10-ui 「외부 도구 · 프로젝트」 단축키): 「다음에서 열기」 팝업(⌥F1)과 기본 도구로 바로 열기(⌘⌥E).
// 웹뷰는 도구 id 와 문맥(프로젝트 · 활성 워크트리 · 활성 탭의 파일과 커서 줄 · 열)만 넘기고, 명령을 만들어 띄우는 것은 Host 다
// (openWith.run, crates/pw-open-with — 파일 경로의 정규화 · 범위 검사도 그쪽이 한다). 팝업 맨 위에는 창이 넘긴 앱 안 대상
// (`OpenWithPlace` — 프로젝트 창의 「프로젝트 뷰」)이 외부 도구보다 먼저 온다.
//
// 팝업은 상단 검색 팝업(ui/search-popup.tsx)이라 IME 조합 중 Enter · Esc 규칙이 같다. 열린 동안은 창 단축키가 꺼진다
// (useKeymapPopup). 실행하지 못한 이유는 조용히 넘기지 않고 `onError` 로 문장을 넘긴다 — 창이 그 자리에 보여 준다.

import type { Action, Platform } from "@pitwall/keymap";
import type { LaunchProblem, OpenWithList, OpenWithOutcome } from "@pitwall/rpc-client";
import { Command } from "cmdk";
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { type AppStrings, useAppStrings } from "../i18n/app-strings";
import { useKeymapPopup } from "../keymap/keymap-provider";
import { useHost } from "../rpc/host";
import { SEARCH_ITEM_CLASS, SearchPopup } from "../ui/search-popup";

const LOG = "[open-with]";

export interface OpenWithTarget {
  projectId: string;
  /** 팝업 제목에 쓴다. */
  name: string;
  /** 활성 워크트리의 꼭대기(`ActiveWorktree.top`). */
  worktree?: string;
  /** 활성 탭의 파일(절대 경로). 있으면 도구의 `file_command` 로 연다(04 F8-4 「현재 파일/라인 유지」). */
  file?: string;
  /** 1부터. */
  line?: number;
  column?: number;
}

/**
 * 「Finder 에서 보기」(탭 우클릭 · `editor.reveal`)에 쓰는 내장 도구(extensions/external-tools). 파일을 상위 폴더에서 골라 보이는
 * `file_command` 가 있다. Linux 의 `file-manager` 는 `file_command` 가 없어 프로젝트 폴더를 연다.
 */
export const REVEAL_TOOLS: Readonly<Record<Platform, string>> = {
  mac: "finder",
  win: "explorer",
  linux: "file-manager",
};

/** 팝업 맨 위의 앱 안 대상(10-ui 「외부 도구 · 프로젝트」 표의 「프로젝트 뷰」). Host 를 거치지 않고 창이 실행한다. */
export interface OpenWithPlace {
  id: string;
  label: string;
  /** 고를 수 없는 이유. `undefined` 면 고를 수 있다. 팝업을 열 때의 대상으로 판정한다. */
  problem(target: OpenWithTarget): string | undefined;
  run(target: OpenWithTarget): void;
}

type Strings = AppStrings["openWith"];

export function describeProblem(T: Strings, problem: LaunchProblem): string {
  switch (problem.kind) {
    case "unknownTool":
      return T.problem.unknownTool;
    case "invalidTool":
      return T.problem.invalidTool(problem.message);
    case "missingVariable":
      return T.problem.missingVariable(problem.variable);
    case "projectRootMissing":
      return T.problem.projectRootMissing(problem.path);
    case "notFound":
      return T.problem.notFound(problem.program, problem.loginShellPath ? "login" : "process");
    case "spawnFailed":
      return T.problem.spawnFailed(problem.program, problem.message);
    case "bundlePath":
      return T.problem.bundlePath(problem.path);
    case "exited":
      return T.problem.exited(problem.code === null ? "signal" : String(problem.code));
  }
}

/** 실패 결과의 문장. 명령이 있으면 덧붙인다 — 터미널에서 같은 명령을 해 볼 수 있게. */
export function describeFailure(T: Strings, outcome: OpenWithOutcome & { kind: "failed" }): string {
  const message = T.failed(outcome.label, describeProblem(T, outcome.problem));
  return outcome.argv.length > 0 ? `${message} — ${T.command(outcome.argv.join(" "))}` : message;
}

/** 「다음에서 열기」(⌥F1) · 「기본 외부 도구로 열기」(⌘⌥E). 기본 키는 extensions/keymap-default/keymaps/default.toml. */
export function openWithActions(deps: {
  strings: Strings;
  /** 열 대상(프로젝트)이 있는가. */
  enabled(): boolean;
  openPopup(): void;
  openDefault(): void;
}): Action[] {
  const T = deps.strings;
  return [
    {
      id: "openWith.popup",
      label: T.action,
      keywords: ["Open In", "Select In", "External Tool", "외부 도구"],
      enabled: deps.enabled,
      run: deps.openPopup,
    },
    {
      id: "openWith.default",
      label: T.actionDefault,
      keywords: ["Open in Default External Tool", "External Tool", "외부 도구"],
      enabled: deps.enabled,
      run: deps.openDefault,
    },
  ];
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * 창 하나의 「다음에서 열기」. `target` 은 지금의 대상(프로젝트 창은 그 프로젝트, 웰컴 창은 고른 프로젝트)을 돌려준다 — 키를 누른
 * 때의 값을 읽는다. 액션(`openWith.popup` · `openWith.default`)과 팝업 요소를 돌려준다. 액션은 창이 등록한다.
 */
export function useOpenWith({
  target,
  onError,
  places = [],
}: {
  target(): OpenWithTarget | undefined;
  /** 실패 문장. `undefined` 면 지난 문장을 지운다. */
  onError(message: string | undefined): void;
  /** 팝업 맨 위의 앱 안 대상. */
  places?: readonly OpenWithPlace[];
}): {
  actions: Action[];
  openPopup(target?: OpenWithTarget): void;
  /** 도구(없으면 기본 도구)로 바로 연다. 실패는 `onError` 로 간다. */
  run(target: OpenWithTarget, tool?: string): Promise<void>;
  popup: ReactNode;
} {
  const T = useAppStrings().openWith;
  const { client } = useHost();
  const [popup, setPopup] = useState<{ target: OpenWithTarget; note?: string }>();
  const latest = useRef({ T, client, target, onError });
  latest.current = { T, client, target, onError };

  const handle = useCallback((outcome: OpenWithOutcome, forTarget: OpenWithTarget) => {
    const { T, onError } = latest.current;
    switch (outcome.kind) {
      case "launched":
        console.info(LOG, `opened with ${outcome.tool}`, outcome.argv);
        return;
      case "failed":
        console.warn(LOG, `failed: ${outcome.tool}`, outcome.problem);
        onError(describeFailure(T, outcome));
        return;
      case "noDefault":
        // 기본 도구가 없으면 고르게 한다.
        setPopup({
          target: forTarget,
          note: outcome.configured ? T.unknownDefault(outcome.configured) : T.noDefault,
        });
    }
  }, []);

  const run = useCallback(
    async (forTarget: OpenWithTarget, tool?: string) => {
      const { client, onError } = latest.current;
      if (!client) return;
      onError(undefined);
      const outcome = await client
        .call("openWith.run", {
          projectId: forTarget.projectId,
          tool: tool ?? null,
          worktree: forTarget.worktree ?? null,
          ...(forTarget.file && {
            file: forTarget.file,
            line: forTarget.line ?? null,
            column: forTarget.column ?? null,
          }),
        })
        .catch((e: unknown) => {
          console.warn(LOG, "openWith.run failed", e);
          onError(latest.current.T.failed(tool ?? latest.current.T.defaultTool, errorText(e)));
          return undefined;
        });
      if (outcome) handle(outcome, forTarget);
    },
    [handle],
  );

  const openPopup = useCallback((explicit?: OpenWithTarget) => {
    const t = explicit ?? latest.current.target();
    if (t) setPopup({ target: t });
  }, []);

  const actions = useMemo(
    () =>
      openWithActions({
        strings: T,
        enabled: () => latest.current.target() !== undefined,
        openPopup: () => openPopup(),
        openDefault: () => {
          const t = latest.current.target();
          if (t) void run(t);
        },
      }),
    [T, openPopup, run],
  );

  const element = popup ? (
    <OpenWithPopup
      key={popup.target.projectId}
      target={popup.target}
      note={popup.note}
      places={places}
      onClose={() => setPopup(undefined)}
      onPlace={(place) => {
        const t = popup.target;
        setPopup(undefined);
        setTimeout(() => place.run(t), 0);
      }}
      onPick={(tool) => {
        const t = popup.target;
        setPopup(undefined);
        // 팝업이 닫히며 포커스를 돌려놓은 뒤에 띄운다(액션 찾기와 같다).
        setTimeout(() => void run(t, tool), 0);
      }}
    />
  ) : null;

  return { actions, openPopup, run, popup: element };
}

function OpenWithPopup({
  target,
  note,
  places,
  onClose,
  onPlace,
  onPick,
}: {
  target: OpenWithTarget;
  note?: string;
  places: readonly OpenWithPlace[];
  onClose(): void;
  onPlace(place: OpenWithPlace): void;
  onPick(tool: string): void;
}) {
  const T = useAppStrings().openWith;
  const { client } = useHost();
  const [list, setList] = useState<OpenWithList>();
  const [error, setError] = useState<string>();
  useKeymapPopup(true);
  const listFailed = useRef(T.listFailed);
  listFailed.current = T.listFailed;
  useEffect(() => {
    if (!client) return;
    let current = true;
    // 열 때마다 새로 읽는다 — 프로젝트 설정(`[open_with] default`)은 Host 가 감시하지 않는다.
    client
      .call("openWith.list", { projectId: target.projectId, worktree: target.worktree ?? null })
      .then((l) => current && setList(l))
      .catch((e: unknown) => current && setError(listFailed.current(errorText(e))));
    return () => {
      current = false;
    };
  }, [client, target]);
  const defaultId = list?.default.kind === "tool" ? list.default.id : undefined;

  return (
    <SearchPopup
      open
      onOpenChange={(open) => !open && onClose()}
      label={T.label}
      title={
        <>
          {T.title(target.name)}
          {(note ?? error) && (
            <div role="status" className="text-ui-accent">
              {error ?? note}
            </div>
          )}
        </>
      }
      placeholder={T.placeholder}
      empty={list ? T.empty : T.loading}
    >
      {places.map((place) => {
        const problem = place.problem(target);
        return (
          <Command.Item
            key={`place:${place.id}`}
            value={`place:${place.id}`}
            keywords={[place.label]}
            disabled={problem !== undefined}
            onSelect={() => onPlace(place)}
            className={SEARCH_ITEM_CLASS}
            title={problem}
          >
            <span className="flex-1 truncate">{place.label}</span>
            {problem && <span className="truncate text-[11px]">{problem}</span>}
          </Command.Item>
        );
      })}
      {list?.tools.map((tool) => (
        <Command.Item
          key={tool.id}
          value={tool.id}
          keywords={[tool.label]}
          disabled={tool.problem !== null}
          onSelect={() => onPick(tool.id)}
          className={SEARCH_ITEM_CLASS}
          title={tool.problem ? T.unavailable(tool.problem) : undefined}
        >
          <span className="flex-1 truncate">{tool.label}</span>
          {tool.problem && (
            <span className="truncate text-[11px]">{T.unavailable(tool.problem)}</span>
          )}
          {tool.id === defaultId && (
            <span className="flex-none rounded-sm border border-ui-border px-1 text-[11px] text-ui-muted-foreground">
              {T.defaultBadge}
            </span>
          )}
        </Command.Item>
      ))}
    </SearchPopup>
  );
}
