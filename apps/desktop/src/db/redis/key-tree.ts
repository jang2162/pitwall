// Redis 키 트리의 모델(14-m3-plan M3-18): `db.kv.scan` 이 묶음으로 준 키(타입 · TTL · 크기)를 React 상태 밖에 쌓고, 구분자로
// 묶은 그룹을 펼친 만큼만 줄로 편다. 화면(key-tree-view.tsx)은 TanStack Virtual 로 보이는 줄만 그린다.
//
// - 키 100만 개를 쥐어도 줄 하나마다 객체를 만들지 않는다: 그룹은 자식 그룹 표와 잎(키 번호의 배열)만 갖고, 펼친 줄은 두 배열
//   (`items` · `depths`)이다. 펼침 · 키가 바뀔 때만 다시 편다 — 화면은 `subscribe` 로 「바뀌었다」 를 한 프레임에 한 번 받는다
//   (결과 버퍼 src/db/grid/result-buffer.ts 와 같은 꼴).
// - 같은 키가 두 번 오면(SCAN 은 스캔 중 바뀐 키를 두 번 줄 수 있다) 정보만 새로 받은 것으로 바꾼다.
// - 정렬: 그룹 · 잎은 이름 순(코드 단위 비교 — 로캘 비교는 100만 개에서 수 초가 걸린다). 스캔 중에는 잎이 `SORT_WHILE_LOADING`
//   보다 많은 그룹을 정렬하지 않고 받은 차례로 둔다 — 묶음마다 큰 그룹을 다시 정렬하면 스크롤이 끊긴다. 스캔이 끝나면(`settle`) 한
//   번 정렬한다.
// - UTF-8 이 아닌 키(`{ hex }`)는 묶지 않고 맨 위 층에 둔다 — 구분자를 바이트 안에서 찾지 않는다.
// - 보유 상한 `MAX_TREE_KEYS`: 넘는 키는 받지 않는다 — 부르는 쪽(스캔)이 그때 스캔을 멈추고 「패턴으로 좁혀라」 를 보인다.

import type { KvKey, KvKeyInfo } from "@pitwall/rpc-client";

/** 트리가 쥐는 키 수의 상한(결정 M3-D17 의 「키 100만 개」 와 같다). */
export const MAX_TREE_KEYS = 1_000_000;
/** 스캔 중에도 정렬하는 그룹의 잎 수 상한(머리 주석). */
export const SORT_WHILE_LOADING = 20_000;
/** 기본 구분자(10-ui.md 「에디터 영역 문서 타입」 의 Redis 키). */
export const DEFAULT_SEPARATOR = ":";

/** 키 하나. */
export interface KeyEntry {
  key: KvKey;
  /** 묶기 · 정렬 · 찾기에 쓰는 이름: 글이면 그대로, 바이트면 hex. */
  name: string;
  info: KvKeyInfo;
  /** 정보를 받은 때(`performance.now()`) — 남은 TTL 을 셀 때 쓴다. */
  at: number;
  /** 지운 키(번호를 다시 쓰지 않는다). */
  gone?: boolean;
}

export interface KeyGroup {
  /** 이 층의 조각. */
  name: string;
  /** 그룹 경로(구분자로 이은 조각들 + 구분자) — 펼침의 키. */
  path: string;
  groups: Map<string, KeyGroup>;
  /** 잎(키 번호). */
  leaves: number[];
  /** 아래 모든 키 수. */
  count: number;
  /** 정렬된 차례인가(잎 · 그룹). */
  sorted: boolean;
}

/** 펼친 줄. 줄 `i` 는 `items[i]`(그룹 또는 키 번호)와 `depths[i]`. */
export interface KeyRows {
  items: (KeyGroup | number)[];
  depths: number[];
}

type Schedule = (run: () => void) => void;

const frame: Schedule = (run) => {
  if (typeof requestAnimationFrame === "function") requestAnimationFrame(() => run());
  else setTimeout(run, 0);
};

/** 키의 이름(묶기 · 정렬). */
export function keyName(key: KvKey): string {
  return typeof key === "string" ? key : key.hex;
}

/** 같은 키인가. 글 `"0xff"` 와 바이트 `0xff` 는 다르다. */
export function sameKey(a: KvKey, b: KvKey): boolean {
  if (typeof a === "string" || typeof b === "string") return a === b;
  return a.hex === b.hex;
}

/** 이름 표의 키 — 글과 바이트가 같은 칸에 들지 않게 앞붙임을 단다. */
const slot = (key: KvKey) => (typeof key === "string" ? `t${key}` : `b${key.hex}`);

const byName = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);

function newGroup(name: string, path: string): KeyGroup {
  return { name, path, groups: new Map(), leaves: [], count: 0, sorted: true };
}

export class KeyTree {
  readonly entries: KeyEntry[] = [];
  private readonly slots = new Map<string, number>();
  private root: KeyGroup = newGroup("", "");
  private live = 0;
  private readonly listeners = new Set<() => void>();
  private pending = false;
  private loading = true;
  /** 바뀔 때마다 오른다(화면의 메모 키). */
  version = 0;

  constructor(
    private separatorValue: string = DEFAULT_SEPARATOR,
    private readonly schedule: Schedule = frame,
    /** 보유 상한(시험이 줄인다). */
    readonly maxKeys: number = MAX_TREE_KEYS,
  ) {}

  get separator(): string {
    return this.separatorValue;
  }

  /** 지우지 않은 키 수. */
  get size(): number {
    return this.live;
  }

  get full(): boolean {
    return this.live >= this.maxKeys;
  }

  /** 받은 키를 더한다. 상한을 넘는 새 키는 버리고 그 수를 돌려준다. */
  add(infos: readonly KvKeyInfo[], at = performance.now()): number {
    let dropped = 0;
    for (const info of infos) {
      const s = slot(info.key);
      const known = this.slots.get(s);
      if (known !== undefined && !this.entries[known]!.gone) {
        const entry = this.entries[known]!;
        entry.info = info;
        entry.at = at;
        continue;
      }
      if (this.live >= this.maxKeys) {
        dropped++;
        continue;
      }
      const index = this.entries.length;
      this.entries.push({ key: info.key, name: keyName(info.key), info, at });
      this.slots.set(s, index);
      this.place(index);
      this.live++;
    }
    this.changed();
    return dropped;
  }

  /** 키를 찾는다. */
  find(key: KvKey): KeyEntry | undefined {
    const index = this.slots.get(slot(key));
    const entry = index === undefined ? undefined : this.entries[index];
    return entry && !entry.gone ? entry : undefined;
  }

  /** 키를 지운 것으로 둔다(삭제 · 이름 바꾸기의 옛 이름). */
  remove(key: KvKey) {
    const index = this.slots.get(slot(key));
    if (index === undefined) return;
    const entry = this.entries[index]!;
    if (entry.gone) return;
    entry.gone = true;
    this.slots.delete(slot(key));
    this.live--;
    const chain = this.chain(entry);
    const owner = chain[chain.length - 1]!;
    const at = owner.leaves.indexOf(index);
    if (at >= 0) owner.leaves.splice(at, 1);
    for (const g of chain) g.count--;
    this.prune(chain);
    this.changed();
  }

  /** 키의 TTL 을 바꾼다(`null` 은 만료 없음). */
  setTtl(key: KvKey, ttlMs: number | null, at = performance.now()) {
    const entry = this.find(key);
    if (!entry) return;
    entry.info = { ...entry.info, ttlMs };
    entry.at = at;
    this.changed();
  }

  /** 구분자를 바꾼다 — 그룹을 다시 만든다. */
  setSeparator(separator: string) {
    if (separator === this.separatorValue) return;
    this.separatorValue = separator;
    this.root = newGroup("", "");
    this.entries.forEach((entry, index) => {
      if (!entry.gone) this.place(index);
    });
    this.changed();
  }

  /** 스캔 중인가 — 큰 그룹의 정렬을 미룬다(머리 주석). `false` 로 바꾸면 미룬 정렬을 다음 펼침에서 한다. */
  setLoading(loading: boolean) {
    if (this.loading === loading) return;
    this.loading = loading;
    this.changed();
  }

  /** 펼친 그룹(경로)만 열어 줄로 편다. */
  rows(expanded: ReadonlySet<string>): KeyRows {
    const items: (KeyGroup | number)[] = [];
    const depths: number[] = [];
    const walk = (group: KeyGroup, depth: number) => {
      this.sort(group);
      for (const child of group.groups.values()) {
        items.push(child);
        depths.push(depth);
        if (expanded.has(child.path)) walk(child, depth + 1);
      }
      for (const leaf of group.leaves) {
        items.push(leaf);
        depths.push(depth);
      }
    };
    walk(this.root, 0);
    return { items, depths };
  }

  /** 줄에 보이는 이름: 그룹은 조각, 키는 마지막 조각(묶이지 않은 키는 이름 전체). */
  leafLabel(index: number): string {
    const entry = this.entries[index]!;
    if (typeof entry.key !== "string" || this.separatorValue === "") return entry.name;
    const at = entry.name.lastIndexOf(this.separatorValue);
    return at < 0 ? entry.name : entry.name.slice(at + this.separatorValue.length);
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private segments(entry: KeyEntry): string[] {
    if (typeof entry.key !== "string" || this.separatorValue === "") return [];
    const parts = entry.name.split(this.separatorValue);
    parts.pop();
    return parts;
  }

  /** 뿌리부터 그 키가 든 그룹까지(없는 그룹은 만든다). */
  private chain(entry: KeyEntry): KeyGroup[] {
    const chain = [this.root];
    let group = this.root;
    let path = "";
    for (const part of this.segments(entry)) {
      path += part + this.separatorValue;
      let next = group.groups.get(part);
      if (!next) {
        next = newGroup(part, path);
        group.groups.set(part, next);
        group.sorted = false;
      }
      group = next;
      chain.push(group);
    }
    return chain;
  }

  private place(index: number) {
    const chain = this.chain(this.entries[index]!);
    const owner = chain[chain.length - 1]!;
    owner.leaves.push(index);
    owner.sorted = false;
    for (const g of chain) g.count++;
  }

  /** 빈 그룹을 걷는다(뿌리는 남긴다). */
  private prune(chain: KeyGroup[]) {
    for (let i = chain.length - 1; i > 0; i--) {
      const g = chain[i]!;
      if (g.count > 0) break;
      chain[i - 1]!.groups.delete(g.name);
    }
  }

  private sort(group: KeyGroup) {
    if (group.sorted) return;
    if (this.loading && group.leaves.length > SORT_WHILE_LOADING) return;
    if (group.groups.size > 1)
      group.groups = new Map([...group.groups.entries()].sort(([a], [b]) => byName(a, b)));
    const entries = this.entries;
    group.leaves.sort((a, b) => byName(entries[a]!.name, entries[b]!.name));
    group.sorted = true;
  }

  private changed() {
    this.version++;
    if (this.pending) return;
    this.pending = true;
    this.schedule(() => {
      this.pending = false;
      for (const l of this.listeners) l();
    });
  }
}
