// 내비게이션 기록(04 F4-8 의 뒤로 · 앞으로 · 최근 위치, 15-m4-plan M4-09 · 결정 M4-D20). 창마다 하나 — 에디터 작업 공간
// (editor-workspace.tsx)이 만들어 들고, 창이 닫히면 함께 사라진다. 프로젝트 상태(D36)에 남기지 않는다.
//
// 모양은 한 줄의 배열과 그 안의 자리(`index`)다: `entries[0, index)` 가 뒤로(오래된 것이 앞), `entries[index..]` 가 앞으로(바로 다음
// 것이 앞)다. 지금 자리는 배열에 없다 — 뒤로 갈 때 지금 자리를 앞으로 쪽 첫 칸에 넣고, 앞으로 갈 때 뒤로 쪽 끝에 넣는다(IntelliJ 의
// 뒤로 · 앞으로 스택과 같다). 새로 기록하면 앞으로 쪽은 버린다. 둘을 합쳐 `NAV_HISTORY_LIMIT` 개까지 — 넘으면 가장 오래된 것이 빠진다.
//
// 위치는 1부터 센 줄 · 열이다. 문서가 열려 있는 동안(문서 모델에 텍스트가 있는 동안)은 편집마다 CodeMirror 의 변경 매핑으로 옮긴다
// — 위쪽 줄을 지우면 기록이 같은 글자를 따라 올라간다. 닫힌 문서는 마지막으로 옮긴 줄 · 열 그대로이고, 그리로 가면 다시 연다.
//
// 기록에 넣는 때(15-m4-plan M4-09): 위치로 가기(`navigateTo(…, { record: true })` — 정의 · 구현 · 사용처 · 심볼 · 문제 · 구조에서
// 고르기, 줄로 이동 ⌘L, 파일로 이동 · 검색 결과 열기)와 같은 파일 안의 큰 이동(커서가 편집 없이 `BIG_MOVE_LINES` 줄을 넘게 옮김).
// 넣는 것은 떠나는 자리다. 큰 이동이 이어지면(PageDown 을 이어 누름 등) 새 칸을 쌓지 않는다 — 그 연속의 첫 이동이 넣은 칸(읽기
// 시작한 자리)이 마지막 칸으로 남는다. 「이어진다」 는 같은 탭에서, 그 사이에 다른 기록(위치로 가기 · 뒤로 · 앞으로)도, 사람이 옮기지
// 않은 커서 보고(편집 · 커서 여럿 · 작업 공간이 둔 커서 — `quiet`)도, 다른 탭의 커서 이동도 없었다는 뜻이다. 시간은 보지 않는다 —
// 읽으며 쉬는 사이가 길어도 한 연속이다.
//
// 루트 밖 문서(읽기 토큰으로만 여는 「외부 소스」, 결정 M4-D13)는 M4-08 이 끼운다: 기록 · 목적지의 `token` 칸, 작업 공간의
// `OutsideSources`(그 탭을 열기 · 그 탭의 자리 읽기). 기록 모델은 토큰을 해석하지 않고 그대로 들고 다닌다 — 뒤로 · 앞으로도 같은
// `open` 을 지나므로 토큰 든 기록으로 돌아가는 길이 따로 없다.

import type { ChangeSet, Text } from "@codemirror/state";

import type { DocumentStore } from "./documents";

/** 기록 하나 · 위치로 가기의 목적지. 줄 · 열은 1부터(열은 UTF-16 단위 — 에디터의 열과 같다). */
export interface NavLocation {
  /** 정규화한 실제 경로(문서 모델의 키). */
  readonly path: string;
  /** 탭의 워크트리(`undefined` 면 메인 체크아웃). */
  readonly worktree?: string;
  readonly line: number;
  readonly column: number;
  /**
   * 루트 밖 문서의 읽기 토큰(M4-08 — `lsp.document.read { token }`). 있으면 작업 공간이 `OutsideSources.open` 으로 연다. 이 행
   * (M4-09)은 채우지 않는다.
   */
  readonly token?: string;
}

/** 위치로 가기의 목적지. 줄이 없으면 파일만 연다(커서는 그 탭의 것 그대로 — 파일로 이동). */
export type NavTarget = Omit<NavLocation, "line" | "column"> & {
  readonly line?: number;
  readonly column?: number;
};

export interface NavigateOptions {
  /** 떠나는 자리를 기록에 넣는다. */
  record?: boolean;
}

/** 뒤로 · 앞으로 기록의 상한(15-m4-plan M4-09). */
export const NAV_HISTORY_LIMIT = 100;
/** 같은 파일 안에서 이만큼을 넘게 옮기면 떠난 자리를 기록에 넣는다(15-m4-plan M4-09 「10줄 넘게」). */
export const BIG_MOVE_LINES = 10;

export interface NavigationDeps {
  documents: Pick<DocumentStore, "onChange">;
  /** 지금 자리(활성 파일 탭의 커서). 파일 탭이 아니면 `undefined` — 기록에 넣을 수 없다. */
  here(): NavLocation | undefined;
  /**
   * 위치를 연다(필요하면 탭을 열고 커서를 둔다). 열지 못했으면 `false`. `travel` 이면 뒤로 · 앞으로가 기록을 따라 연다 — 열지 못한
   * 기록은 조용히 건너뛰므로 오류를 알리지 않는다(사람이 고른 목적지인 위치로 가기는 알린다).
   */
  open(location: NavTarget, how: { travel: boolean }): Promise<boolean>;
  /** 창의 활성 워크트리. 다른 워크트리의 기록은 뒤로 · 앞으로가 건너뛴다(탭 묶음이 워크트리마다라 그 자리로 갈 수 없다). */
  activeWorktree?(): string | undefined;
}

/** 같은 파일의 같은 줄. 줄 없는 목적지는 같은 파일이면 같은 자리다. */
const samePlace = (a: NavLocation, b: NavTarget) =>
  a.path === b.path && a.worktree === b.worktree && (b.line === undefined || a.line === b.line);

/** `loc` 을 편집(`changes`, `before` → `after`)에 따라 옮긴다. 지워진 범위 안이면 그 범위의 시작으로 간다. */
export function mapLocation(
  loc: NavLocation,
  changes: ChangeSet,
  before: Text,
  after: Text,
): NavLocation {
  const line = before.line(Math.min(Math.max(1, loc.line), before.lines));
  const pos = line.from + Math.min(Math.max(0, loc.column - 1), line.length);
  // 바로 그 자리에 넣은 글은 앞에 둔다 — 기록은 원래 글자를 따라간다.
  const next = changes.mapPos(pos, 1);
  const at = after.lineAt(next);
  if (at.number === loc.line && next - at.from + 1 === loc.column) return loc;
  return { ...loc, line: at.number, column: next - at.from + 1 };
}

export function createNavigation(deps: NavigationDeps) {
  let entries: NavLocation[] = [];
  let index = 0;
  /** 기록에 든 경로 → 그 문서의 바뀜 구독 해제. */
  const watched = new Map<string, () => void>();
  /** 탭(패널 id) → 마지막으로 본 커서(큰 이동 판정 · 떠난 자리). */
  const lastCursor = new Map<string, { line: number; column: number }>();
  /** 기록이 바뀐 차례(`changed`). 큰 이동의 연속이 그사이 다른 기록이 있었는지 본다. */
  let version = 0;
  /** 이어지는 큰 이동: 그 탭과, 그 연속이 칸을 넣은 때의 `version`. */
  let run: { panelId: string; version: number } | undefined;
  /** 뒤로 · 앞으로 · 위치로 가기를 차례로 한다 — 앞의 것이 탭을 다 열기 전에 다음 것이 지금 자리를 읽지 않게. */
  let queue: Promise<unknown> = Promise.resolve();
  const listeners = new Set<() => void>();

  function changed() {
    version++;
    // 기록에 든 경로만 듣는다.
    const paths = new Set(entries.map((e) => e.path));
    for (const [path, off] of watched)
      if (!paths.has(path)) {
        off();
        watched.delete(path);
      }
    for (const path of paths)
      if (!watched.has(path))
        watched.set(
          path,
          deps.documents.onChange(path, (change) => {
            if (change.kind !== "edit") return;
            entries = entries.map((e) =>
              e.path === path ? mapLocation(e, change.changes, change.before, change.after) : e,
            );
          }),
        );
    for (const listener of listeners) listener();
  }

  /** 넘친 만큼 가장 오래된 것을 뺀다. */
  function trim() {
    const over = entries.length - NAV_HISTORY_LIMIT;
    if (over <= 0) return;
    entries = entries.slice(over);
    index = Math.max(0, index - over);
  }

  function push(loc: NavLocation) {
    entries = entries.slice(0, index);
    const last = entries[entries.length - 1];
    // 같은 줄을 두 번 넣지 않는다 — 뒤로 한 번이 제자리걸음이 된다.
    if (last && samePlace(last, loc)) entries[entries.length - 1] = loc;
    else entries.push(loc);
    index = entries.length;
    trim();
    changed();
  }

  const reachable = (loc: NavLocation) =>
    !deps.activeWorktree || loc.worktree === deps.activeWorktree();

  /**
   * 뒤로(`-1`) · 앞으로(`1`) 갈 곳을 기록에서 꺼낸다. 지금 자리와 같은 줄 · 갈 수 없는 워크트리의 것은 건너뛰며 버린다. 지금
   * 자리(`here`)는 반대쪽 첫 칸에 넣는다. 갈 곳이 없으면 아무것도 바꾸지 않는다.
   */
  function take(direction: -1 | 1, here: NavLocation | undefined): NavLocation | undefined {
    const skip = (loc: NavLocation) => (here && samePlace(loc, here)) || !reachable(loc);
    if (direction < 0) {
      let i = index - 1;
      while (i >= 0 && skip(entries[i]!)) i--;
      if (i < 0) return undefined;
      const target = entries[i]!;
      entries = [...entries.slice(0, i), ...(here ? [here] : []), ...entries.slice(index)];
      index = i;
      trim();
      changed();
      return target;
    }
    let j = index;
    while (j < entries.length && skip(entries[j]!)) j++;
    if (j >= entries.length) return undefined;
    const target = entries[j]!;
    entries = [...entries.slice(0, index), ...(here ? [here] : []), ...entries.slice(j + 1)];
    index += here ? 1 : 0;
    trim();
    changed();
    return target;
  }

  /** 기록을 따라 간다. 열지 못한 기록(지워진 파일 등)은 버리고 그다음 것으로 간다. 갔으면 `true`. */
  async function travel(direction: -1 | 1): Promise<boolean> {
    let target = take(direction, deps.here());
    while (target) {
      if (await deps.open(target, { travel: true })) return true;
      // 지금 자리는 이미 반대쪽에 넣었다 — 다음 후보만 꺼낸다.
      target = take(direction, undefined);
    }
    return false;
  }

  function enqueue<T>(job: () => Promise<T>): Promise<T> {
    const next = queue.then(job, job);
    queue = next.catch(() => {});
    return next;
  }

  return {
    /**
     * 위치로 가기: 연다(탭 · 커서). `record` 면 떠나는 자리를 기록에 넣는다 — 열지 못했으면 넣지 않는다. 연 위치가 지금 자리와 같은
     * 줄이면 넣지 않는다. 갔으면 `true`.
     */
    navigateTo(location: NavTarget, options: NavigateOptions = {}): Promise<boolean> {
      return enqueue(async () => {
        const from = options.record ? deps.here() : undefined;
        if (!(await deps.open(location, { travel: false }))) return false;
        if (from && !samePlace(from, location)) push(from);
        return true;
      });
    },
    /** 뒤로(⌘[ · 마우스 4). 갔으면 `true`. */
    back: (): Promise<boolean> => enqueue(() => travel(-1)),
    /** 앞으로(⌘] · 마우스 5). 갔으면 `true`. */
    forward: (): Promise<boolean> => enqueue(() => travel(1)),
    /** 떠나는 자리를 기록에 넣는다(앞으로 쪽은 버린다). */
    push,
    /** 기록 전체, 방문 순(오래된 것이 앞). 「최근 위치」 팝업이 거꾸로 보인다. */
    entries: (): readonly NavLocation[] => entries,
    /** 뒤로 쪽 개수(시험 · 진단). */
    backCount: () => index,
    /** 기록이 바뀔 때 듣는다. 해제 함수를 돌려준다. */
    subscribe(listener: () => void): () => void {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },

    /**
     * 탭의 커서가 옮겨졌다(작업 공간의 `reportCursor`). 사람이(`quiet` 아님) 같은 탭에서 `BIG_MOVE_LINES` 줄을 넘게 옮겼으면 떠난
     * 자리를 넣는다 — 큰 이동이 이어지면 넣지 않는다(머리 주석). `quiet` 는 편집으로 밀린 커서 · 커서가 여럿인 선택 · 작업 공간이 둔
     * 커서(위치로 가기 · 열기 요청 · 복원)다 — 그 보고는 기록하지 않고 마지막 줄만 바꾸며, 큰 이동의 연속을 끊는다.
     */
    cursorMoved(
      panelId: string,
      at: { path: string; worktree?: string; token?: string },
      cursor: { line: number; column: number },
      quiet: boolean,
    ) {
      const before = lastCursor.get(panelId);
      lastCursor.set(panelId, cursor);
      if (quiet || (run && run.panelId !== panelId)) run = undefined;
      if (quiet || !before || Math.abs(cursor.line - before.line) <= BIG_MOVE_LINES) return;
      // 이어지는 큰 이동 — 연속의 첫 칸(시작한 자리)을 그대로 둔다.
      if (run && run.panelId === panelId && run.version === version) return;
      push({ ...at, line: before.line, column: before.column });
      run = { panelId, version };
    },
    /** 탭이 닫혔다 — 그 탭의 마지막 커서를 잊는다. */
    forgetPanel(panelId: string) {
      lastCursor.delete(panelId);
    },
    /** 파일 · 폴더를 옮겼다(작업 공간의 `moved`). `under` 인 경로의 기록이 `rename` 한 새 경로를 따라간다. */
    moved(under: (path: string) => boolean, rename: (path: string) => string) {
      if (!entries.some((e) => under(e.path))) return;
      entries = entries.map((e) => (under(e.path) ? { ...e, path: rename(e.path) } : e));
      changed();
    },
    dispose() {
      for (const off of watched.values()) off();
      watched.clear();
      listeners.clear();
    },
  };
}

export type Navigation = ReturnType<typeof createNavigation>;
