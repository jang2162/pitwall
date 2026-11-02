// 문제 툴 윈도우 · 상태바의 진단 수 · 다음/이전 오류(15-m4-plan M4-06). 창이 `useDiagnosticsUi` 를 한 번 부른다(project-window.tsx).
//
// - 창 연결: 창의 LSP 세션(`workspace.lsp` — 프로젝트 · `services`)과 활성 워크트리를 `diagnosticsUi`(./diagnostics-model.ts)에 둔다 —
//   에디터의 진단 기능 · 이 파일의 화면이 함께 읽는다.
// - 상태바 `problems` 자리: `⚠ n ⛔ n` — 수는 `lsp.diagnostics.counts` 의 답 그대로다(완료 기준 ⑥). 이벤트 `lsp.diagnostics` 가 오면
//   다시 묻되 묻는 중이면 끝난 뒤 한 번 더만 묻는다(이벤트가 몰려도 RPC 가 쌓이지 않는다). 누르면 문제 툴 윈도우를 연다
//   (`toolWindow.problems` 액션). 상한에 걸리면 수 뒤에 `+`.
// - 액션 `editor.nextError` · `editor.previousError`(`F2` · `⇧F2`, 글자 영역): 활성 에디터 안에서 다음 · 이전 오류(./diagnostics.ts
//   `gotoDiagnostic`).
// - 문제 툴 윈도우(자리 `problems`): 「현재 파일」 · 「프로젝트」 탭, 파일별 트리(파일 줄 → 진단 줄, 보이는 줄만 그린다 — TanStack
//   Virtual), 심각도 거르기, 줄 더블클릭 · `↩` 로 그 위치를 연다(위치로 가기 `navigateTo` — 뒤로 가기 기록에 넣는다). 열지 않은
//   파일의 진단도 보인다(서버가 보낸 만큼). 툴 윈도우가 열려 있는 동안 `lsp.diagnostics.list` 를 쪽마다 끝까지 읽고 이벤트로 경로를
//   바꾼다. 상한에 걸리면 위에 그렇다고 알린다. 미신뢰 프로젝트에서는 서버가 없어 빈 목록이다(오류를 보이지 않는다 — 결정 M4-D8).

import type { Action } from "@pitwall/keymap";
import type { LspDiagnostic, LspDiagnosticsCounts } from "@pitwall/rpc-client";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { DockviewApi, IDockviewPanelProps } from "dockview-react";
import { ChevronDown, ChevronRight, CircleX, File, TriangleAlert } from "lucide-react";
import { type KeyboardEvent, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";

import { targetView } from "../editor/editor-ops";
import { type EditorWorkspace, useEditorWorkspace } from "../editor/editor-workspace";
import { useKeymap, useRegisterActions } from "../keymap/keymap-provider";
import { type ProjectStrings, useProjectStrings } from "../layout/strings";
import { useActiveWorktree } from "../open-with/active-worktree";
import { useHost } from "../rpc/host";
import { STATUS_SLOTS, useStatusBarItem } from "../status-bar/status-bar";
import {
  callSafely,
  DIAGNOSTICS_TOPIC,
  type DiagnosticsConnection,
  diagnosticsUi,
  FILE_LIMIT,
  type LspDiagnosticsEvent,
  type LspSeverity,
  normalizeWorktree,
  PROJECT_LIMIT,
  SEVERITIES,
} from "./diagnostics-model";
import { gotoDiagnostic } from "./diagnostics";

const ROW_HEIGHT = 22;
/** 툴 윈도우가 한 번에 읽는 쪽의 크기(Host 의 `LIST_LIMIT`). */
const PAGE = 5_000;

const GLYPH_CLASS: Record<LspSeverity, string> = {
  error: "text-[var(--pw-diagnostic-error)]",
  warning: "text-[var(--pw-diagnostic-warning)]",
  information: "text-[var(--pw-diagnostic-info)]",
  hint: "text-[var(--pw-diagnostic-hint)]",
};
const GLYPHS: Record<LspSeverity, string> = {
  error: "✖",
  warning: "▲",
  information: "●",
  hint: "◇",
};

type DiagnosticStringsUi = ProjectStrings["diagnostics"];

// ---- 창 연결 --------------------------------------------------------------------------------------------------------------------

/** 창의 진단 화면을 붙인다: 연결 · 문자열 · 상태바의 수 · `F2` 액션. 프로젝트 창이 한 번 부른다. */
export function useDiagnosticsUi(workspace: EditorWorkspace) {
  const T = useProjectStrings().diagnostics;
  const worktree = useActiveWorktree();
  const sessions = workspace.lsp;
  // Host 가 붙은 뒤에 묻는다(붙기 전의 `services.call` 은 거절된다). 다시 붙으면 다시 묻는다.
  const client = useHost().client;

  useEffect(() => {
    diagnosticsUi.setState({
      strings: {
        severity: T.severity,
        source: T.source,
        openDocs: T.openDocs,
        related: T.related,
      },
    });
  }, [T]);

  useEffect(() => {
    if (!sessions || !client) return;
    const connection: DiagnosticsConnection = {
      services: sessions.services,
      projectId: sessions.projectId,
      ...(worktree !== undefined && { worktree }),
    };
    diagnosticsUi.setState({ connection, counts: null });
    let alive = true;
    let asking = false;
    let again = false;
    const ask = () => {
      if (asking) {
        again = true;
        return;
      }
      asking = true;
      again = false;
      callSafely(connection.services, "lsp.diagnostics.counts", {
        projectId: connection.projectId,
        worktree: connection.worktree ?? null,
      })
        .then((counts: LspDiagnosticsCounts) => {
          if (alive) diagnosticsUi.setState({ counts });
        })
        .catch(() => {})
        .finally(() => {
          asking = false;
          if (alive && again) ask();
        });
    };
    const mine = normalizeWorktree(worktree);
    const off = connection.services.subscribe([DIAGNOSTICS_TOPIC], (event) => {
      const payload = event.payload as unknown as LspDiagnosticsEvent;
      if (normalizeWorktree(payload.worktree) === mine) ask();
    });
    ask();
    return () => {
      alive = false;
      off();
      if (diagnosticsUi.getState().connection === connection)
        diagnosticsUi.setState({ connection: undefined, counts: null });
    };
  }, [sessions, worktree, client]);

  const actions = useMemo(() => diagnosticActions({ workspace, strings: T }), [workspace, T]);
  useRegisterActions(actions);
  useStatusBarItem({ id: "problems", ...STATUS_SLOTS.problems, component: ProblemsStatusItem });
}

/** `F2` · `⇧F2` — 활성 에디터 안의 다음 · 이전 오류(./diagnostics.ts `gotoDiagnostic`). */
export function diagnosticActions(deps: {
  workspace: EditorWorkspace;
  strings: Pick<DiagnosticStringsUi, "nextError" | "previousError">;
}): Action[] {
  const view = () => targetView(deps.workspace);
  const go = (forward: boolean) => () => {
    const v = view();
    if (v) gotoDiagnostic(v, forward);
  };
  return [
    {
      id: "editor.nextError",
      label: deps.strings.nextError,
      keywords: ["Next Highlighted Error", "Problems"],
      enabled: () => view() !== undefined,
      repeat: true,
      run: go(true),
    },
    {
      id: "editor.previousError",
      label: deps.strings.previousError,
      keywords: ["Previous Highlighted Error", "Problems"],
      enabled: () => view() !== undefined,
      repeat: true,
      run: go(false),
    },
  ];
}

// ---- 상태바 ---------------------------------------------------------------------------------------------------------------------

function ProblemsStatusItem() {
  const T = useProjectStrings().diagnostics;
  const counts = useStore(diagnosticsUi, (s) => s.counts);
  const connected = useStore(diagnosticsUi, (s) => s.connection !== undefined);
  const { registry } = useKeymap();
  if (!connected || !counts) return null;
  const more = counts.truncated ? "+" : "";
  return (
    <button
      type="button"
      data-testid="status-problems"
      aria-label={T.statusLabel(counts.error, counts.warning, counts.truncated)}
      title={T.statusLabel(counts.error, counts.warning, counts.truncated)}
      onClick={() => void registry.run("toolWindow.problems")}
      className="flex items-center gap-1.5 rounded-sm px-1 hover:bg-ui-hover"
    >
      <span className="flex items-center gap-0.5" data-severity="warning">
        <TriangleAlert size={11} aria-hidden className={GLYPH_CLASS.warning} />
        <span data-count="warning">
          {counts.warning}
          {more}
        </span>
      </span>
      <span className="flex items-center gap-0.5" data-severity="error">
        <CircleX size={11} aria-hidden className={GLYPH_CLASS.error} />
        <span data-count="error">
          {counts.error}
          {more}
        </span>
      </span>
    </button>
  );
}

// ---- 문제 툴 윈도우 -------------------------------------------------------------------------------------------------------------

interface FileProblems {
  readonly diagnostics: readonly LspDiagnostic[];
  readonly truncated: boolean;
}

interface ProblemsData {
  readonly files: ReadonlyMap<string, FileProblems>;
  readonly truncated: boolean;
  readonly loading: boolean;
}

/** 툴 윈도우가 열려 있는 동안 프로젝트의 진단을 쥔다: 쪽마다 끝까지 읽고, 이벤트로 경로를 바꾼다. */
function useProblemsData(connection: DiagnosticsConnection): ProblemsData {
  const [data, setData] = useState<ProblemsData>({
    files: new Map(),
    truncated: false,
    loading: true,
  });
  useEffect(() => {
    let alive = true;
    const files = new Map<string, FileProblems>();
    /** 읽는 사이 이벤트로 받은 경로 — 쪽의 것보다 새것이다. */
    const evented = new Set<string>();
    let loading = true;
    let scheduled = false;
    // 이벤트가 몰려도 한 프레임에 한 번 그린다(진단 1만 개를 경로마다 다시 그리지 않게).
    const publish = () => {
      if (scheduled) return;
      scheduled = true;
      queueMicrotask(() => {
        scheduled = false;
        // 머리 안내는 지금 파일들의 것으로 매번 센다 — 한 번 켜진 채 남지 않는다(M4-06 리뷰 L4). Host 는 상한에 잘려 진단이
        // 0개인 파일도 싣는다.
        const truncated = [...files.values()].some((f) => f.truncated);
        if (alive) setData({ files: new Map(files), truncated, loading });
      });
    };
    const mine = normalizeWorktree(connection.worktree);
    const off = connection.services.subscribe([DIAGNOSTICS_TOPIC], (event) => {
      const payload = event.payload as unknown as LspDiagnosticsEvent;
      if (normalizeWorktree(payload.worktree) !== mine) return;
      evented.add(payload.path);
      if (payload.diagnostics.length === 0 && !payload.truncated) files.delete(payload.path);
      else
        files.set(payload.path, {
          diagnostics: payload.diagnostics,
          truncated: payload.truncated,
        });
      publish();
    });
    const read = async (cursor: string | null) => {
      const page = await callSafely(connection.services, "lsp.diagnostics.list", {
        projectId: connection.projectId,
        worktree: connection.worktree ?? null,
        cursor,
        limit: PAGE,
      });
      if (!alive) return;
      for (const file of page.files) {
        if (evented.has(file.path)) continue;
        const before = cursor === null ? undefined : files.get(file.path);
        files.set(file.path, {
          diagnostics: [...(before?.diagnostics ?? []), ...file.diagnostics],
          truncated: file.truncated,
        });
      }
      if (page.nextCursor) await read(page.nextCursor);
    };
    read(null)
      .catch(() => {})
      .finally(() => {
        loading = false;
        publish();
      });
    return () => {
      alive = false;
      off();
    };
  }, [connection]);
  return data;
}

/**
 * 활성 에디터 탭의 파일(툴 윈도우에 포커스가 있어도 마지막 에디터 탭). 도킹 영역은 패널의 `containerApi` 로 듣는다 — 툴 윈도우는
 * 작업 공간이 도킹 api 를 받기(`attach`) 전에 그려질 수 있다.
 */
function useActiveFile(workspace: EditorWorkspace, dock: DockviewApi): string | undefined {
  const [path, setPath] = useState(() => workspace.activeContext()?.file);
  useEffect(() => {
    const update = () => setPath(workspace.activeContext()?.file);
    update();
    const d = dock.onDidActivePanelChange(update);
    return () => d.dispose();
  }, [workspace, dock]);
  return path;
}

type ProblemRow =
  | {
      kind: "file";
      path: string;
      count: number;
      collapsed: boolean;
      truncated: boolean;
      worst: LspSeverity;
    }
  | { kind: "diagnostic"; path: string; index: number; diagnostic: LspDiagnostic };

const rowKey = (r: ProblemRow) => (r.kind === "file" ? r.path : `${r.path}\n${r.index}`);

function problemRows(
  data: ProblemsData,
  only: string | undefined,
  scope: "file" | "project",
  shown: ReadonlySet<LspSeverity>,
  collapsed: ReadonlySet<string>,
): ProblemRow[] {
  const rows: ProblemRow[] = [];
  const paths = [...data.files.keys()].sort();
  for (const path of paths) {
    if (scope === "file" && path !== only) continue;
    const file = data.files.get(path)!;
    const visible = file.diagnostics.filter((d) => shown.has(d.severity));
    if (visible.length === 0) continue;
    const isCollapsed = collapsed.has(path);
    const worst = visible.reduce<LspSeverity>(
      (w, d) => (SEVERITIES.indexOf(d.severity) < SEVERITIES.indexOf(w) ? d.severity : w),
      "hint",
    );
    rows.push({
      kind: "file",
      path,
      count: visible.length,
      collapsed: isCollapsed,
      truncated: file.truncated,
      worst,
    });
    if (!isCollapsed)
      visible.forEach((diagnostic, index) =>
        rows.push({ kind: "diagnostic", path, index, diagnostic }),
      );
  }
  return rows;
}

function baseName(path: string) {
  return path.slice(path.lastIndexOf("/") + 1);
}

function dirName(path: string) {
  const at = path.lastIndexOf("/");
  return at > 0 ? path.slice(0, at) : "";
}

export function ProblemsToolWindow({ api, containerApi }: IDockviewPanelProps) {
  const T = useProjectStrings().diagnostics;
  const connection = useStore(diagnosticsUi, (s) => s.connection);
  // 창의 Host · LSP 세션이 붙기 전(그리고 에디터 작업 공간이 없는 도킹 영역만의 시험)에는 빈 자리다.
  if (!connection)
    return (
      <div
        data-tool-window={api.id}
        tabIndex={-1}
        className="flex h-full flex-col bg-ui-panel text-ui-foreground outline-none"
      >
        <p className="p-2 text-[12px] text-ui-muted-foreground">{T.empty}</p>
      </div>
    );
  return <ProblemsView api={api} dock={containerApi} connection={connection} />;
}

function ProblemsView({
  api,
  dock,
  connection,
}: {
  api: IDockviewPanelProps["api"];
  dock: DockviewApi;
  connection: DiagnosticsConnection;
}) {
  const T = useProjectStrings().diagnostics;
  const workspace = useEditorWorkspace();
  const data = useProblemsData(connection);
  const active = useActiveFile(workspace, dock);
  const [scope, setScope] = useState<"file" | "project">("project");
  const [shown, setShown] = useState<ReadonlySet<LspSeverity>>(new Set(SEVERITIES));
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const [selectedKey, setSelectedKey] = useState<string>();
  const scrollRef = useRef<HTMLDivElement>(null);
  const rows = useMemo(
    () => problemRows(data, active, scope, shown, collapsed),
    [data, active, scope, shown, collapsed],
  );
  const counts = useMemo(() => {
    const c: Record<LspSeverity, number> = { error: 0, warning: 0, information: 0, hint: 0 };
    for (const [path, file] of data.files)
      if (scope === "project" || path === active)
        for (const d of file.diagnostics) c[d.severity] += 1;
    return c;
  }, [data, scope, active]);
  const truncated =
    scope === "project"
      ? data.truncated
      : active !== undefined && (data.files.get(active)?.truncated ?? false);
  const selected =
    selectedKey === undefined ? -1 : rows.findIndex((r) => rowKey(r) === selectedKey);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (index) => (rows[index] ? rowKey(rows[index]) : index),
    overscan: 12,
  });
  const [scrollPending, setScrollPending] = useState(0);
  useLayoutEffect(() => {
    if (scrollPending > 0 && selected >= 0) virtualizer.scrollToIndex(selected, { align: "auto" });
  }, [scrollPending, selected, virtualizer]);

  const select = (index: number) => {
    const row = rows[index];
    if (!row) return;
    setSelectedKey(rowKey(row));
    setScrollPending((n) => n + 1);
  };
  const toggle = (path: string, open?: boolean) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      const close = open === undefined ? !next.has(path) : !open;
      if (close) next.add(path);
      else next.delete(path);
      return next;
    });
  const activate = (row: ProblemRow) => {
    if (row.kind === "file") {
      toggle(row.path);
      return;
    }
    const { start } = row.diagnostic.range;
    void workspace.navigateTo(
      {
        path: row.path,
        ...(connection.worktree !== undefined && { worktree: connection.worktree }),
        line: start.line + 1,
        column: start.character + 1,
      },
      { record: true },
    );
  };
  const onKeyDown = (e: KeyboardEvent) => {
    const row = rows[selected];
    switch (e.key) {
      case "ArrowDown":
        select(selected < 0 ? 0 : Math.min(rows.length - 1, selected + 1));
        break;
      case "ArrowUp":
        select(Math.max(0, selected - 1));
        break;
      case "ArrowRight":
        if (row?.kind === "file" && row.collapsed) toggle(row.path, true);
        else select(selected + 1);
        break;
      case "ArrowLeft":
        if (row?.kind === "file" && !row.collapsed) toggle(row.path, false);
        else if (row?.kind === "diagnostic")
          select(rows.findIndex((r) => r.kind === "file" && r.path === row.path));
        break;
      case "Enter":
        if (row) activate(row);
        break;
      default:
        return;
    }
    e.preventDefault();
    e.stopPropagation();
  };
  const toggleSeverity = (severity: LspSeverity) =>
    setShown((prev) => {
      const next = new Set(prev);
      if (next.has(severity)) next.delete(severity);
      else next.add(severity);
      return next;
    });

  return (
    <div
      data-tool-window={api.id}
      tabIndex={-1}
      className="flex h-full flex-col bg-ui-panel text-ui-foreground outline-none"
    >
      <div className="flex h-[26px] flex-none items-center gap-2 border-b border-ui-border px-2 text-[12px]">
        <div role="tablist" aria-label={T.title} className="flex flex-none items-center gap-1">
          {(["file", "project"] as const).map((s) => (
            <button
              key={s}
              type="button"
              role="tab"
              aria-selected={scope === s}
              data-problems-scope={s}
              onClick={() => setScope(s)}
              className={`rounded-sm px-1.5 ${scope === s ? "bg-ui-active" : "hover:bg-ui-hover"}`}
            >
              {s === "file" ? T.currentFile : T.project}
            </button>
          ))}
        </div>
        <div className="flex flex-none items-center gap-1" aria-label={T.filter} role="group">
          {SEVERITIES.map((severity) => (
            <button
              key={severity}
              type="button"
              aria-pressed={shown.has(severity)}
              data-problems-filter={severity}
              title={T.severity[severity]}
              aria-label={`${T.severity[severity]} ${counts[severity]}`}
              onClick={() => toggleSeverity(severity)}
              className={`flex items-center gap-0.5 rounded-sm px-1 ${
                shown.has(severity) ? "bg-ui-active" : "text-ui-muted-foreground hover:bg-ui-hover"
              }`}
            >
              <span aria-hidden className={GLYPH_CLASS[severity]}>
                {GLYPHS[severity]}
              </span>
              {counts[severity]}
            </button>
          ))}
        </div>
        {truncated && (
          <span
            data-testid="problems-truncated"
            className="min-w-0 truncate text-ui-muted-foreground"
          >
            {T.truncated(FILE_LIMIT, PROJECT_LIMIT)}
          </span>
        )}
      </div>
      {rows.length === 0 ? (
        <p className="p-2 text-[12px] text-ui-muted-foreground">
          {data.loading ? T.loading : scope === "file" && active === undefined ? T.noFile : T.empty}
        </p>
      ) : (
        <div
          ref={scrollRef}
          role="tree"
          aria-label={T.title}
          aria-activedescendant={selected >= 0 ? `problem-${selected}` : undefined}
          tabIndex={0}
          data-keymap-context="problems"
          onKeyDown={onKeyDown}
          className="min-h-0 flex-1 overflow-auto text-[12px] outline-none"
        >
          <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((item) => {
              const row = rows[item.index];
              if (!row) return null;
              return (
                <ProblemRowView
                  key={item.key}
                  row={row}
                  index={item.index}
                  start={item.start}
                  selected={item.index === selected}
                  strings={T}
                  onSelect={() => {
                    setSelectedKey(rowKey(row));
                    if (row.kind === "file") toggle(row.path);
                  }}
                  onActivate={() => row.kind === "diagnostic" && activate(row)}
                />
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

function ProblemRowView({
  row,
  index,
  start,
  selected,
  strings: T,
  onSelect,
  onActivate,
}: {
  row: ProblemRow;
  index: number;
  start: number;
  selected: boolean;
  strings: DiagnosticStringsUi;
  onSelect: () => void;
  onActivate: () => void;
}) {
  const common = {
    id: `problem-${index}`,
    role: "treeitem",
    "aria-selected": selected,
    "data-problem-row": rowKey(row),
    onClick: onSelect,
    onDoubleClick: onActivate,
    style: { transform: `translateY(${start}px)`, height: ROW_HEIGHT },
  } as const;
  const base = `absolute top-0 left-0 flex w-full cursor-default items-center gap-1 pr-2 ${
    selected ? "bg-ui-active" : "hover:bg-ui-hover"
  }`;
  if (row.kind === "file")
    return (
      <div {...common} aria-level={1} aria-expanded={!row.collapsed} className={`${base} pl-1`}>
        {row.collapsed ? (
          <ChevronRight size={12} aria-hidden className="flex-none" />
        ) : (
          <ChevronDown size={12} aria-hidden className="flex-none" />
        )}
        <File size={13} aria-hidden className="flex-none" />
        <span className="flex-none">{baseName(row.path)}</span>
        <span className="min-w-0 truncate text-ui-muted-foreground" title={row.path}>
          {dirName(row.path)}
        </span>
        <span className="ml-auto flex-none text-[11px] text-ui-muted-foreground">
          {T.fileCount(row.count, row.truncated)}
        </span>
      </div>
    );
  const d = row.diagnostic;
  const where = `${d.range.start.line + 1}:${d.range.start.character + 1}`;
  const code = [d.source, d.code].filter((s): s is string => !!s).join(" ");
  return (
    <div
      {...common}
      aria-level={2}
      aria-label={`${T.severity[d.severity]} ${where} ${d.message}`}
      data-severity={d.severity}
      className={`${base} pl-8`}
    >
      <span aria-hidden className={`w-3 flex-none text-center ${GLYPH_CLASS[d.severity]}`}>
        {GLYPHS[d.severity]}
      </span>
      <span className="min-w-0 truncate" title={d.message}>
        {d.message.split("\n")[0]}
      </span>
      <span className="flex-none truncate text-ui-muted-foreground">
        {T.source(d.server, code || null)}
      </span>
      <span className="ml-auto flex-none text-[11px] text-ui-muted-foreground">{where}</span>
    </div>
  );
}
