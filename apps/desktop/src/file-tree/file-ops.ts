// 파일 트리의 파일 조작(docs/design/12-m1-plan.md M1-21, F3-2, 10-ui §5.4): 새 파일 · 폴더, 이름 바꾸기(⇧F6), 삭제(휴지통), 드래그
// 이동 · ⌥+드래그 복사, Finder 에서 드롭 → 복사. 화면(우클릭 메뉴 · 대화상자 · 드래그)은 file-tree.tsx · file-dialogs.tsx 이고, 여기는
// 조작의 순서와 그 뒤 맞추기다.
//
// - 디스크는 Host 가 바꾼다(`fs.create` · `fs.rename` · `fs.copy` · `fs.trash`, crates/pw-fs/src/ops.rs). 같은 이름이 있으면 Host 가
//   아무것도 바꾸지 않고 `CONFLICT` 로 멈춘다 — 여기서는 그것을 「이미 있다」 알림으로 보일 뿐 미리 막지 않는다(확인과 쓰기 사이의
//   경합은 Host 가 지킨다).
// - 조작이 끝나면 감시 이벤트(`fs.changed`)를 기다리지 않고 맞춘다: 트리는 바뀐 부모를 다시 읽고(`touched` · `moved`), 열린 탭은
//   새 경로를 따라가거나(`moved`) 닫힌다(`deleted` — 미저장 탭은 「지워짐」 배너로 남는다). 뒤늦게 온 감시 이벤트는 같은 결과를
//   한 번 더 확인할 뿐이다.
// - 삭제는 늘 확인을 받는다(미저장 탭이 있으면 그 경고를 함께). 휴지통에 넣지 못한 항목(`unavailable`)은 「영구 삭제」 를 한 번 더
//   확인한 뒤에만 `permanent` 로 지운다. 권한이 없어 못 지운 것은 그대로 알린다.

import type { Action } from "@pitwall/keymap";
import {
  ERROR_CODE,
  type FsCopied,
  type FsCreated,
  type FsMoved,
  type FsTrashed,
} from "@pitwall/rpc-client";

import { baseName } from "../editor/editor-tabs";
import type { EditorWorkspace } from "../editor/editor-workspace";
import type { NotificationInput } from "../notifications/notifications";
import type { FileTreeStrings } from "./strings";
import { folderOf } from "./tree-drop";
import { type FileTreeModel, isWithin, joinPath, parentOf } from "./tree-model";

/** Finder 드롭 들여오기의 결과(apps/desktop/src-tauri/src/drop_import.rs → crates/pw-fs `Imported`). */
export interface DropImported {
  copied: string[];
  conflicts: string[];
  skipped: string[];
  links: number;
  cancelled: boolean;
}

/** 이름을 묻는 대화상자(file-dialogs.tsx). 취소하면 `undefined`. */
export interface NameQuestion {
  title: string;
  /** 제목 아래 한 줄(만들 자리 등). */
  hint?: string;
  initial: string;
  /** 처음에 확장자 앞까지만 고른다(이름 바꾸기). */
  selectStem?: boolean;
  /** 쓸 수 없는 이름이면 그 이유. */
  validate(name: string): string | undefined;
}

/** 예 / 아니오 확인(file-dialogs.tsx). */
export interface ConfirmQuestion {
  title: string;
  body: string;
  /** 본문 아래 경고(미저장 탭 등). */
  warning?: string;
  confirm: string;
  /** 되돌릴 수 없는 조작(영구 삭제). */
  danger?: boolean;
}

export interface FileOpsDeps {
  /** Host RPC. 프로젝트 · 워크트리는 부르는 쪽이 채운다. */
  rpc: {
    create(path: string, kind: "file" | "dir"): Promise<FsCreated>;
    rename(from: string, to: string): Promise<FsMoved>;
    copy(from: string, to: string): Promise<FsCopied>;
    trash(paths: string[], permanent: boolean): Promise<FsTrashed>;
  };
  /** Finder 드롭 들여오기(데스크톱 command). 없으면(시험 · 웹) 드롭을 받지 않는다. */
  importDrop?(dropId: number, dir: string): Promise<DropImported>;
  workspace: Pick<EditorWorkspace, "moved" | "deleted" | "unsavedUnder">;
  model: FileTreeModel;
  strings: FileTreeStrings["ops"];
  notify(input: NotificationInput): void;
  askName(question: NameQuestion): Promise<string | undefined>;
  confirm(question: ConfirmQuestion): Promise<boolean>;
  /** 새로 만든 파일을 고정 탭으로 연다. */
  open(path: string): void;
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function isConflict(e: unknown): boolean {
  return (e as { code?: unknown } | null)?.code === ERROR_CODE.CONFLICT;
}

/** 쓸 수 없는 이름이면 그 이유. 앞뒤 공백은 부르는 쪽이 걷는다. */
export function nameProblem(name: string, strings: FileTreeStrings["ops"]): string | undefined {
  if (name === "") return strings.name.empty;
  if (/[/\\]/.test(name)) return strings.name.separator;
  if (name === "." || name === "..") return strings.name.dots;
  return undefined;
}

const lines = (paths: readonly string[]) => paths.map(baseName).join("\n");

export function fileOps(deps: FileOpsDeps) {
  const { strings: T } = deps;

  /** 조작 실패를 알린다. 겹치는 이름이면 그 이름으로. */
  function failed(e: unknown, name: string, title: (message: string) => string) {
    if (isConflict(e)) deps.notify({ level: "error", title: T.exists(name) });
    else deps.notify({ level: "error", title: title(errorText(e)) });
  }

  async function askName(question: Omit<NameQuestion, "validate">): Promise<string | undefined> {
    const name = await deps.askName({ ...question, validate: (n) => nameProblem(n.trim(), T) });
    return name?.trim();
  }

  /** 휴지통 결과를 맞춘다: 지운 것은 탭 · 트리에서 걷고, 못 지운 것은 이유대로. 영구 삭제를 확인할 것이 남으면 돌려준다. */
  function settle(result: FsTrashed): FsTrashed["failed"] {
    for (const path of result.removed) deps.workspace.deleted(path);
    const gone = result.failed.filter((f) => f.reason === "missing").map((f) => f.path);
    for (const path of gone) deps.workspace.deleted(path);
    const permission = result.failed.filter((f) => f.reason === "permission");
    if (permission.length > 0)
      deps.notify({
        level: "error",
        title: T.deletePermission(lines(permission.map((f) => f.path))),
      });
    for (const f of result.failed.filter((f) => f.reason === "failed"))
      deps.notify({ level: "error", title: T.failed.delete(f.message) });
    void deps.model.touched(
      [...result.removed, ...gone],
      [...result.removed, ...gone].map(() => "removed" as const),
    );
    return result.failed.filter((f) => f.reason === "unavailable");
  }

  return {
    /** `dir` 안에 새 파일 · 폴더를 만든다. 파일이면 연다. */
    async create(dir: string, kind: "file" | "dir"): Promise<string | undefined> {
      const name = await askName({
        title: kind === "file" ? T.name.newFile : T.name.newFolder,
        hint: T.name.in(baseName(dir)),
        initial: "",
      });
      if (!name) return undefined;
      try {
        const { path } = await deps.rpc.create(joinPath(dir, name), kind);
        await deps.model.touched([path], ["created"]);
        await deps.model.reveal(path);
        if (kind === "file") deps.open(path);
        return path;
      } catch (e) {
        failed(e, name, T.failed.create);
        return undefined;
      }
    },

    /** 이름을 묻고 바꾼다. 열린 탭이 새 이름을 따라간다. */
    async rename(path: string): Promise<string | undefined> {
      const parent = parentOf(path);
      if (parent === undefined) return undefined;
      const current = baseName(path);
      const name = await askName({ title: T.name.rename, initial: current, selectStem: true });
      if (!name || name === current) return undefined;
      try {
        const moved = await deps.rpc.rename(path, joinPath(parent, name));
        deps.workspace.moved(moved.from, moved.to);
        await deps.model.moved(path, moved.to);
        deps.model.select(moved.to);
        return moved.to;
      } catch (e) {
        failed(e, name, T.failed.rename);
        return undefined;
      }
    },

    /**
     * `paths` 를 `dir` 로 옮긴다(`copy` 면 복사, ⌥+드래그). 이미 그 폴더에 있는 것을 옮기는 것 · 자기 안으로 옮기는 것은 하지 않는다.
     * 옮긴 탭은 새 경로를 따라간다.
     */
    async move(paths: readonly string[], dir: string, copy: boolean): Promise<void> {
      const touched: string[] = [];
      for (const path of paths) {
        if (isWithin(dir, path)) continue;
        if (!copy && parentOf(path) === dir) continue;
        const to = joinPath(dir, baseName(path));
        try {
          if (copy) {
            const copied = await deps.rpc.copy(path, to);
            touched.push(copied.to);
            if (copied.skipped.length > 0)
              deps.notify({ level: "warning", title: T.drop.skipped(lines(copied.skipped)) });
          } else {
            const moved = await deps.rpc.rename(path, to);
            deps.workspace.moved(moved.from, moved.to);
            await deps.model.moved(path, moved.to);
          }
        } catch (e) {
          failed(e, baseName(path), copy ? T.failed.copy : T.failed.move);
        }
      }
      if (touched.length > 0)
        await deps.model.touched(
          touched,
          touched.map(() => "created" as const),
        );
    },

    /** 확인을 받고 휴지통으로 옮긴다. 휴지통이 받지 않으면 한 번 더 확인하고 영구히 지운다. */
    async remove(paths: readonly string[]): Promise<void> {
      if (paths.length === 0) return;
      const unsaved = [...new Set(paths.flatMap((p) => deps.workspace.unsavedUnder(p)))];
      const ok = await deps.confirm({
        title: T.delete.title,
        body: T.delete.body(paths.length, baseName(paths[0]!)),
        warning: unsaved.length > 0 ? T.delete.unsaved(lines(unsaved)) : undefined,
        confirm: T.delete.confirm,
      });
      if (!ok) return;
      let unavailable: FsTrashed["failed"];
      try {
        unavailable = settle(await deps.rpc.trash([...paths], false));
      } catch (e) {
        deps.notify({ level: "error", title: T.failed.delete(errorText(e)) });
        return;
      }
      if (unavailable.length === 0) return;
      const permanent = await deps.confirm({
        title: T.permanent.title,
        body: T.permanent.body(lines(unavailable.map((f) => f.path)), unavailable[0]!.message),
        confirm: T.permanent.confirm,
        danger: true,
      });
      if (!permanent) return;
      try {
        const left = settle(
          await deps.rpc.trash(
            unavailable.map((f) => f.path),
            true,
          ),
        );
        for (const f of left) deps.notify({ level: "error", title: T.failed.delete(f.message) });
      } catch (e) {
        deps.notify({ level: "error", title: T.failed.delete(errorText(e)) });
      }
    },

    /** Finder 에서 드롭한 것(`dropId`)을 `dir` 로 복사한다. */
    async importDrop(dropId: number, dir: string): Promise<void> {
      if (!deps.importDrop) return;
      let result: DropImported;
      try {
        result = await deps.importDrop(dropId, dir);
      } catch (e) {
        deps.notify({ level: "error", title: T.drop.failed(errorText(e)) });
        return;
      }
      if (result.conflicts.length > 0)
        deps.notify({ level: "error", title: T.drop.conflicts(lines(result.conflicts)) });
      if (result.skipped.length > 0)
        deps.notify({ level: "warning", title: T.drop.skipped(lines(result.skipped)) });
      if (result.cancelled) deps.notify({ level: "info", title: T.drop.cancelled });
      await deps.model.touched(
        result.copied,
        result.copied.map(() => "created" as const),
      );
    },
  };
}

export type FileOps = ReturnType<typeof fileOps>;

/** 고른 항목(없으면 루트)이 든 폴더. 새 파일 · 터미널이 쓴다. */
function selectedFolder(model: FileTreeModel): string | undefined {
  const root = model.getRoot();
  if (root === undefined) return undefined;
  const path = model.selected();
  const row = path === undefined ? undefined : model.rows().find((r) => r.entry.path === path);
  return row ? folderOf(row.entry, root) : root;
}

/** 고른 항목. 루트는 이름을 바꾸거나 지울 수 없어 뺀다. */
function selectedEntry(model: FileTreeModel): string | undefined {
  const path = model.selected();
  return path === undefined || path === model.getRoot() ? undefined : path;
}

/**
 * 트리의 파일 조작 액션(⌘⇧A 로도 찾는다). 대상은 트리에서 고른 항목이다. 기본 키는 extensions/keymap-default/keymaps/default.toml 의
 * 「파일 트리」 묶음 — `when = "fileTree"` 라 트리에 포커스가 있을 때만 키로 돈다.
 */
export function fileOpsActions(deps: {
  strings: FileTreeStrings["actions"];
  model: FileTreeModel;
  ops: Pick<FileOps, "create" | "rename" | "remove">;
  canOpenTerminal(): boolean;
  openInTerminal(dir: string): void;
}): Action[] {
  const { strings: T, model, ops } = deps;
  const inFolder = (run: (dir: string) => void) => () => {
    const dir = selectedFolder(model);
    if (dir !== undefined) run(dir);
  };
  const onEntry = (run: (path: string) => void) => () => {
    const path = selectedEntry(model);
    if (path !== undefined) run(path);
  };
  return [
    {
      id: "fileTree.newFile",
      label: T.newFile,
      keywords: ["New File", "새 파일"],
      enabled: () => selectedFolder(model) !== undefined,
      run: inFolder((dir) => void ops.create(dir, "file")),
    },
    {
      id: "fileTree.newFolder",
      label: T.newFolder,
      keywords: ["New Directory", "New Folder", "새 폴더"],
      enabled: () => selectedFolder(model) !== undefined,
      run: inFolder((dir) => void ops.create(dir, "dir")),
    },
    {
      id: "fileTree.rename",
      label: T.rename,
      keywords: ["Rename File", "Rename", "이름 바꾸기"],
      enabled: () => selectedEntry(model) !== undefined,
      run: onEntry((path) => void ops.rename(path)),
    },
    {
      id: "fileTree.delete",
      label: T.delete,
      keywords: ["Delete", "Move to Trash", "삭제", "휴지통"],
      enabled: () => selectedEntry(model) !== undefined,
      run: onEntry((path) => void ops.remove([path])),
    },
    {
      id: "fileTree.openInTerminal",
      label: T.openInTerminal,
      keywords: ["Open in Terminal", "터미널"],
      enabled: () => deps.canOpenTerminal() && selectedFolder(model) !== undefined,
      run: inFolder(deps.openInTerminal),
    },
  ];
}
