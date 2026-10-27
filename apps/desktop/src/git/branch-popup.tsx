// 브랜치 팝업(⌃⇧B, 10-ui.md 「다이얼로그 · 팝업 목록」 「Git」 행, 13-m2-plan.md M2-13)과 VCS 작업 팝업(⌃V).
//
// 브랜치 팝업은 두 단계다. 첫 화면은 검색 칸 + 「새 브랜치…」 · 즐겨찾기 · 로컬 · 원격 · 태그 · 워크트리 묶음. ref 를 고르면(Enter ·
// 클릭) 그 ref 의 동작 목록(체크아웃 · 여기서 새 브랜치 · 지금 것과 비교 · merge · rebase · 이름 바꾸기 · 삭제 · 즐겨찾기)으로 바뀌고,
// Esc · ⌫(빈 칸) · 「뒤로」 가 첫 화면으로 돌아간다. 동작은 branch-ops.ts 가 한다.
//
// - 즐겨찾기(F5-5)는 프로젝트 상태(layout/project-state.ts 의 `favoriteBranches`, ref 전체 이름)에 둔다 — 행의 별 단추나 동작 목록의
//   「즐겨찾기」 로 바꾼다. 지금 없는 ref 의 이름은 건너뛴다.
// - 워크트리 묶음은 `git.worktrees` 다(F5-15 의 목록 · 전환, 결정 M2-D5). 고르면 M1-25 의 워크트리 전환 경로(창의 열기 요청 →
//   워크트리 세션: 탭 묶음 저장 · 미저장 확인 · 복원)로 바꾼다. 다른 저장소 체크아웃의 등록 프로젝트처럼 이 창이 그리로 바꿀 수 없는
//   것은 고를 수 없게 보인다.
// - VCS 작업 팝업은 이 창에 등록된 git 액션(`git.*` · 커밋 · Git 툴 윈도우)을 단축키와 함께 보인다 — 다른 행이 액션을 더하면 저절로
//   늘어난다.

import { matchScore } from "@pitwall/keymap";
import type { GitBranch, GitWorktree } from "@pitwall/rpc-client";
import * as Dialog from "@radix-ui/react-dialog";
import { Command } from "cmdk";
import { ArrowLeft, Star } from "lucide-react";
import { type ReactNode, useState } from "react";

import { useKeymap, useKeymapPopup, useRegistryVersion } from "../keymap/keymap-provider";
import { useActiveWorktree } from "../open-with/active-worktree";
import { useRpcQuery } from "../rpc/host";
import { isComposingKey } from "../ui/ime";
import { SEARCH_ITEM_CLASS } from "../ui/search-popup";
import { type BranchOps, type BranchRef, allRefs, refName, shortName } from "./branch-ops";
import { useBranch } from "./branch-state";
import { headName } from "./branch-status";
import { type BranchStrings, useBranchStrings } from "./branch-strings";
import { canCompareCommits } from "./compare-action";

export interface BranchFavorites {
  readonly list: readonly string[];
  toggle(refName: string): void;
}

const GROUP_CLASS =
  "[&_[cmdk-group-heading]]:px-3 [&_[cmdk-group-heading]]:pt-1 [&_[cmdk-group-heading]]:text-[11px] [&_[cmdk-group-heading]]:text-ui-muted-foreground";

/**
 * 줄의 낱말(`keywords`)로만 거른다 — 값(`local:refs/heads/…`)은 같은 이름이 여러 묶음에 있어도 겹치지 않게 붙인 것이라 비교에
 * 넣지 않는다. 낱말이 없는 줄(안내)은 늘 보인다.
 */
export function keywordFilter(_value: string, search: string, keywords?: string[]): number {
  if (!keywords || keywords.length === 0) return 1;
  return Math.max(...keywords.map((k) => matchScore(k, search)));
}

/**
 * 화면 상단의 검색 팝업(ui/search-popup.tsx 와 같은 겉모양 · 키). 다른 점은 낱말로만 거르는 것과, Esc 를 `onEscape` 가 먼저 받는
 * 것(동작 목록에서 첫 화면으로 돌아간다). 조합을 취소하는 Esc 는 팝업을 닫지 않는다(ui/ime.ts).
 */
function PopupShell({
  kind,
  open,
  onOpenChange,
  label,
  title,
  placeholder,
  empty,
  query,
  onQueryChange,
  onEscape,
  onEmptyBackspace,
  children,
}: {
  /** `data-git-popup` — 어느 팝업 · 단계인가(시험이 찾는다). */
  kind: string;
  open: boolean;
  onOpenChange(open: boolean): void;
  label: string;
  title: ReactNode;
  placeholder: string;
  empty: ReactNode;
  query: string;
  onQueryChange(query: string): void;
  /** 참을 돌려주면 Esc 가 팝업을 닫지 않는다. */
  onEscape?: () => boolean;
  onEmptyBackspace?: () => void;
  children: ReactNode;
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay cmdk-overlay="" className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          cmdk-dialog=""
          data-git-popup={kind}
          aria-label={label}
          aria-describedby={undefined}
          onEscapeKeyDown={(e) => {
            if (isComposingKey(e) || onEscape?.()) e.preventDefault();
          }}
          className="fixed top-[15%] left-1/2 w-[520px] max-w-[90vw] -translate-x-1/2 overflow-hidden rounded-md border border-ui-border bg-ui-panel text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="sr-only">{label}</Dialog.Title>
          <Command label={label} filter={keywordFilter} loop>
            <div className="truncate border-b border-ui-border px-3 py-1 text-center text-[12px] text-ui-muted-foreground">
              {title}
            </div>
            <Command.Input
              autoFocus
              value={query}
              onValueChange={onQueryChange}
              onKeyDown={(e) => {
                if (
                  e.key === "Backspace" &&
                  query === "" &&
                  onEmptyBackspace &&
                  !isComposingKey(e.nativeEvent)
                ) {
                  e.preventDefault();
                  onEmptyBackspace();
                }
              }}
              placeholder={placeholder}
              className="w-full border-b border-ui-border bg-transparent px-3 py-2 outline-none placeholder:text-ui-muted-foreground"
            />
            <Command.List className="max-h-[360px] overflow-y-auto py-1">
              <Command.Empty className="px-3 py-2 text-ui-muted-foreground">{empty}</Command.Empty>
              {children}
            </Command.List>
          </Command>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function Muted({ children }: { children: ReactNode }) {
  return <span className="flex-none text-[11px] text-ui-muted-foreground">{children}</span>;
}

function FavoriteButton({ T, on, onToggle }: { T: BranchStrings; on: boolean; onToggle(): void }) {
  const label = on ? T.popup.unfavorite : T.popup.favorite;
  return (
    <button
      type="button"
      tabIndex={-1}
      aria-label={label}
      aria-pressed={on}
      title={label}
      data-favorite-toggle=""
      // 입력 칸의 포커스와 고른 줄을 지킨다 — 별을 눌러도 그 줄을 고르지 않는다.
      onMouseDown={(e) => e.preventDefault()}
      onPointerDown={(e) => e.stopPropagation()}
      onClick={(e) => {
        e.stopPropagation();
        onToggle();
      }}
      className="flex-none rounded-sm p-0.5 hover:bg-ui-hover"
    >
      <Star
        size={11}
        aria-hidden
        className={on ? "fill-current text-ui-accent" : "text-ui-muted-foreground"}
      />
    </button>
  );
}

function RefItem({
  T,
  ref: r,
  value,
  favorites,
  onChoose,
}: {
  T: BranchStrings;
  ref: BranchRef;
  value: string;
  favorites: BranchFavorites;
  onChoose(): void;
}) {
  const name = shortName(r);
  const current = r.kind === "local" && r.branch.head;
  const track = r.kind === "local" ? r.branch.upstream?.track : undefined;
  return (
    <Command.Item
      value={value}
      keywords={[name]}
      onSelect={onChoose}
      data-ref={refName(r)}
      className={SEARCH_ITEM_CLASS}
    >
      <span className={`min-w-0 flex-1 truncate ${current ? "font-semibold" : ""}`}>{name}</span>
      {track && (track.ahead > 0 || track.behind > 0) && (
        <Muted>{`${track.ahead > 0 ? `↑${track.ahead}` : ""}${track.behind > 0 ? `↓${track.behind}` : ""}`}</Muted>
      )}
      {r.kind === "local" && r.branch.upstream && <Muted>{r.branch.upstream.name}</Muted>}
      {current && <Muted>{T.popup.current}</Muted>}
      {r.kind !== "tag" && (
        <FavoriteButton
          T={T}
          on={favorites.list.includes(refName(r))}
          onToggle={() => favorites.toggle(refName(r))}
        />
      )}
    </Command.Item>
  );
}

function WorktreeItem({
  T,
  worktree: w,
  onSwitch,
}: {
  T: BranchStrings;
  worktree: GitWorktree;
  onSwitch(): void;
}) {
  const head =
    w.head === null ? "" : w.head.kind === "detached" ? w.head.commit.slice(0, 7) : w.head.name;
  const disabled = w.current || w.switchTo === null;
  return (
    <Command.Item
      value={`worktree:${w.path}`}
      keywords={[w.path, head]}
      disabled={disabled}
      onSelect={onSwitch}
      title={w.path}
      className={SEARCH_ITEM_CLASS}
    >
      <span className="flex-none">{head}</span>
      <span
        dir="rtl"
        className="min-w-0 flex-1 truncate text-left text-[11px] text-ui-muted-foreground"
      >
        {`‎${w.path}‎`}
      </span>
      {w.main && <Muted>{T.popup.worktreeMain}</Muted>}
      {w.current && <Muted>{T.popup.worktreeCurrent}</Muted>}
      {w.missing && <Muted>{T.popup.worktreeMissing}</Muted>}
      {w.locked && <Muted>{T.popup.worktreeLocked}</Muted>}
      {!w.current && !w.missing && w.switchTo === null && <Muted>{T.popup.worktreeOther}</Muted>}
    </Command.Item>
  );
}

/** 첫 화면: 새 브랜치 · 즐겨찾기 · 로컬 · 원격 · 태그 · 워크트리. */
function RefList({
  projectId,
  favorites,
  onChoose,
  onCreate,
  onSwitchWorktree,
}: {
  projectId: string;
  favorites: BranchFavorites;
  onChoose(ref: BranchRef): void;
  onCreate(): void;
  onSwitchWorktree(worktree: GitWorktree): void;
}) {
  const T = useBranchStrings();
  const { refs, error } = useBranch();
  const worktree = useActiveWorktree();
  const worktrees = useRpcQuery(
    "git.worktrees",
    { projectId, worktree: worktree ?? null },
    { staleTime: 0 },
  );
  if (!refs)
    return (
      <Command.Item disabled value="note:loading" className={SEARCH_ITEM_CLASS}>
        {error ? T.popup.failed(error) : T.popup.loading}
      </Command.Item>
    );
  if (refs.workTree === null || refs.unsupported)
    return (
      <Command.Item disabled value="note:none" className={SEARCH_ITEM_CLASS}>
        {refs.unsupported ? T.status.unsupportedDetail : T.popup.noRepository}
      </Command.Item>
    );
  const every = allRefs(refs);
  const favored = favorites.list.flatMap((name) => every.filter((r) => refName(r) === name));
  const item = (r: BranchRef, group: string) => (
    <RefItem
      key={`${group}:${refName(r)}`}
      T={T}
      ref={r}
      value={`${group}:${refName(r)}`}
      favorites={favorites}
      onChoose={() => onChoose(r)}
    />
  );
  const linked = worktrees.data?.worktrees ?? [];
  return (
    <>
      <Command.Item
        value="action:new"
        keywords={[T.popup.newBranch]}
        onSelect={onCreate}
        className={SEARCH_ITEM_CLASS}
      >
        {T.popup.newBranch}
      </Command.Item>
      {favored.length > 0 && (
        <Command.Group heading={T.popup.groups.favorites} className={GROUP_CLASS}>
          {favored.map((r) => item(r, "favorite"))}
        </Command.Group>
      )}
      {refs.branches.length > 0 && (
        <Command.Group heading={T.popup.groups.local} className={GROUP_CLASS}>
          {refs.branches.map((branch) => item({ kind: "local", branch }, "local"))}
        </Command.Group>
      )}
      {refs.remoteBranches.length > 0 && (
        <Command.Group heading={T.popup.groups.remote} className={GROUP_CLASS}>
          {refs.remoteBranches.map((branch) => item({ kind: "remote", branch }, "remote"))}
        </Command.Group>
      )}
      {refs.tags.length > 0 && (
        <Command.Group heading={T.popup.groups.tags} className={GROUP_CLASS}>
          {refs.tags.map((tag) => item({ kind: "tag", tag }, "tag"))}
        </Command.Group>
      )}
      {linked.length > 1 && (
        <Command.Group heading={T.popup.groups.worktrees} className={GROUP_CLASS}>
          {linked.map((w) => (
            <WorktreeItem key={w.path} T={T} worktree={w} onSwitch={() => onSwitchWorktree(w)} />
          ))}
        </Command.Group>
      )}
    </>
  );
}

/** 고른 ref 의 동작 목록. */
function RefMenu({
  chosen,
  ops,
  favorites,
  run,
  onBack,
}: {
  chosen: BranchRef;
  ops: BranchOps;
  favorites: BranchFavorites;
  /** 팝업을 닫고 동작을 한다. */
  run(step: () => void | Promise<void>): void;
  onBack(): void;
}) {
  const T = useBranchStrings();
  const M = T.popup.menu;
  const { registry } = useKeymap();
  useRegistryVersion(registry);
  const { refs } = useBranch();
  const current = (refs && headName(refs)) ?? "HEAD";
  const name = shortName(chosen);
  const isHead = chosen.kind === "local" && chosen.branch.head;
  const entry = (id: string, label: string, step: () => void | Promise<void>) => (
    <Command.Item
      key={id}
      value={`menu:${id}`}
      onSelect={() => run(step)}
      className={SEARCH_ITEM_CLASS}
    >
      {label}
    </Command.Item>
  );
  const local: GitBranch | undefined = chosen.kind === "local" ? chosen.branch : undefined;
  const favored = favorites.list.includes(refName(chosen));
  return (
    <>
      <Command.Item value="menu:back" onSelect={onBack} className={SEARCH_ITEM_CLASS}>
        <ArrowLeft size={11} aria-hidden />
        <span className="flex-1 truncate font-semibold">{name}</span>
        <Muted>{T.popup.back}</Muted>
      </Command.Item>
      {!isHead && entry("checkout", M.checkout, () => ops.checkout(chosen))}
      {entry("new", M.newFrom(name), () => ops.create(chosen))}
      {!isHead &&
        canCompareCommits(registry) &&
        entry("compare", M.compare(name, current), () => ops.compare(chosen))}
      {!isHead && entry("merge", M.merge(name, current), () => ops.merge(chosen))}
      {!isHead && entry("rebase", M.rebase(current, name), () => ops.rebase(chosen))}
      {local && entry("rename", M.rename, () => ops.rename(local))}
      {local && !isHead && entry("delete", M.delete, () => ops.remove(local))}
      {chosen.kind !== "tag" && (
        <Command.Item
          value="menu:favorite"
          onSelect={() => favorites.toggle(refName(chosen))}
          className={SEARCH_ITEM_CLASS}
        >
          {favored ? T.popup.unfavorite : T.popup.favorite}
        </Command.Item>
      )}
    </>
  );
}

export function BranchPopup({
  open,
  onOpenChange,
  projectId,
  ops,
  favorites,
  onSwitchWorktree,
}: {
  open: boolean;
  onOpenChange(open: boolean): void;
  projectId: string;
  ops: BranchOps;
  favorites: BranchFavorites;
  onSwitchWorktree(worktree: GitWorktree): void;
}) {
  const T = useBranchStrings();
  useKeymapPopup(open);
  const [chosen, setChosen] = useState<BranchRef>();
  const [query, setQuery] = useState("");
  const choose = (ref: BranchRef | undefined) => {
    setChosen(ref);
    setQuery("");
  };
  const close = () => {
    choose(undefined);
    onOpenChange(false);
  };
  // 닫히며 포커스가 이전 자리로 돌아간 뒤에 한다 — 동작이 대화상자를 띄울 수 있다.
  const run = (step: () => void | Promise<void>) => {
    close();
    setTimeout(() => void step(), 0);
  };
  return (
    <PopupShell
      kind={chosen ? "branch-menu" : "branches"}
      open={open}
      onOpenChange={(next) => (next ? onOpenChange(true) : close())}
      label={T.popup.label}
      title={chosen ? shortName(chosen) : T.popup.label}
      placeholder={T.popup.placeholder}
      empty={T.popup.empty}
      query={query}
      onQueryChange={setQuery}
      onEscape={() => {
        if (!chosen) return false;
        choose(undefined);
        return true;
      }}
      onEmptyBackspace={chosen ? () => choose(undefined) : undefined}
    >
      {chosen ? (
        <RefMenu
          key={refName(chosen)}
          chosen={chosen}
          ops={ops}
          favorites={favorites}
          run={run}
          onBack={() => choose(undefined)}
        />
      ) : (
        <RefList
          projectId={projectId}
          favorites={favorites}
          onChoose={choose}
          onCreate={() => run(() => ops.create())}
          onSwitchWorktree={(w) => run(() => onSwitchWorktree(w))}
        />
      )}
    </PopupShell>
  );
}

/** VCS 작업 팝업에 보일 액션인가. */
export function isVcsAction(id: string): boolean {
  return (
    (id.startsWith("git.") && id !== "git.vcsOperations") ||
    id === "toolWindow.git" ||
    id === "toolWindow.commit"
  );
}

export function VcsPopup({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange(open: boolean): void;
}) {
  const T = useBranchStrings();
  const { registry, shortcuts } = useKeymap();
  useRegistryVersion(registry);
  useKeymapPopup(open);
  const [query, setQuery] = useState("");
  const actions = registry
    .list()
    .filter((a) => isVcsAction(a.id) && registry.isEnabled(a.id) && !a.popup);
  return (
    <PopupShell
      kind="vcs"
      open={open}
      onOpenChange={(next) => {
        if (!next) setQuery("");
        onOpenChange(next);
      }}
      label={T.vcs.label}
      title={T.vcs.label}
      placeholder={T.vcs.placeholder}
      empty={T.vcs.empty}
      query={query}
      onQueryChange={setQuery}
    >
      {actions.map((action) => (
        <Command.Item
          key={action.id}
          value={action.id}
          keywords={[action.label, ...(action.keywords ?? [])]}
          onSelect={() => {
            setQuery("");
            onOpenChange(false);
            setTimeout(() => registry.run(action.id), 0);
          }}
          className={SEARCH_ITEM_CLASS}
        >
          <span className="flex-1 truncate">{action.label}</span>
          {shortcuts(action.id).map((k) => (
            <kbd key={k.text} className="font-sans text-[11px] text-ui-muted-foreground">
              {k.text}
            </kbd>
          ))}
        </Command.Item>
      ))}
    </PopupShell>
  );
}
