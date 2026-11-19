// 실행 선택기 · 실행 액션(17-m6-plan M6-07 ① ② — 10-ui.md 「S4」 의 타이틀바 `▶ <구성> ▾ ▶ 🐞 ■`).
//
// - 타이틀바(App.tsx 의 `[data-titlebar-end]` 자리)에 구성 이름 단추(▾ — 고르기 팝업) · 실행 · 디버그(M6-18 전에는 꺼짐) · 중지.
// - 고르기 팝업은 세 묶음이다: 쓸 수 있는 구성(프로필이 있으면 프로필마다 한 줄) · 감지된 태스크 · 쓸 수 없는 구성(까닭 — 고를 수
//   없다). 감지된 태스크는 바로 띄우지 않고 「구성으로 저장」 이다(코디네이터 판정 2026-11-19 — `run.start` 가 감지 열쇠를 받지
//   않는다). gradle · maven · mise 태스크는 「태스크 읽기」 를 누를 때만 읽는다(D70).
// - 띄우기는 `run.start`(첫 실행 확인 — run-confirm.ts), `sql` 구성은 `run.sql.start`(위험 확인 — sql-run.ts). 띄운 실행은 곧바로
//   「실행」 툴 윈도우의 탭이 된다(`windowRuns().upsert`, M6-06).

import type { DockviewApi } from "dockview-react";
import type { RunConfigEntry, RunInfo } from "@pitwall/rpc-client";
import { Command } from "cmdk";
import { Bug, ChevronDown, Play, Square, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { useStore } from "zustand";

import { useRiskConfirm } from "../db/safety/risk-confirm";
import { useKeymapPopup, useRegisterActions } from "../keymap/keymap-provider";
import { showPanel } from "../layout/project-layout";
import { useNotify } from "../notifications/notifications";
import { windowDebug } from "../debug/debug-session";
import { activeWorktreeStore } from "../open-with/active-worktree";
import { useHost } from "../rpc/host";
import { SEARCH_ITEM_CLASS, SearchPopup } from "../ui/search-popup";
import { formOf, formOfEntry, idFromName, newForm } from "./config-form";
import { openRunConfigEditor, RunConfigEditorHost } from "./config-editor";
import { runActions } from "./run-actions";
import { withRunConfirms } from "./run-confirm";
import { RunConfirmHost } from "./run-confirm-dialog";
import { errorText, type RunConfigs, windowRunConfigs } from "./run-configs";
import { isLive, type RunTabs, windowRuns } from "./run-tabs";
import { sqlRunQuestion, startSqlRun } from "./sql-run";
import { type RunStrings, useRunStrings } from "./strings";

const TOOL = "flex items-center rounded-sm p-1 hover:bg-ui-hover disabled:opacity-40";

/** 파일 경로의 마지막 이름. */
const baseName = (path: string) => path.split(/[\\/]/).pop() ?? path;

/** 보이는 이름(프로필이 있으면 함께). */
function targetLabel(T: RunStrings["picker"], entry: RunConfigEntry, profile: string | null) {
  const name = entry.config.name || entry.id;
  return profile === null ? name : T.withProfile(name, profile);
}

/**
 * 멈출 실행의 탭: 활성 탭이 돌고 있으면 그것, 아니면 지금 구성의 가장 늦게 띄운 실행, 아니면 가장 늦게 띄운 실행. 탭의 열쇠(`key`)를
 * 돌려준다(`RunTabs.stop` 이 받는다).
 */
export function runToStop(tabs: RunTabs, configId: string | undefined): string | undefined {
  const { tabs: all, activeKey } = tabs.store.getState();
  const active = all.find((t) => t.key === activeKey);
  if (active && isLive(active.run)) return active.key;
  const live = all.filter((t) => isLive(t.run)).sort((a, b) => b.run.startedAt - a.run.startedAt);
  return (live.find((t) => t.run.config === configId) ?? live[0])?.key;
}

/** 타이틀바 · 팝업 · 액션이 함께 쓰는 띄우기. */
function useLauncher(configs: RunConfigs, tabs: RunTabs, api: DockviewApi | undefined) {
  const T = useRunStrings().picker;
  const notify = useNotify();
  const risk = useRiskConfirm();
  const launch = useCallback(
    async (entry: RunConfigEntry, profile: string | null) => {
      const place = configs.place();
      if (!place) return;
      const { client, projectId, worktree } = place;
      try {
        let run: RunInfo | undefined;
        if (entry.config.type === "sql" && (entry.config.before ?? []).length === 0) {
          const started = await startSqlRun(
            (params) => client.call("run.sql.start", params),
            { projectId, worktree, configId: entry.id },
            risk.confirm,
          );
          run = started.kind === "started" ? started.run : undefined;
        } else {
          // 사슬(`before` · 복합 구성)이면 `sql` 단계의 위험 확인도 이 루프가 받는다(run-confirm.ts).
          run = await withRunConfirms(
            (confirmed, sqlConfirmed) =>
              client.call("run.start", {
                projectId,
                worktree,
                configId: entry.id,
                profile,
                confirmed,
                sqlConfirmed,
              }),
            async (data) => {
              const answer = await risk.confirm(sqlRunQuestion(data));
              return answer === undefined || answer === "busy" ? undefined : answer;
            },
          );
        }
        if (!run) return;
        tabs.upsert(run);
        if (api) showPanel(api, "run");
      } catch (e) {
        notify({ level: "error", title: T.startFailed(errorText(e)) });
      } finally {
        // 쓸 수 없게 된 구성 · 새로 기록된 내용이 선택기에 보이게.
        void configs.reload();
      }
    },
    [configs, tabs, api, risk.confirm, notify, T],
  );
  // 디버그(M6-18): 그 구성의 `[debug]` 로 디버그 세션을 연다 — 세션 · 툴 윈도우는 디버그 화면(src/debug/debug-session.ts)이 맡는다.
  const debug = useCallback(
    async (entry: RunConfigEntry, profile: string | null) => {
      const place = configs.place();
      if (!place) return;
      try {
        await windowDebug().start(entry.id, {
          ...(place.worktree !== null &&
            place.worktree !== undefined && { worktree: place.worktree }),
          ...(profile !== null && { profile }),
        });
      } catch (e) {
        notify({ level: "error", title: T.debugFailed(errorText(e)) });
      }
    },
    [configs, notify, T],
  );
  return { launch, debug, riskElement: risk.element };
}

/** 구성 고르기 팝업. `mode` 가 `run` 이면 고른 구성을 곧바로 띄운다(⌃⌥R). */
export function RunChooser({
  configs,
  mode,
  onClose,
  onRun,
}: {
  configs: RunConfigs;
  mode: "select" | "run";
  onClose(): void;
  onRun(entry: RunConfigEntry, profile: string | null): void;
}) {
  const T = useRunStrings().picker;
  useKeymapPopup(true);
  const state = useStore(configs.store);
  const { configs: usable, detected, unusable } = configs.groups();
  useEffect(() => {
    void configs.reload();
  }, [configs]);
  const pick = (entry: RunConfigEntry, profile: string | null) => {
    configs.select({ id: entry.id, profile });
    onClose();
    if (mode === "run") onRun(entry, profile);
  };
  const current = configs.current();
  const taken = [...usable.map((c) => c.id), ...unusable.map((u) => u.id)];
  return (
    <SearchPopup
      open
      onOpenChange={(open) => !open && onClose()}
      label={T.choose}
      placeholder={T.search}
      empty={T.empty}
      title={state.error ? T.listFailed(state.error) : undefined}
    >
      {usable.length > 0 && (
        <Command.Group heading={T.groupConfigs}>
          {usable.flatMap((entry) =>
            [null, ...Object.keys(entry.config.profiles ?? {})].map((profile) => (
              <Command.Item
                key={`${entry.id}\u0000${profile ?? ""}`}
                value={`config:${entry.id}:${profile ?? ""}`}
                keywords={[entry.config.name, entry.id, profile ?? ""]}
                data-run-config={entry.id}
                data-run-profile={profile ?? ""}
                className={SEARCH_ITEM_CLASS}
                onSelect={() => pick(entry, profile)}
              >
                <Play size={12} aria-hidden />
                <span className="truncate">{targetLabel(T, entry, profile)}</span>
                <span className="ml-auto text-[11px] text-ui-muted-foreground">
                  {entry.config.type}
                </span>
              </Command.Item>
            )),
          )}
        </Command.Group>
      )}
      {detected.length > 0 && (
        <Command.Group heading={T.groupDetected}>
          {detected.map((task) => (
            <Command.Item
              key={task.key}
              value={task.key}
              keywords={[task.config.name, task.detector]}
              data-run-detected={task.key}
              className={SEARCH_ITEM_CLASS}
              onSelect={() => {
                onClose();
                openRunConfigEditor({
                  creating: true,
                  form: formOf(idFromName(task.config.name, taken), task.config),
                  kept: task.config,
                });
              }}
            >
              <span className="truncate">{task.config.name}</span>
              <span className="truncate text-[11px] text-ui-muted-foreground">
                {T.detectedFrom(task.detector, baseName(task.source))}
              </span>
              <span className="ml-auto flex-none text-[11px] text-ui-muted-foreground">
                {T.saveDetected}
              </span>
            </Command.Item>
          ))}
        </Command.Group>
      )}
      {unusable.length > 0 && (
        <Command.Group heading={T.groupUnusable}>
          {unusable.map((u) => (
            <Command.Item
              key={u.id}
              value={`unusable:${u.id}`}
              keywords={[u.name ?? "", u.id]}
              disabled
              data-run-unusable={u.id}
              className={SEARCH_ITEM_CLASS}
            >
              <TriangleAlert size={12} aria-hidden />
              <span className="truncate">{u.name ?? u.id}</span>
              <span className="truncate text-[11px]" data-run-unusable-reason>
                {T.unusable(u.reason)}
              </span>
            </Command.Item>
          ))}
        </Command.Group>
      )}
      <Command.Group heading={T.groupMore}>
        {current?.kind === "usable" && (
          <Command.Item
            value="more:edit"
            className={SEARCH_ITEM_CLASS}
            onSelect={() => {
              onClose();
              openRunConfigEditor({
                creating: false,
                form: formOfEntry(current.entry),
                kept: current.entry.config,
              });
            }}
          >
            {T.edit}
          </Command.Item>
        )}
        <Command.Item
          value="more:new"
          className={SEARCH_ITEM_CLASS}
          onSelect={() => {
            onClose();
            openRunConfigEditor({ creating: true, form: newForm(idFromName("run", taken)) });
          }}
        >
          {T.create}
        </Command.Item>
        {(state.list?.taskLists ?? [])
          .filter((l) => l.state !== "read")
          .map((l) => (
            <Command.Item
              key={l.detector}
              value={`more:read:${l.detector}`}
              data-run-read-tasks={l.detector}
              disabled={state.reading.includes(l.detector)}
              className={SEARCH_ITEM_CLASS}
              onSelect={() => void configs.readTasks(l.detector)}
            >
              {l.state === "stale" ? T.rereadTasks(l.detector) : T.readTasks(l.detector)}
              {state.readErrors[l.detector] && (
                <span role="alert" className="truncate text-[11px] text-ui-accent">
                  {T.readFailed(l.detector, state.readErrors[l.detector]!)}
                </span>
              )}
            </Command.Item>
          ))}
      </Command.Group>
    </SearchPopup>
  );
}

/** 타이틀바의 `▶ <구성> ▾ ▶ 🐞 ■`. */
export function RunToolbar({
  configs,
  tabs,
  onChoose,
  onRun,
  onDebug,
}: {
  configs: RunConfigs;
  tabs: RunTabs;
  onChoose(): void;
  onRun(entry: RunConfigEntry, profile: string | null): void;
  onDebug(entry: RunConfigEntry, profile: string | null): void;
}) {
  const T = useRunStrings().picker;
  useStore(configs.store);
  const runTabs = useStore(tabs.store, (s) => s.tabs);
  const current = configs.current();
  const usable = current?.kind === "usable" ? current : undefined;
  const debuggable = usable?.entry.config.debug ? usable : undefined;
  const label = usable
    ? targetLabel(T, usable.entry, usable.profile)
    : current?.kind === "unusable"
      ? (current.entry.name ?? current.entry.id)
      : T.none;
  const reason = current?.kind === "unusable" ? T.unusable(current.entry.reason) : undefined;
  const stoppable = runTabs.some((t) => isLive(t.run));
  return (
    <div className="flex items-center gap-0.5 font-normal" data-testid="run-toolbar">
      <button
        type="button"
        className="flex max-w-[220px] items-center gap-1 rounded-sm px-1.5 py-0.5 hover:bg-ui-hover"
        aria-label={T.choose}
        title={reason ?? T.choose}
        data-testid="run-picker"
        onClick={onChoose}
      >
        {reason ? <TriangleAlert size={12} aria-hidden /> : <Play size={12} aria-hidden />}
        <span className="truncate">{label}</span>
        <ChevronDown size={12} aria-hidden />
      </button>
      {reason && (
        <span className="sr-only" data-testid="run-unusable-reason">
          {reason}
        </span>
      )}
      <button
        type="button"
        className={TOOL}
        aria-label={T.run}
        title={reason ?? T.run}
        data-testid="run-start"
        disabled={!usable}
        onClick={() => usable && onRun(usable.entry, usable.profile)}
      >
        <Play size={14} aria-hidden />
      </button>
      <button
        type="button"
        className={TOOL}
        aria-label={T.debug}
        title={reason ?? (debuggable ? T.debug : T.debugNone)}
        data-testid="run-debug"
        disabled={!debuggable}
        onClick={() => debuggable && onDebug(debuggable.entry, debuggable.profile)}
      >
        <Bug size={14} aria-hidden />
      </button>
      <button
        type="button"
        className={TOOL}
        aria-label={T.stop}
        title={T.stop}
        data-testid="run-stop"
        disabled={!stoppable}
        onClick={() => {
          const key = runToStop(tabs, current?.entry.id);
          if (key) void tabs.stop(key);
        }}
      >
        <Square size={13} aria-hidden />
      </button>
    </div>
  );
}

/** 타이틀바 끝 자리(App.tsx). 프로젝트 창이 그린 뒤에 찾는다. */
function useTitlebarSlot(): Element | null {
  const [slot, setSlot] = useState<Element | null>(null);
  useEffect(() => setSlot(document.querySelector("[data-titlebar-end]")), []);
  return slot;
}

/**
 * 프로젝트 창의 실행 선택기 · 액션 · 첫 실행 확인 · 구성 편집을 붙인다. `StatusBarProvider` · 키맵 안에 둔다. `api` 는 띄운 뒤
 * 「실행」 툴 윈도우를 보이는 데 쓴다.
 */
export function RunPickerFeature({
  projectId,
  api,
  configs = windowRunConfigs(),
  tabs = windowRuns(),
}: {
  projectId: string;
  api: DockviewApi | undefined;
  configs?: RunConfigs;
  tabs?: RunTabs;
}) {
  const strings = useRunStrings();
  const { client } = useHost();
  const worktree = useStore(activeWorktreeStore, (s) => s.top ?? null);
  const [chooser, setChooser] = useState<"select" | "run">();
  const { launch, debug, riskElement } = useLauncher(configs, tabs, api);
  const slot = useTitlebarSlot();

  useEffect(
    () => (client ? configs.connect(client, projectId, worktree) : undefined),
    [configs, client, projectId, worktree],
  );

  const runCurrent = useCallback(() => {
    const current = configs.current();
    if (current?.kind === "usable") void launch(current.entry, current.profile);
  }, [configs, launch]);

  const actions = useMemo(
    () =>
      runActions({
        strings,
        canRun: () => configs.current()?.kind === "usable",
        runCurrent,
        canDebug: () => {
          const current = configs.current();
          return current?.kind === "usable" && Boolean(current.entry.config.debug);
        },
        debugCurrent: () => {
          const current = configs.current();
          if (current?.kind === "usable" && current.entry.config.debug)
            void debug(current.entry, current.profile);
        },
        chooseAndRun: () => setChooser("run"),
        canStop: () => runToStop(tabs, configs.current()?.entry.id) !== undefined,
        stop: () => {
          const key = runToStop(tabs, configs.current()?.entry.id);
          if (key) void tabs.stop(key);
        },
        editConfigs: () => {
          const current = configs.current();
          if (current?.kind === "usable")
            openRunConfigEditor({
              creating: false,
              form: formOfEntry(current.entry),
              kept: current.entry.config,
            });
          else {
            const { configs: usable, unusable } = configs.groups();
            const taken = [...usable.map((c) => c.id), ...unusable.map((u) => u.id)];
            openRunConfigEditor({ creating: true, form: newForm(idFromName("run", taken)) });
          }
        },
      }),
    [strings, configs, tabs, runCurrent, debug],
  );
  useRegisterActions(actions);
  // 실행 · 중지 액션의 `enabled` 가 목록 · 탭을 본다 — 바뀌면 메뉴 · 키가 다시 본다.
  useStore(configs.store);
  useStore(tabs.store);

  return (
    <>
      {slot &&
        createPortal(
          <RunToolbar
            configs={configs}
            tabs={tabs}
            onChoose={() => setChooser("select")}
            onRun={(entry, profile) => void launch(entry, profile)}
            onDebug={(entry, profile) => void debug(entry, profile)}
          />,
          slot,
        )}
      {chooser && (
        <RunChooser
          configs={configs}
          mode={chooser}
          onClose={() => setChooser(undefined)}
          onRun={(entry, profile) => void launch(entry, profile)}
        />
      )}
      <RunConfirmHost />
      <RunConfigEditorHost configs={configs} />
      {riskElement}
    </>
  );
}
