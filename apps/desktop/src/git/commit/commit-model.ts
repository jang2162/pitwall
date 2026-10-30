// 커밋 툴 윈도우의 목록 모델(docs/design/13-m2-plan.md M2-16): 창의 git 상태(`windowGitStatus()` — M2-12 의 감시 하나)의 항목을 그룹
// 행으로 바꾼다. React 밖의 순수 함수라 시험 · 측정 하네스가 그대로 쓴다.
//
// - 그룹: 충돌 · 스테이징됨 · 변경 · 버전 관리 안 됨(IntelliJ 의 스테이징 영역 모드와 같다). 충돌을 맨 위에 둔다 — 1만 개의
//   untracked 아래에 묻히지 않게. 충돌 항목은 다른 그룹에 들지 않는다. 한 파일이 스테이징된 부분과 작업 트리 변경을 함께 가지면
//   (`MM`) 두 그룹에 모두 든다 — 변경 그룹의 체크는 「부분」(일부만 스테이징)이다.
// - 체크 = 스테이징: 스테이징됨 그룹은 체크돼 있고(누르면 스테이징을 푼다), 변경 · 버전 관리 안 됨은 비어 있다(누르면 스테이징).
// - 정렬은 경로의 코드 단위 순(`<`)이다 — 1만 개를 다시 묶을 때 `localeCompare` 는 수십 ms 가 든다.
// - 행은 그룹 머리 · 파일 · stash 머리 · stash 항목이다. 접힌 그룹의 파일은 행에서 빠진다.

import type {
  GitChange,
  GitConflict,
  GitLostChanges,
  GitStatusEntry,
  StashEntry,
} from "@pitwall/rpc-client";

import { entryLetter, type GitLetter, type GitTone } from "../git-status";

export type CommitGroup = "conflicts" | "staged" | "changes" | "untracked";

export const GROUPS: readonly CommitGroup[] = ["conflicts", "staged", "changes", "untracked"];

const LETTER: Record<GitChange, GitLetter> = {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
  copied: "C",
  typeChanged: "T",
  untracked: "?",
};

const TONE: Record<GitLetter, GitTone> = {
  A: "added",
  C: "added",
  M: "modified",
  T: "modified",
  D: "deleted",
  R: "renamed",
  "?": "untracked",
  U: "conflicted",
};

/** 목록의 파일 하나(그룹 안). */
export interface CommitItem {
  /** 그룹 · 경로로 만든 고유 키. */
  readonly key: string;
  readonly group: CommitGroup;
  /** 작업 트리 꼭대기 기준 상대 경로(`/`). */
  readonly path: string;
  /** 스테이징된 이름 바뀜의 옛 경로(스테이징됨 그룹만). */
  readonly origPath: string | null;
  /** 그 그룹에서 보이는 글자 · 색. */
  readonly letter: GitLetter;
  readonly tone: GitTone;
  /** 변경 그룹에서, 같은 파일에 스테이징된 부분도 있다. */
  readonly partial: boolean;
  readonly conflict: GitConflict | null;
  readonly entry: GitStatusEntry;
}

export type CommitGroups = Readonly<Record<CommitGroup, readonly CommitItem[]>>;

const byPath = (a: CommitItem, b: CommitItem) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0);

function item(
  group: CommitGroup,
  entry: GitStatusEntry,
  letter: GitLetter,
  extra?: Partial<Pick<CommitItem, "origPath" | "partial">>,
): CommitItem {
  return {
    key: `${group}:${entry.path}`,
    group,
    path: entry.path.replace(/\/+$/, ""),
    origPath: extra?.origPath ?? null,
    letter,
    tone: TONE[letter],
    partial: extra?.partial ?? false,
    conflict: entry.conflict,
    entry,
  };
}

/** status 항목을 그룹으로 나눈다(머리 주석). */
export function groupEntries(entries: readonly GitStatusEntry[]): CommitGroups {
  const out: Record<CommitGroup, CommitItem[]> = {
    conflicts: [],
    staged: [],
    changes: [],
    untracked: [],
  };
  for (const entry of entries) {
    if (entry.conflict) {
      out.conflicts.push(item("conflicts", entry, entryLetter(entry)));
      continue;
    }
    if (entry.unstaged === "untracked") {
      out.untracked.push(item("untracked", entry, "?"));
      continue;
    }
    if (entry.staged)
      out.staged.push(item("staged", entry, LETTER[entry.staged], { origPath: entry.origPath }));
    if (entry.unstaged)
      out.changes.push(
        item("changes", entry, LETTER[entry.unstaged], { partial: entry.staged !== null }),
      );
  }
  for (const g of GROUPS) out[g].sort(byPath);
  return out;
}

export type CommitRow =
  | { readonly kind: "group"; readonly group: CommitGroup; readonly count: number }
  | { readonly kind: "file"; readonly item: CommitItem }
  | { readonly kind: "stashHeader"; readonly count: number }
  | { readonly kind: "stash"; readonly stash: StashEntry };

/** 그리는 행. 빈 그룹은 빼고(머리도), 접힌 그룹은 머리만 둔다. stash 머리는 늘 끝에 있다. */
export function commitRows(
  groups: CommitGroups,
  collapsed: ReadonlySet<string>,
  stashes: readonly StashEntry[],
): CommitRow[] {
  const rows: CommitRow[] = [];
  for (const group of GROUPS) {
    const items = groups[group];
    if (items.length === 0) continue;
    rows.push({ kind: "group", group, count: items.length });
    if (!collapsed.has(group)) for (const i of items) rows.push({ kind: "file", item: i });
  }
  rows.push({ kind: "stashHeader", count: stashes.length });
  if (!collapsed.has("stash")) for (const stash of stashes) rows.push({ kind: "stash", stash });
  return rows;
}

/** 체크 상태: 체크됨 · 일부 · 빔. 충돌은 체크가 없다. */
export type CheckState = "checked" | "mixed" | "unchecked" | "none";

export function itemCheck(i: CommitItem): CheckState {
  switch (i.group) {
    case "conflicts":
      return "none";
    case "staged":
      return "checked";
    case "changes":
      return i.partial ? "mixed" : "unchecked";
    case "untracked":
      return "unchecked";
  }
}

export function groupCheck(group: CommitGroup, items: readonly CommitItem[]): CheckState {
  if (group === "conflicts") return "none";
  if (group === "staged") return "checked";
  return items.some((i) => i.partial) ? "mixed" : "unchecked";
}

/** 스테이징을 풀 때 넘길 경로(이름 바뀜은 옛 경로도 — `git.unstage` 의 규칙). */
export function unstagePaths(items: readonly CommitItem[]): string[] {
  const out = new Set<string>();
  for (const i of items) {
    out.add(i.path);
    if (i.origPath) out.add(i.origPath);
  }
  return [...out];
}

/** 변경 되돌리기(⌘⌥Z)가 할 일과 그것이 디스크에서 바꾸는 것(확인 다이얼로그의 목록). */
export interface RollbackPlan {
  /** `git.unstage` 에 넘길 경로(스테이징됨 그룹에서 고른 것, 이름 바뀜은 옛 경로도). 비면 부르지 않는다. */
  readonly unstage: string[];
  /** `git.discard` 에 넘길 경로(경로순). */
  readonly discard: string[];
  /** 잃는 것: 작업 트리가 HEAD(스테이징을 풀지 않으면 index)로 돌아가는 변경과 덮이는 추적 안 된 파일. */
  readonly lost: GitLostChanges;
  /** 지우지 않고 디스크에 그대로 두는 새 파일(스테이징을 풀면 버전 관리 안 됨이 된다). 경로순. */
  readonly kept: string[];
}

/** 스테이징이 HEAD 에 없는 경로를 더했다(그 경로를 HEAD 로 돌릴 것이 없다). */
const NEW_IN_INDEX: ReadonlySet<GitChange | null> = new Set(["added", "copied", "renamed"]);

/**
 * 고른 항목(스테이징됨 · 변경 그룹의 것만 넘긴다)의 되돌리기: 스테이징된 것은 스테이징을 풀고 작업 트리를 index 로 돌린다 — 둘을 거치면 HEAD 다.
 * `git.discard` 는 index 에 없는 경로와 intent-to-add 파일을 지우지 않는다(crates/pw-git/src/stage/mod.rs `split_tracked`). 그래서:
 *
 * - 스테이징을 푸는 새 파일(추가 · 복사 · 이름 바뀜의 새 경로)은 디스크에 남는다 — 작업 트리에서 지운 것(`AD`)만 남을 것이 없어
 *   스테이징한 내용을 잃는다. 이름 바뀜은 옛 경로가 HEAD 의 것으로 돌아오므로 그 바뀜도 잃는 쪽에 든다.
 * - 스테이징을 풀지 않으면(변경 그룹만 고름) 작업 트리를 index 로 돌린다 — intent-to-add 파일(`.A`)만 그대로 남는다.
 * - 이름 바뀜의 옛 경로에 추적 안 된 파일이 있으면(`untracked`) 그 파일은 HEAD 의 내용으로 덮인다.
 *
 * 무엇을 잃고 무엇이 남는지가 진짜 git 과 같은지는 fixtures/discard.json(crates/pw-git `commit::tests::discard_fixture`)이 본다.
 */
export function rollbackPlan(
  chosen: readonly CommitItem[],
  untracked: readonly string[],
): RollbackPlan {
  const staged = chosen.filter((i) => i.group === "staged");
  const unstaging = new Set(staged.map((i) => i.path));
  const entries = new Map<string, GitStatusEntry>();
  for (const i of chosen) entries.set(i.path, i.entry);
  const outside = new Set(untracked);
  const changes: GitStatusEntry[] = [];
  const overwritten = new Set<string>();
  const kept: string[] = [];
  for (const [path, e] of entries) {
    const unstage = unstaging.has(path);
    const keeps = unstage
      ? NEW_IN_INDEX.has(e.staged) && e.unstaged !== "deleted"
      : e.unstaged === "added";
    if (keeps) kept.push(path);
    if (!keeps || (unstage && e.staged === "renamed")) changes.push(e);
    if (unstage && e.origPath && e.staged === "renamed" && outside.has(e.origPath))
      overwritten.add(e.origPath);
  }
  const order = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);
  return {
    unstage: unstagePaths(staged),
    discard: unstagePaths(chosen).sort(order),
    lost: {
      changes: changes.sort((a, b) => order(a.path, b.path)),
      overwritten: [...overwritten].sort(order),
    },
    kept: kept.sort(order),
  };
}

/**
 * 템플릿(`commit.template`)에서 온 주석 줄을 걷는다: 메시지의 줄 중 템플릿의 주석 글자(`core.commentChar`, 기본 `#`)로 시작하는 줄과
 * 글자 그대로 같은 것만. 커밋은
 * `git commit -F -` 라 git 의 기본 정리(`whitespace`)가 주석 줄을 남긴다 — 사람이 손대지 않은 템플릿 안내문이 커밋 메시지에 들지
 * 않게 한다. 사람이 쓴 `#` 줄(`#123 고침`)은 템플릿에 없으면 남는다.
 */
export function withoutTemplateComments(
  message: string,
  template: string | undefined,
  commentChar = "#",
): string {
  if (!template) return message;
  const comments = new Set(
    template
      .replaceAll("\r\n", "\n")
      .split("\n")
      .filter((l) => l.startsWith(commentChar)),
  );
  if (comments.size === 0) return message;
  return message
    .split("\n")
    .filter((l) => !comments.has(l.replace(/\r$/, "")))
    .join("\n")
    .replace(/\n+$/, "\n");
}

/**
 * 템플릿을 손대지 않았다: 템플릿 주석을 걷은 메시지가 템플릿에서 주석을 걷은 것과 같다(앞뒤 공백 무시). git 은 이때 「메시지를 고치지
 * 않았다」 로 커밋하지 않는다 — 창도 부르지 않는다.
 */
export function templateUnedited(
  message: string,
  template: string | undefined,
  commentChar = "#",
): boolean {
  if (!template) return false;
  const clean = (text: string) => withoutTemplateComments(text, template, commentChar).trim();
  return clean(message.replaceAll("\r\n", "\n")) === clean(template.replaceAll("\r\n", "\n"));
}

/** 고르기: 클릭 · ⌘ · ⇧ 를 고른 키 목록(행 차례)으로. 파일 행만 고른다. */
export function selectItems(
  rows: readonly CommitRow[],
  selected: readonly string[],
  anchor: string | undefined,
  key: string,
  mode: "single" | "toggle" | "range",
): { selected: string[]; anchor: string } {
  if (mode === "toggle")
    return {
      selected: selected.includes(key) ? selected.filter((k) => k !== key) : [...selected, key],
      anchor: key,
    };
  if (mode === "range" && anchor !== undefined) {
    const keys = rows.flatMap((r) => (r.kind === "file" ? [r.item.key] : []));
    const from = keys.indexOf(anchor);
    const to = keys.indexOf(key);
    if (from >= 0 && to >= 0) {
      const [a, b] = from <= to ? [from, to] : [to, from];
      return { selected: keys.slice(a, b + 1), anchor };
    }
  }
  return { selected: [key], anchor: key };
}

/** 목록에 보이는 줄(많으면 줄인다). */
export function listLines(lines: readonly string[], max: number, more: (n: number) => string) {
  const shown = lines.slice(0, max);
  if (lines.length > max) shown.push(more(lines.length - max));
  return shown.join("\n");
}
