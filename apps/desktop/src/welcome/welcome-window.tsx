// S1 웰컴 / 프로젝트 런처(10-ui §2.1). 프로젝트 목록 · 검색 · 새 프로젝트 · 폴더 열기 · 폴더 드롭, 그리고 파일로 실행했을 때의
// 프로젝트 찾기(S2) → 생성 시트(S3) 흐름을 이 창이 이끈다. 네이티브와 주고받는 것은 native.ts 한 곳이다.
//
// 키보드: 목록은 cmdk 라 ↑↓ · Enter(열기)를 cmdk 가 받는다 — 한글 조합을 확정하는 Enter 는 열지 않는다. 나머지(새 프로젝트 ·
// 폴더 열기 · 목록에서 제거 · 파일 관리자에서 보기)는 키맵 액션이고 기본 키는 extensions/keymap-default/keymaps/default.toml.
// 목록에서 제거(⌘⌫)는 검색 칸이 비어 있을 때만 키를 가져간다 — 검색어를 지우려던 ⌘⌫ 가 프로젝트를 지우지 않게.
// 클릭은 고르기만 하고 여는 것은 더블클릭이다.

import type { Project } from "@pitwall/rpc-client";
import * as ContextMenu from "@radix-ui/react-context-menu";
import { useQueryClient } from "@tanstack/react-query";
import { Command } from "cmdk";
import { FolderOpen, Plus, Star } from "lucide-react";
import {
  type MouseEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { useKeymap, useKeymapPopup, useRegisterActions } from "../keymap/keymap-provider";
import { useHost, useRpcQuery } from "../rpc/host";
import { rpcQueryKey } from "../rpc/queries";
import { searchFilter } from "../ui/search-popup";
import { welcomeActions } from "./actions";
import { CreateProjectSheet } from "./create-project";
import { FindProjectPopup } from "./find-project";
import type { FindProjectRequest, OpenProjectWindowArgs, WelcomeNative } from "./native";
import {
  errorText,
  existingProjectFor,
  isStorageUnavailable,
  openArgsFor,
  isRemote,
  parentDir,
  sortProjects,
  storageBanner,
} from "./projects";
import { STRINGS } from "./strings";

type Flow =
  | { kind: "idle" }
  | { kind: "find"; request: FindProjectRequest }
  /** S2 의 「폴더 직접 선택…」 대화상자가 떠 있다. 취소하면 S2 로 돌아간다. */
  | { kind: "picking"; request: FindProjectRequest }
  | { kind: "create"; root: string; origin?: FindProjectRequest };

const BUTTON =
  "flex items-center gap-1.5 rounded-sm border border-ui-border bg-ui-panel px-2 py-0.5 hover:bg-ui-hover disabled:opacity-50";

function Kbd({ text }: { text: string | undefined }) {
  return text ? <kbd className="font-sans text-[11px] text-ui-muted-foreground">{text}</kbd> : null;
}

export function WelcomeWindow({ native }: { native: WelcomeNative }) {
  const { client, key } = useHost();
  const queryClient = useQueryClient();
  const { platform, shortcuts } = useKeymap();
  const projects = useRpcQuery("project.list", null);
  const storage = useRpcQuery("project.storage", null);
  const banner = storageBanner(storage.data);
  const readOnly = banner?.readOnly === true;
  const list = useMemo(() => sortProjects(projects.data ?? []), [projects.data]);

  const [chosen, setChosen] = useState("");
  // 고른 줄이 목록에 없으면(처음 · 제거 · 다른 창에서 지움) 첫 줄. cmdk 가 스스로 첫 줄을 고르게 두지 않는다 — cmdk 는 자기 선택을
  // 바꿀 때 포커스가 어느 cmdk 입력 칸에든 있으면 자기 입력 칸으로 가져가서, S2 팝업이 떠 있는 동안 목록이 로드되면 팝업의
  // 포커스를 뺏는다.
  const selected = list.some((p) => p.id === chosen) ? chosen : (list[0]?.id ?? "");
  const [search, setSearch] = useState("");
  const [flow, setFlow] = useState<Flow>({ kind: "idle" });
  const [error, setError] = useState<string>();
  /** 오류는 아니지만 알릴 것(만든 프로젝트가 열려던 파일을 포함하지 않음 등). */
  const [notice, setNotice] = useState<string>();
  const [dropping, setDropping] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  useKeymapPopup(menuOpen);

  const fail = useCallback(
    (what: string) => (e: unknown) => setError(STRINGS.errors.action(what, errorText(e))),
    [],
  );
  const refreshList = useCallback(
    () => queryClient.invalidateQueries({ queryKey: rpcQueryKey(key, "project.list") }),
    [queryClient, key],
  );
  // 쓰기가 STORAGE_UNAVAILABLE 로 거절되면 저장소 상태가 바뀐 것이다 — 배너 · 쓰기 비활성을 창 포커스를 기다리지 않고 맞춘다
  // (docs/design/03-architecture.md §5-0).
  const writeFailed = useCallback(
    (what: string) => (e: unknown) => {
      fail(what)(e);
      if (!isStorageUnavailable(e)) return;
      void queryClient.invalidateQueries({ queryKey: rpcQueryKey(key, "project.storage") });
      void refreshList();
    },
    [fail, queryClient, key, refreshList],
  );

  const openWindow = useCallback(
    (args: OpenProjectWindowArgs) => {
      setError(undefined);
      setNotice(undefined);
      return native.openProjectWindow(args).catch(fail(STRINGS.errors.open));
    },
    [native, fail],
  );

  /** 고르거나 끌어다 놓은 폴더: 이미 그 루트의 프로젝트면 열고, 아니면 S3. */
  const openFolder = useCallback(
    async (path: string) => {
      if (!client) return;
      setError(undefined);
      const resolution = await client.call("project.resolve", { path }).catch((e: unknown) => {
        fail(STRINGS.errors.open)(e);
        return undefined;
      });
      if (resolution === undefined) return;
      const existing = existingProjectFor(resolution);
      if (existing) await openWindow(existing);
      else setFlow({ kind: "create", root: path });
    },
    [client, fail, openWindow],
  );

  const pickAndOpenFolder = useCallback(async () => {
    const picked = await native.pickFolder().catch((e: unknown) => {
      fail(STRINGS.errors.pickFolder)(e);
      return null;
    });
    if (picked !== null) await openFolder(picked);
  }, [native, fail, openFolder]);

  // 파일로 실행했는데 프로젝트를 못 찾은 요청(F2). 창이 뜰 때와 알림을 받을 때 꺼낸다.
  // 꺼낸 요청은 effect 가 해제된 뒤에 와도 버리지 않는다 — Rust 쪽은 꺼낼 때 비우므로 버리면 다시 꺼낼 수 없다. StrictMode 는
  // effect 를 붙였다 떼고 다시 붙이는데, 첫 take 가 요청을 가져가고 둘째는 null 을 받는다. 컴포넌트 상태는 그 사이에도 남으므로
  // 첫 응답을 그대로 넣는다(정말 언마운트됐으면 setState 는 아무 일도 하지 않는다 — 창이 닫힌 것이다).
  useEffect(() => {
    const take = () => {
      native
        .takePendingFindProject()
        .then((request) => {
          if (request) setFlow({ kind: "find", request });
        })
        .catch(fail(STRINGS.errors.findProject));
    };
    take();
    return native.onFindProjectRequested(take);
  }, [native, fail]);

  /** 경로를 담은 등록 프로젝트를 그 파일과 함께 연다. 그 경로를 담은 프로젝트가 없으면 거짓. */
  const openPathInProject = useCallback(
    async (request: FindProjectRequest): Promise<boolean> => {
      if (!client) return false;
      const resolution = await client.call("project.resolve", { path: request.path });
      const args = resolution && openArgsFor(resolution, resolution.project.id, request);
      if (!args) return false;
      await openWindow(args);
      return true;
    },
    [client, openWindow],
  );

  /** 끌어다 놓은 것. 폴더는 「폴더 열기」와 같고, 파일은 F2(`pitwall <파일>`)와 같은 갈래다 — 프로젝트 안이면 열고 아니면 S2. */
  const openDropped = useCallback(
    async (path: string) => {
      setError(undefined);
      const kind = await native.pathKind(path).catch(() => "dir" as const);
      if (kind === "dir") return openFolder(path);
      const request = { path };
      const opened = await openPathInProject(request).catch((e: unknown) => {
        fail(STRINGS.errors.open)(e);
        return true;
      });
      if (!opened) setFlow({ kind: "find", request });
    },
    [native, openFolder, openPathInProject, fail],
  );

  useEffect(
    () =>
      native.onDragDrop((event) => {
        if (event.kind !== "drop") return setDropping(event.kind === "enter");
        setDropping(false);
        // 여럿을 놓으면 첫 번째만 연다 — 멀티 루트(F1-6)가 생기면 다시 본다.
        const [first] = event.paths;
        if (first !== undefined) void openDropped(first);
      }),
    [native, openDropped],
  );

  const byId = (id: string) => list.find((p) => p.id === id);
  const selectedProject = useRef<() => Project | undefined>(() => undefined);
  selectedProject.current = () => byId(selected);
  const searchEmpty = useRef(true);
  searchEmpty.current = search === "";
  const writable = useRef(false);
  writable.current = !readOnly && client !== undefined;

  const removeProject = useCallback(
    async (project: Project) => {
      if (!client) return;
      setError(undefined);
      await client
        .call("project.delete", { id: project.id })
        .then(refreshList)
        .catch(writeFailed(STRINGS.errors.remove));
    },
    [client, refreshList, writeFailed],
  );
  const reveal = useCallback(
    (project: Project) => {
      const root = project.roots[0];
      if (root !== undefined) void native.reveal(root).catch(fail(STRINGS.errors.reveal));
    },
    [native, fail],
  );

  const actions = useMemo(
    () =>
      welcomeActions({
        platform,
        selected: () => selectedProject.current(),
        searchEmpty: () => searchEmpty.current,
        writable: () => writable.current,
        newProject: () => setFlow({ kind: "create", root: "" }),
        openFolder: pickAndOpenFolder,
        open: (p) => openWindow({ projectId: p.id }),
        reveal,
        remove: removeProject,
      }),
    [platform, pickAndOpenFolder, openWindow, reveal, removeProject],
  );
  useRegisterActions(actions);

  // 클릭은 고르기만 한다(cmdk 는 클릭을 onSelect 로 보낸다). 키보드 Enter 의 onSelect 만 연다.
  const pointerSelect = useRef(false);
  const [menuTarget, setMenuTarget] = useState<Project>();
  const onContextMenu = (e: MouseEvent) => {
    const id = (e.target as Element).closest("[cmdk-item]")?.getAttribute("data-value");
    const project = id ? byId(id) : undefined;
    if (!project) {
      e.preventDefault();
      return;
    }
    setChosen(project.id);
    setMenuTarget(project);
  };

  const T = STRINGS;
  return (
    <div className="relative flex min-h-0 flex-1">
      <nav
        aria-label={T.nav.label}
        className="flex w-[150px] flex-none flex-col gap-0.5 border-r border-ui-border p-1.5"
      >
        <button
          type="button"
          aria-current="page"
          className="rounded-sm bg-ui-active px-2 py-0.5 text-left"
        >
          {T.nav.projects}
        </button>
        {[T.nav.remoteHosts, T.nav.extensions].map((label) => (
          <button
            key={label}
            type="button"
            disabled
            title={T.nav.notYet}
            className="rounded-sm px-2 py-0.5 text-left text-ui-muted-foreground"
          >
            {label}
          </button>
        ))}
      </nav>
      <section className="flex min-w-0 flex-1 flex-col">
        {banner && (
          <div
            role="status"
            className="flex-none border-b border-ui-accent bg-ui-panel px-3 py-1 text-[12px]"
          >
            {banner.text}
          </div>
        )}
        <Command
          label={T.list.label}
          filter={searchFilter}
          value={selected}
          onValueChange={setChosen}
          disablePointerSelection
          loop
          className="flex min-h-0 flex-1 flex-col"
        >
          <Command.Input
            autoFocus
            value={search}
            onValueChange={setSearch}
            placeholder={T.list.search}
            className="flex-none border-b border-ui-border bg-transparent px-3 py-1.5 outline-none placeholder:text-ui-muted-foreground"
          />
          <ContextMenu.Root onOpenChange={setMenuOpen}>
            <ContextMenu.Trigger asChild>
              <div
                className="min-h-0 flex-1 overflow-y-auto"
                onContextMenu={onContextMenu}
                // 검색 칸의 포커스를 지킨다 — 클릭한 뒤에도 ↑↓ · Enter 가 목록으로 간다.
                onMouseDown={(e) => e.preventDefault()}
              >
                <Command.List className="py-1">
                  <Command.Empty className="px-3 py-2 text-ui-muted-foreground">
                    {list.length === 0 ? T.list.empty : T.list.noMatch}
                  </Command.Empty>
                  {list.map((project) => (
                    <Command.Item
                      key={project.id}
                      value={project.id}
                      keywords={[project.name, ...project.roots, ...project.tags]}
                      onPointerDown={() => (pointerSelect.current = true)}
                      onSelect={() => {
                        if (pointerSelect.current) pointerSelect.current = false;
                        else void openWindow({ projectId: project.id });
                      }}
                      onDoubleClick={() => void openWindow({ projectId: project.id })}
                      className="flex h-[22px] cursor-default items-center gap-2 px-3 data-[selected=true]:bg-ui-active"
                    >
                      <Star
                        aria-label={project.favorite ? T.list.favorite : undefined}
                        aria-hidden={!project.favorite}
                        className={`size-3 flex-none ${project.favorite ? "fill-current text-ui-accent" : "invisible"}`}
                      />
                      <span className="w-[180px] flex-none truncate">
                        {project.name}
                        {isRemote(project) && (
                          <span className="ml-1 text-ui-muted-foreground">({T.list.remote})</span>
                        )}
                      </span>
                      {/* 경로는 앞에서 자른다 — 프로젝트를 가르는 것은 끝 조각이다. rtl 로 말줄임을 앞에 두고 글자 순서는 LRM 으로 지킨다. */}
                      <span
                        dir="rtl"
                        className="min-w-0 flex-1 truncate text-left text-ui-muted-foreground"
                        title={project.roots[0]}
                      >
                        {`\u200e${isRemote(project) ? `${project.host}:` : ""}${project.roots[0]}\u200e`}
                      </span>
                      {project.tags.map((tag) => (
                        <span
                          key={tag}
                          className="flex-none rounded-sm border border-ui-border px-1 text-[11px]"
                        >
                          {tag}
                        </span>
                      ))}
                    </Command.Item>
                  ))}
                </Command.List>
              </div>
            </ContextMenu.Trigger>
            <ContextMenu.Portal>
              <ContextMenu.Content className="min-w-[180px] rounded-md border border-ui-border bg-ui-panel py-1 text-ui-foreground shadow-lg">
                {menuTarget && (
                  <>
                    <MenuItem onSelect={() => void openWindow({ projectId: menuTarget.id })}>
                      {T.menu.open}
                    </MenuItem>
                    <MenuItem onSelect={() => reveal(menuTarget)}>
                      {T.menu.reveal(platform)}
                    </MenuItem>
                    <ContextMenu.Separator className="my-1 h-px bg-ui-border" />
                    {/* 단축키를 적지 않는다 — 메뉴가 열린 동안은 창 단축키가 꺼져(useKeymapPopup) ⌘⌫ 가 돌지 않는다. */}
                    <MenuItem disabled={readOnly} onSelect={() => void removeProject(menuTarget)}>
                      {T.menu.remove}
                    </MenuItem>
                  </>
                )}
              </ContextMenu.Content>
            </ContextMenu.Portal>
          </ContextMenu.Root>
        </Command>
        {error && (
          <div
            role="alert"
            className="flex-none truncate border-t border-ui-accent px-3 py-0.5 text-[12px]"
            title={error}
          >
            {error}
          </div>
        )}
        {notice && (
          <div
            role="status"
            className="flex-none truncate border-t border-ui-border px-3 py-0.5 text-[12px] text-ui-muted-foreground"
            title={notice}
          >
            {notice}
          </div>
        )}
        <div className="flex flex-none gap-2 border-t border-ui-border px-3 py-2">
          <button
            type="button"
            className={BUTTON}
            disabled={readOnly}
            onClick={() => setFlow({ kind: "create", root: "" })}
          >
            <Plus className="size-3.5" />
            {T.buttons.newProject}
            <Kbd text={shortcuts("project.new")[0]?.text} />
          </button>
          <button type="button" className={BUTTON} onClick={() => void pickAndOpenFolder()}>
            <FolderOpen className="size-3.5" />
            {T.buttons.openFolder}
            <Kbd text={shortcuts("project.openFolder")[0]?.text} />
          </button>
        </div>
      </section>
      {dropping && (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center border-2 border-dashed border-ui-accent bg-ui-background/70">
          {T.drop}
        </div>
      )}
      {flow.kind === "find" && (
        <FindProjectPopup
          request={flow.request}
          onPick={(root) => setFlow({ kind: "create", root, origin: flow.request })}
          onPickFolder={() => {
            const request = flow.request;
            setFlow({ kind: "picking", request });
            native
              .pickFolder(parentDir(request.path))
              .then((root) =>
                setFlow(
                  root === null
                    ? { kind: "find", request }
                    : { kind: "create", root, origin: request },
                ),
              )
              .catch((e: unknown) => {
                fail(STRINGS.errors.pickFolder)(e);
                setFlow({ kind: "find", request });
              });
          }}
          onCancel={() => setFlow({ kind: "idle" })}
        />
      )}
      {flow.kind === "create" && (
        <CreateProjectSheet
          key={flow.root}
          initialRoot={flow.root}
          readOnly={readOnly}
          pickFolder={native.pickFolder}
          onWriteError={writeFailed(STRINGS.errors.create)}
          onCancel={() => setFlow({ kind: "idle" })}
          onCreated={async (project) => {
            const origin = flow.origin;
            setFlow({ kind: "idle" });
            void refreshList();
            const args = origin && (await originArgs(origin, project.id));
            await openWindow(args ?? { projectId: project.id });
            if (origin && !args) setNotice(STRINGS.notices.fileNotInProject(origin.path));
          }}
        />
      )}
    </div>
  );

  /**
   * S2 에서 왔으면 원래 경로를 다시 해석해, 만든 프로젝트가 그 경로를 포함할 때만 활성 워크트리(F2-5)와 열 파일을 넘긴다.
   * 워크트리 꼭대기(`top`)로 다시 해석하지 않는다 — 프로젝트 루트가 메인 체크아웃의 하위 폴더면 꼭대기는 그 프로젝트 밖이다.
   */
  async function originArgs(
    origin: FindProjectRequest,
    projectId: string,
  ): Promise<OpenProjectWindowArgs | undefined> {
    const resolution = client
      ? await client.call("project.resolve", { path: origin.path }).catch(() => null)
      : null;
    return openArgsFor(resolution, projectId, origin);
  }
}

function MenuItem({
  children,
  disabled,
  onSelect,
}: {
  children: ReactNode;
  disabled?: boolean;
  onSelect(): void;
}) {
  return (
    <ContextMenu.Item
      disabled={disabled}
      onSelect={onSelect}
      className="flex h-[24px] cursor-default items-center gap-3 px-3 outline-none data-[disabled]:text-ui-muted-foreground data-[highlighted]:bg-ui-active"
    >
      {children}
    </ContextMenu.Item>
  );
}
