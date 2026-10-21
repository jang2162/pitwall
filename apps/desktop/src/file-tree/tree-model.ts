// 파일 트리의 상태(docs/design/12-m1-plan.md M1-14): 루트 아래 펼친 폴더의 목록과 펼침 · 선택. 화면(file-tree.tsx)과 떨어진 순수 모델이라
// 가짜 `list` 로 시험한다.
//
// - 폴더는 펼칠 때 `fs.list` 로 읽는다. 읽은 목록은 접어도 기억하고, 바뀌었다는 소식(`fs.changed`)이 오면 펼친 것만 다시 읽고 접힌
//   것은 낡았다고 표시해 다음에 펼칠 때 읽는다.
// - `fs.changed` 의 규칙은 crates/pw-fs/src/watch/mod.rs 의 `FsChanged`: 만들어지거나 지워진 경로는 그 부모를, 폴더 경로의 `modified`
//   는 「그 아래를 다시 읽어라」(ignore 된 폴더 안의 변경이 그 폴더 하나로 접혀 온다)다. `rescan` 이면 펼친 폴더를 모두 다시 읽는다.
// - 경로는 화면에 보이는 논리 경로(루트 + 이름들)다. 루트 안 심링크 폴더(pnpm 의 `node_modules/x` 등)를 펼치면 `fs.list` 가 실제
//   경로를 돌려주는데, 감시는 실제 경로로 알리므로 그 짝(`aliases`)을 기억해 같은 변경을 논리 경로에도 맞춘다.
// - 루트 밖을 가리키는 심링크(`outside`, 결정 M1-D8)는 펼치지 않는다 — Host 가 그 아래 읽기를 거절한다.

import type { EntryKind, FsList } from "@pitwall/rpc-client";

/** `fs.changed` 의 내용(crates/pw-fs/src/watch/mod.rs 의 `FsChanged`). 이벤트 타입이라 RPC 생성물에 없다. */
export interface FsChanged {
  root: string;
  worktree: string | null;
  paths: string[];
  kinds: ("created" | "modified" | "removed")[];
  rescan: boolean;
}

export interface TreeEntry {
  /** 논리 경로(부모 경로 + 이름). */
  path: string;
  name: string;
  /** 펼칠 수 있다: 폴더이거나 루트 안 폴더를 가리키는 심링크. */
  dir: boolean;
  symlink: boolean;
  /** 루트 밖을 가리키는 심링크. 펼치거나 열지 않는다. */
  outside: boolean;
  /** `.gitignore` 등으로 무시된다(무시된 폴더 안 포함). */
  ignored: boolean;
  /** 벤더 · 생성 · 문서 파일(M1-02 `lang.detect`). 목록을 받은 뒤 채운다. */
  dim: boolean;
}

export interface TreeRow {
  entry: TreeEntry;
  /** 루트가 0. */
  depth: number;
  expanded: boolean;
  /** 펼쳤는데 아직 목록이 없다. */
  loading: boolean;
  /** 펼쳤는데 읽지 못했다. */
  error?: string;
}

export type IgnoredMode = "dim" | "hide";

export interface TreeDeps {
  list(path: string): Promise<FsList>;
  /** 파일들의 벤더 · 생성 · 문서 여부(같은 순서). 없으면 흐리게 하지 않는다. */
  dimmed?(paths: string[]): Promise<boolean[]>;
}

interface Listing {
  entries: TreeEntry[];
  /** 바뀌었다는 소식이 왔다 — 다음에 펼칠 때 다시 읽는다. */
  stale: boolean;
}

/** Host 경로의 구분자. Windows 의 Host 는 `\` 로 준다. */
export function separatorOf(root: string): string {
  return root.includes("\\") && !root.includes("/") ? "\\" : "/";
}

export function joinPath(dir: string, name: string, sep = separatorOf(dir)): string {
  return dir.endsWith(sep) ? dir + name : dir + sep + name;
}

/** `path` 가 `dir` 이거나 그 아래다. */
export function isWithin(path: string, dir: string, sep = separatorOf(dir)): boolean {
  if (path === dir) return true;
  return path.startsWith(dir.endsWith(sep) ? dir : dir + sep);
}

function parentOf(path: string, sep: string): string | undefined {
  const at = path.lastIndexOf(sep);
  if (at <= 0) return at === 0 && path.length > 1 ? sep : undefined;
  return path.slice(0, at);
}

function entryKind(kind: EntryKind, target: EntryKind | null): boolean {
  return kind === "dir" || (kind === "symlink" && target === "dir");
}

/** 폴더 먼저, 그 안은 Host 의 순서(이름순). */
function sortEntries(entries: TreeEntry[]): TreeEntry[] {
  return [...entries.filter((e) => e.dir), ...entries.filter((e) => !e.dir)];
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export class FileTreeModel {
  private root: string | undefined;
  private sep = "/";
  private rootEntry: TreeEntry | undefined;
  private readonly listings = new Map<string, Listing>();
  private readonly expanded = new Set<string>();
  private readonly loading = new Map<string, Promise<void>>();
  private readonly errors = new Map<string, string>();
  /** 실제 경로 → 그것을 보인 논리 경로들. */
  private readonly aliases = new Map<string, Set<string>>();
  /** 경로마다 마지막으로 보낸 읽기 번호. 늦게 온 옛 응답을 버린다. */
  private readonly requests = new Map<string, number>();
  private nextRequest = 1;
  /** 루트를 바꿀 때마다 오른다. 옛 루트의 응답을 버린다. */
  private generation = 0;
  private selectedPath: string | undefined;
  private mode: IgnoredMode = "dim";
  private version = 0;
  private cachedRows: { version: number; rows: TreeRow[] } | undefined;
  private readonly listeners = new Set<() => void>();

  constructor(private readonly deps: TreeDeps) {}

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** 바뀔 때마다 오른다(`useSyncExternalStore` 의 스냅숏). */
  getVersion(): number {
    return this.version;
  }

  private changed() {
    this.version++;
    for (const listener of this.listeners) listener();
  }

  getRoot(): string | undefined {
    return this.root;
  }

  selected(): string | undefined {
    return this.selectedPath;
  }

  ignoredMode(): IgnoredMode {
    return this.mode;
  }

  isExpanded(path: string): boolean {
    return this.expanded.has(path);
  }

  /** 루트를 바꾼다(활성 워크트리가 바뀌었다). 펼침 · 선택은 버린다. `undefined` 면 비운다. */
  setRoot(root: string | undefined): Promise<void> {
    if (root === this.root) return this.loading.get(root ?? "") ?? Promise.resolve();
    this.generation++;
    this.root = root;
    this.listings.clear();
    this.expanded.clear();
    this.loading.clear();
    this.errors.clear();
    this.aliases.clear();
    this.requests.clear();
    this.selectedPath = undefined;
    if (root === undefined) {
      this.rootEntry = undefined;
      this.changed();
      return Promise.resolve();
    }
    this.sep = separatorOf(root);
    const name = root.slice(root.lastIndexOf(this.sep) + 1) || root;
    this.rootEntry = {
      path: root,
      name,
      dir: true,
      symlink: false,
      outside: false,
      ignored: false,
      dim: false,
    };
    this.expanded.add(root);
    this.changed();
    return this.load(root);
  }

  setIgnoredMode(mode: IgnoredMode) {
    if (mode === this.mode) return;
    this.mode = mode;
    // 숨긴 행이 선택돼 있었으면 선택을 그대로 두되 화면에서는 사라진다 — 다시 보이면 돌아온다.
    this.changed();
  }

  select(path: string | undefined) {
    if (path === this.selectedPath) return;
    this.selectedPath = path;
    this.changed();
  }

  private entry(path: string): TreeEntry | undefined {
    if (path === this.root) return this.rootEntry;
    const parent = parentOf(path, this.sep);
    return parent === undefined
      ? undefined
      : this.listings.get(parent)?.entries.find((e) => e.path === path);
  }

  /** 펼친다. 목록이 없거나 낡았으면 읽는다 — 읽기가 끝나면 풀린다. */
  expand(path: string): Promise<void> {
    const entry = this.entry(path);
    if (!entry?.dir || entry.outside) return Promise.resolve();
    if (!this.expanded.has(path)) {
      this.expanded.add(path);
      this.changed();
    }
    const listing = this.listings.get(path);
    if (listing && !listing.stale) return Promise.resolve();
    return this.loading.get(path) ?? this.load(path);
  }

  collapse(path: string) {
    if (!this.expanded.delete(path)) return;
    // 접은 폴더 안에 선택이 있으면 그 폴더로 올린다(IntelliJ 와 같다).
    if (
      this.selectedPath &&
      this.selectedPath !== path &&
      isWithin(this.selectedPath, path, this.sep)
    )
      this.selectedPath = path;
    this.changed();
  }

  toggle(path: string): Promise<void> {
    if (this.expanded.has(path)) {
      this.collapse(path);
      return Promise.resolve();
    }
    return this.expand(path);
  }

  private load(path: string): Promise<void> {
    const generation = this.generation;
    const request = this.nextRequest++;
    this.requests.set(path, request);
    const current = () => generation === this.generation && this.requests.get(path) === request;
    const done = this.deps
      .list(path)
      .then(
        (list) => {
          if (!current()) return;
          this.errors.delete(path);
          if (list.path !== path) {
            let logical = this.aliases.get(list.path);
            if (!logical) this.aliases.set(list.path, (logical = new Set()));
            logical.add(path);
          }
          const entries = sortEntries(
            list.entries.map((e) => ({
              path: joinPath(path, e.name, this.sep),
              name: e.name,
              dir: entryKind(e.kind, e.target) && !e.outside,
              symlink: e.kind === "symlink",
              outside: e.outside,
              ignored: e.ignored,
              dim:
                this.listings.get(path)?.entries.find((old) => old.name === e.name)?.dim ?? false,
            })),
          );
          this.forgetMissing(path, entries);
          this.listings.set(path, { entries, stale: false });
          this.changed();
          this.markDimmed(path, entries, generation);
          // 펼쳐 둔 채 목록이 없는(낡아 버린) 하위 폴더도 이어서 읽는다.
          return Promise.all(
            entries
              .filter((e) => e.dir && this.expanded.has(e.path))
              .filter((e) => this.listings.get(e.path)?.stale !== false)
              .map((e) => this.loading.get(e.path) ?? this.load(e.path)),
          ).then(() => undefined);
        },
        (e: unknown) => {
          if (!current()) return;
          this.errors.set(path, errorText(e));
          this.changed();
        },
      )
      .finally(() => {
        if (this.loading.get(path) === done) this.loading.delete(path);
      });
    this.loading.set(path, done);
    return done;
  }

  /** 새 목록에 없는 하위 항목의 기억(펼침 · 목록 · 선택)을 지운다. */
  private forgetMissing(path: string, entries: TreeEntry[]) {
    const old = this.listings.get(path);
    if (!old) return;
    const kept = new Set(entries.filter((e) => e.dir).map((e) => e.path));
    for (const gone of old.entries.filter((e) => e.dir && !kept.has(e.path)))
      this.forgetBelow(gone.path);
    const names = new Set(entries.map((e) => e.path));
    if (
      this.selectedPath &&
      isWithin(this.selectedPath, path, this.sep) &&
      this.selectedPath !== path
    ) {
      const child = old.entries.find((e) => isWithin(this.selectedPath!, e.path, this.sep));
      if (child && !names.has(child.path)) this.selectedPath = path;
    }
  }

  // Map · Set 은 돌면서 지금 항목을 지워도 된다(남은 항목은 그대로 돈다).
  private forgetBelow(dir: string) {
    for (const map of [this.listings, this.errors] as Map<string, unknown>[])
      for (const key of map.keys()) if (isWithin(key, dir, this.sep)) map.delete(key);
    for (const key of this.expanded) if (isWithin(key, dir, this.sep)) this.expanded.delete(key);
    for (const [real, logical] of this.aliases) {
      for (const l of logical) if (isWithin(l, dir, this.sep)) logical.delete(l);
      if (logical.size === 0) this.aliases.delete(real);
    }
  }

  private markDimmed(path: string, entries: TreeEntry[], generation: number) {
    const dimmed = this.deps.dimmed;
    const files = entries.filter((e) => !e.dir && !e.outside);
    if (!dimmed || files.length === 0) return;
    dimmed(files.map((f) => f.path))
      .then((flags) => {
        if (generation !== this.generation) return;
        const listing = this.listings.get(path);
        if (!listing) return;
        let any = false;
        const byPath = new Map(files.map((f, i) => [f.path, flags[i] ?? false]));
        listing.entries = listing.entries.map((e) => {
          const dim = byPath.get(e.path);
          if (dim === undefined || dim === e.dim) return e;
          any = true;
          return { ...e, dim };
        });
        if (any) this.changed();
      })
      .catch((e: unknown) => console.warn("[tree] lang.detect failed", path, e));
  }

  /** 실제 경로를 이 트리의 논리 경로들로. 루트 밖이면 빈 목록. */
  private logicalPaths(path: string): string[] {
    const out: string[] = [];
    if (this.root !== undefined && isWithin(path, this.root, this.sep)) out.push(path);
    for (const [real, logical] of this.aliases) {
      if (!isWithin(path, real, this.sep)) continue;
      const rest = path.slice(real.length);
      for (const l of logical) out.push(l + rest);
    }
    return out;
  }

  /**
   * `fs.changed` 를 반영한다. 펼친 폴더는 다시 읽고(끝나면 풀린다), 접힌 폴더의 기억은 낡았다고 표시한다. 이 트리의 루트 밖
   * 경로(다른 워크트리를 연 창의 이벤트)는 무시한다.
   */
  applyChange(change: FsChanged): Promise<void> {
    if (this.root === undefined) return Promise.resolve();
    const dirs = new Set<string>();
    if (change.rescan) {
      for (const dir of this.listings.keys()) dirs.add(dir);
    } else {
      change.paths.forEach((changed, i) => {
        const kind = change.kinds[i];
        for (const path of this.logicalPaths(changed)) {
          if (kind !== "modified") {
            const parent = parentOf(path, this.sep);
            if (parent !== undefined && this.listings.has(parent)) dirs.add(parent);
          }
          // 폴더의 `modified` 는 그 아래를 다시 읽으라는 뜻이다. 그 아래에서 기억하는 목록을 모두 고른다.
          if (kind !== "removed" && this.listings.has(path))
            for (const dir of this.listings.keys())
              if (isWithin(dir, path, this.sep)) dirs.add(dir);
        }
      });
    }
    const reloads: Promise<void>[] = [];
    for (const dir of dirs) {
      const listing = this.listings.get(dir);
      if (!listing) continue;
      if (this.expanded.has(dir) && this.visible(dir)) reloads.push(this.load(dir));
      else listing.stale = true;
    }
    return Promise.all(reloads).then(() => undefined);
  }

  /** 기억하는 목록을 모두 다시 읽는다(펼친 것은 곧바로, 접힌 것은 다음에 펼칠 때). */
  refresh(): Promise<void> {
    return this.applyChange({ root: "", worktree: null, paths: [], kinds: [], rescan: true });
  }

  /** `path`(실제 경로여도 된다)가 이 트리 안에 있다. 루트를 아직 모르면 참 — 고르는 쪽이 알게 된 뒤에 판정한다. */
  contains(path: string): boolean {
    return this.root === undefined || this.logicalPaths(path).length > 0;
  }

  /** 폴더와 그 조상이 모두 펼쳐져 있다(화면에 그 아래가 보인다). */
  private visible(dir: string): boolean {
    for (let p: string | undefined = dir; p !== undefined; p = parentOf(p, this.sep)) {
      if (!this.expanded.has(p)) return false;
      if (p === this.root) return true;
    }
    return false;
  }

  /** `path`(파일 · 폴더, 실제 경로여도 된다)까지 조상을 펼치고 고른다. 루트 밖이거나 가는 길에 없으면(지워졌다) `false`. */
  async reveal(path: string): Promise<boolean> {
    const root = this.root;
    if (root === undefined) return false;
    const logical = this.logicalPaths(path)[0];
    if (logical === undefined) return false;
    if (logical === root) {
      this.select(root);
      return true;
    }
    const chain: string[] = [];
    for (let p = parentOf(logical, this.sep); p !== undefined; p = parentOf(p, this.sep)) {
      chain.unshift(p);
      if (p === root) break;
    }
    if (chain[0] !== root) return false;
    for (const dir of chain) {
      await (this.loading.get(dir) ?? Promise.resolve());
      await this.expand(dir);
      if (this.root !== root || !this.entry(dir)) return false;
    }
    if (!this.entry(logical)) return false;
    this.select(logical);
    return true;
  }

  private leadsToSelection(path: string): boolean {
    return this.selectedPath !== undefined && isWithin(this.selectedPath, path, this.sep);
  }

  /**
   * 펼친 순서대로 보이는 행. 바뀌지 않았으면 같은 배열을 돌려준다. 숨김 모드에서도 고른 항목과 그 조상은 보인다 — 「프로젝트 뷰에서
   * 선택」 한 ignore 된 파일이 숨은 채로 남지 않게.
   */
  rows(): TreeRow[] {
    if (this.cachedRows?.version === this.version) return this.cachedRows.rows;
    const rows: TreeRow[] = [];
    const hide = this.mode === "hide";
    const visit = (entry: TreeEntry, depth: number) => {
      const expanded = entry.dir && this.expanded.has(entry.path);
      const listing = expanded ? this.listings.get(entry.path) : undefined;
      rows.push({
        entry,
        depth,
        expanded,
        loading: expanded && !listing && !this.errors.has(entry.path),
        error: expanded ? this.errors.get(entry.path) : undefined,
      });
      if (!listing) return;
      for (const child of listing.entries) {
        if (hide && child.ignored && !this.leadsToSelection(child.path)) continue;
        visit(child, depth + 1);
      }
    };
    if (this.rootEntry) visit(this.rootEntry, 0);
    this.cachedRows = { version: this.version, rows };
    return rows;
  }
}
