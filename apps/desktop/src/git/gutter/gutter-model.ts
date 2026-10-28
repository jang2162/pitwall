// 문서 하나의 거터 · blame 모델(13-m2-plan M2-17, 결정 M2-D9). 같은 문서를 여러 탭이 보여도 하나다(`LiveText` 마다) — 탭의 뷰는
// `attach` 로 붙고 마지막 뷰가 떨어지면 모델을 버린다(스트림을 닫는다).
//
// - 기준: HEAD 의 그 파일을 작업 트리 꼴로(`git.blob { rev: HEAD 의 해시, workTreeForm: true }`) 한 번 받는다. HEAD 가 움직이면
//   (커밋 · 체크아웃 — 창의 브랜치 상태 `useBranch` 의 HEAD 해시) 다시 받는다. 그 파일의 git 상태가 바뀌어도 다시 본다(untracked 였던
//   파일을 커밋하는 등). 감시를 따로 열지 않는다 — 창의 git 상태 저장소(git-status.ts)와 브랜치 저장소(branch-state.ts)를 듣는다.
// - 끄는 경우(`off`): 저장소 밖 · 저장소 없음 · untracked(HEAD 에 없고 스테이징하지도 않음) · 외부 필터 드라이버가 걸린 파일
//   (`filter`) · HEAD 쪽이 바이너리이거나 대용량. HEAD 에 없지만 스테이징한 새 파일(`A`)은 빈 기준과 견준다(전부 더한 줄).
// - 바뀜: 문서가 바뀔 때마다 `RECOMPUTE_MS` 안에 한 번 기준과 견준다(line-diff.ts). 결과(`hunks`)를 뷰에 알린다.
// - blame: 기준을 받은 뒤 `git.blame { rev: 같은 해시 }` 을 스트림으로 받는다. 범위는 HEAD 의 줄 번호라 지금 텍스트의 줄로 옮길 때
//   거터와 같은 hunk 를 쓴다 — hunk 안의 새 줄(저장 안 한 편집 · 저장했으나 커밋 안 한 변경)은 「아직 커밋 안 됨」 이다(M2-05).

import type { Text } from "@codemirror/state";
import {
  ERROR_CODE,
  type BlameChunk,
  type BlameCommit,
  type BlameFile,
  type BlameRange,
  type DiffFile,
  type GitBlob,
  type GitBlobParams,
  type HunkSelection,
} from "@pitwall/rpc-client";

import type { DiffGroup } from "../diff/diff-model";
import { diffLines, splitLines } from "./line-diff";

/** 문서가 바뀐 뒤 다시 견주기까지의 최대 지연(ms). 거터 표시는 이것 + 계산 시간 안에 따라온다(완료 기준 100 ms). */
export const RECOMPUTE_MS = 30;

/** 거터를 끈 까닭. */
export type GutterOff =
  /** 저장소가 없거나 그 밖의 파일이다. */
  | { reason: "outside" }
  /** HEAD 에 없고 스테이징하지도 않았다. */
  | { reason: "untracked" }
  /** 외부 필터 드라이버(LFS 등)가 걸려 있다 — 기준을 만들 수 없다. */
  | { reason: "filter"; driver: string }
  /** HEAD 쪽이 바이너리 · 대용량이거나 읽지 못했다. */
  | { reason: "unreadable"; message?: string };

export type GutterState =
  | { kind: "loading" }
  | ({ kind: "off" } & GutterOff)
  | { kind: "on"; rev: string; hunks: readonly DiffGroup[] };

/** 지금 텍스트의 줄 하나의 blame. */
export type LineBlame =
  | { kind: "commit"; commit: BlameCommit; range: BlameRange }
  /** 맞는 HEAD 줄이 없다(편집 · 커밋 안 한 변경). */
  | { kind: "uncommitted" };

export type BlameState =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "failed"; message: string }
  | { kind: "done"; file: BlameFile | null; rev: string };

/** 창의 Host 와 git 상태에서 모델이 쓰는 것. 창은 RPC 를, 시험은 가짜를 넘긴다. */
export interface GutterSource {
  /** 절대 경로 → 작업 트리 기준 상대 경로(`/`). 저장소 밖이면 `undefined`. */
  relative(path: string): string | undefined;
  /** 그 파일의 git 상태 글자(git-status.ts `entryLetter`). 바뀐 것이 없으면 `undefined`. */
  letter(path: string): string | undefined;
  /** HEAD 의 커밋 해시. 아직 모르면 `undefined`(그때는 `HEAD` 로 부른다). 태어나지 않은 브랜치면 `null`. */
  head(): string | null | undefined;
  /** 위 셋이 바뀔 수 있을 때 부른다. 해제 함수를 돌려준다. */
  subscribe(listener: () => void): () => void;
  blob(params: Pick<GitBlobParams, "rev" | "path" | "workTreeForm">): Promise<GitBlob>;
  blame(params: { path: string; rev: string }, signal: AbortSignal): AsyncIterable<BlameChunk>;
  /** 작업 트리 diff(index ↔ 작업 트리)의 그 파일. 없으면(바뀐 것 없음) `undefined`. */
  workTreeDiff(path: string): Promise<DiffFile | undefined>;
  stageLines(path: string, hunks: HunkSelection[]): Promise<void>;
}

/** 오류의 RPC 코드(`NOT_FOUND` 등). */
function codeOf(e: unknown): number | undefined {
  const code = (e as { code?: unknown } | null)?.code;
  return typeof code === "number" ? code : undefined;
}

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** 모델이 붙는 문서(`LiveText` 의 일부). */
export interface GutterText {
  readonly text: Text;
  readonly separator: string | undefined;
  readonly dirty: boolean;
  onChange(listener: () => void): () => void;
  /** 미저장 여부가 바뀔 때(저장 · 되돌림). */
  onDirtyChange(listener: () => void): () => void;
}

/** 지금 줄 → HEAD 줄(`undefined` 면 맞는 줄이 없다). hunk 는 줄 순이다. */
export function baseLineOf(hunks: readonly DiffGroup[], line: number): number | undefined {
  let lo = 0;
  let hi = hunks.length;
  // `newTo <= line` 인 마지막 hunk 를 찾는다(그 뒤로는 줄이 그만큼 옮겨졌다).
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (hunks[mid]!.newFrom <= line) lo = mid + 1;
    else hi = mid;
  }
  const at = lo - 1;
  if (at < 0) return line;
  const h = hunks[at]!;
  if (line < h.newTo) return undefined;
  return line - h.newTo + h.oldTo;
}

export class GutterDoc {
  state: GutterState = { kind: "loading" };
  blame: BlameState = { kind: "idle" };
  /** HEAD 의 줄(작업 트리 꼴). `on` 일 때만 뜻이 있다. */
  private base: string[] = [];
  /** blame 범위를 첫 줄 순으로. */
  private ranges: BlameRange[] = [];
  private readonly commits = new Map<string, BlameCommit>();
  private readonly listeners = new Set<() => void>();
  private generation = 0;
  private blameAbort: AbortController | undefined;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private readonly offText: () => void;
  private readonly offSource: () => void;
  /** 마지막으로 기준을 받은 HEAD · 상태 글자. 바뀌면 다시 받는다. */
  private seen: { head: string | null | undefined; letter: string | undefined; rel?: string } = {
    head: undefined,
    letter: undefined,
  };
  disposed = false;

  constructor(
    private readonly source: GutterSource,
    readonly path: string,
    private readonly live: GutterText,
  ) {
    this.offText = live.onChange(() => this.schedule());
    this.offSource = source.subscribe(() => this.sourceChanged());
    this.refresh();
  }

  /** 상태 · hunk · blame 이 바뀔 때 부른다. */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private emit() {
    for (const listener of this.listeners) listener();
  }

  dispose() {
    this.disposed = true;
    this.generation++;
    clearTimeout(this.timer);
    this.blameAbort?.abort();
    this.offText();
    this.offSource();
    this.listeners.clear();
  }

  /** 지금 HEAD 기준의 hunk. 꺼져 있으면 빈다. */
  get hunks(): readonly DiffGroup[] {
    return this.state.kind === "on" ? this.state.hunks : [];
  }

  /** HEAD 의 줄 `[from, to)`(1 기준). */
  baseLines(from: number, to: number): string[] {
    return this.base.slice(from - 1, to - 1);
  }

  private sourceChanged() {
    const head = this.source.head();
    const letter = this.source.letter(this.path);
    const rel = this.source.relative(this.path);
    if (head === this.seen.head && letter === this.seen.letter && rel === this.seen.rel) return;
    this.refresh();
  }

  /** 기준을 (다시) 받는다. */
  refresh() {
    const generation = ++this.generation;
    const head = this.source.head();
    const letter = this.source.letter(this.path);
    const rel = this.source.relative(this.path);
    this.seen = { head, letter, rel };
    const off = (why: GutterOff) => {
      this.state = { kind: "off", ...why };
      this.base = [];
      this.stopBlame();
      this.emit();
    };
    if (rel === undefined || rel === "") return off({ reason: "outside" });
    if (letter === "?") return off({ reason: "untracked" });
    if (head === null) {
      // 커밋이 없는 브랜치: HEAD 에 아무것도 없다. 스테이징한 파일만 빈 기준과 견준다.
      if (letter === "A") return this.useBase(generation, "", [], "HEAD");
      return off({ reason: "untracked" });
    }
    const rev = head ?? "HEAD";
    void (async () => {
      let blob: GitBlob;
      try {
        blob = await this.source.blob({ rev, path: rel, workTreeForm: true });
      } catch (e) {
        if (generation !== this.generation) return;
        if (codeOf(e) === ERROR_CODE.NOT_FOUND)
          return letter === "A"
            ? this.useBase(generation, "", [], rev)
            : off({ reason: "untracked" });
        return off({ reason: "unreadable", message: errorText(e) });
      }
      if (generation !== this.generation) return;
      if (blob.filter) return off({ reason: "filter", driver: blob.filter });
      if (blob.tooLarge || blob.judgment?.binary || blob.content?.kind !== "text")
        return off({ reason: "unreadable" });
      this.useBase(generation, rel, splitLines(blob.content.text, this.live.separator), rev);
    })();
  }

  private useBase(generation: number, rel: string, base: string[], rev: string) {
    if (generation !== this.generation) return;
    this.base = base;
    this.state = { kind: "on", rev, hunks: [] };
    this.recompute();
    this.startBlame(rel, rev);
  }

  private schedule() {
    if (this.state.kind !== "on" || this.timer !== undefined) return;
    this.timer = setTimeout(() => {
      this.timer = undefined;
      this.recompute();
    }, RECOMPUTE_MS);
  }

  /** 지금 텍스트를 기준과 견준다. */
  recompute() {
    if (this.state.kind !== "on") return;
    clearTimeout(this.timer);
    this.timer = undefined;
    const hunks = diffLines(this.base, this.live.text.toJSON());
    this.state = { ...this.state, hunks };
    this.emit();
  }

  private stopBlame() {
    this.blameAbort?.abort();
    this.blameAbort = undefined;
    this.ranges = [];
    this.commits.clear();
    this.blame = { kind: "idle" };
  }

  /** HEAD 의 blame 을 받는다. 커밋이 없는 브랜치 · 새 파일이면 모든 줄이 「아직 커밋 안 됨」 이다. */
  private startBlame(rel: string, rev: string) {
    this.stopBlame();
    if (rel === "") {
      this.blame = { kind: "done", file: null, rev };
      this.emit();
      return;
    }
    const abort = (this.blameAbort = new AbortController());
    this.blame = { kind: "loading" };
    void (async () => {
      let file: BlameFile | null = null;
      const ranges: BlameRange[] = [];
      try {
        for await (const chunk of this.source.blame({ path: rel, rev }, abort.signal)) {
          if (abort.signal.aborted) return;
          file ??= chunk.file;
          for (const c of chunk.commits) this.commits.set(c.hash, c);
          ranges.push(...chunk.ranges);
        }
      } catch (e) {
        if (abort.signal.aborted) return;
        this.blame = { kind: "failed", message: errorText(e) };
        this.emit();
        return;
      }
      if (abort.signal.aborted) return;
      ranges.sort((a, b) => a.line - b.line);
      this.ranges = ranges;
      this.blame = { kind: "done", file, rev };
      this.emit();
    })();
  }

  /** 지금 텍스트의 줄(1 기준)의 blame. blame 을 아직 받지 못했으면 `undefined`. */
  lineBlame(line: number): LineBlame | undefined {
    if (this.blame.kind !== "done" || this.state.kind !== "on") return undefined;
    const base = baseLineOf(this.state.hunks, line);
    if (base === undefined) return { kind: "uncommitted" };
    const range = rangeAt(this.ranges, base);
    const commit = range && this.commits.get(range.commit);
    if (range && commit) return { kind: "commit", commit, range };
    // 파일 끝 줄바꿈 뒤의 빈 줄(CodeMirror 의 마지막 줄)은 git 에게 줄이 아니다 — 주석을 달지 않는다.
    const lines = this.blame.file?.lines ?? 0;
    if (base > lines && this.base[base - 1] === "" && base === this.base.length) return undefined;
    return { kind: "uncommitted" };
  }

  /** 지금 텍스트에서 그 hunk 의 새 줄이 보이는 첫 줄 · 끝 줄(스크롤 · 팝업 자리). */
  hunkAt(line: number): number | undefined {
    const hunks = this.hunks;
    for (let i = 0; i < hunks.length; i++) {
      const h = hunks[i]!;
      const last = Math.max(h.newFrom, h.newTo - 1);
      // 지운 묶음(`newFrom == newTo`)은 그 앞 줄의 아래 경계에 그려진다.
      const first = h.newFrom === h.newTo ? Math.max(1, h.newFrom - 1) : h.newFrom;
      if (line >= first && line <= last) return i;
    }
    return undefined;
  }

  /** 미저장 편집이 있다(스테이징은 저장된 텍스트로만 한다). */
  get dirty(): boolean {
    return this.live.dirty;
  }
}

function rangeAt(ranges: readonly BlameRange[], line: number): BlameRange | undefined {
  let lo = 0;
  let hi = ranges.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (ranges[mid]!.line <= line) lo = mid + 1;
    else hi = mid;
  }
  const r = ranges[lo - 1];
  return r && line < r.line + r.count ? r : undefined;
}

/**
 * [되돌리기]: 문서에서 hunk 하나를 HEAD 의 줄로 바꾸는 변경(CodeMirror `ChangeSpec`). 줄은 문서의 줄 구분으로 넣는다 — 저장할 때
 * 문서의 줄바꿈으로 바뀐다(live-text.ts `serialize`). 바뀌는 것은 그 hunk 의 줄과 그 경계의 줄바꿈뿐이다.
 */
export function revertChange(
  doc: Text,
  hunk: DiffGroup,
  /** HEAD 의 줄 `[oldFrom, oldTo)`(`GutterDoc.baseLines`). */
  lines: readonly string[],
  lineBreak: string,
): { from: number; to: number; insert: string } {
  const removes = hunk.newTo > hunk.newFrom;
  const adds = lines.length > 0;
  if (removes && adds)
    return {
      from: doc.line(hunk.newFrom).from,
      to: doc.line(hunk.newTo - 1).to,
      insert: lines.join(lineBreak),
    };
  if (removes) {
    // 더한 줄만: 그 줄들과 줄바꿈 하나를 지운다. 파일 끝이면 앞 줄의 줄바꿈을 지운다.
    if (hunk.newTo <= doc.lines)
      return { from: doc.line(hunk.newFrom).from, to: doc.line(hunk.newTo).from, insert: "" };
    const from = hunk.newFrom > 1 ? doc.line(hunk.newFrom - 1).to : 0;
    return { from, to: doc.length, insert: "" };
  }
  // 지운 줄만: 그 자리(줄 `newFrom` 앞)에 HEAD 의 줄을 넣는다. 파일 끝이면 마지막 줄 뒤에.
  if (hunk.newFrom <= doc.lines) {
    const at = doc.line(hunk.newFrom).from;
    return { from: at, to: at, insert: lines.join(lineBreak) + lineBreak };
  }
  return { from: doc.length, to: doc.length, insert: lineBreak + lines.join(lineBreak) };
}

/**
 * [스테이징]: 거터 hunk(HEAD ↔ 문서 — 문서가 디스크와 같을 때)와 겹치는 작업 트리 diff(index ↔ 작업 트리)의 줄을 고른다. 더한 줄은
 * 새 줄 번호가 거터 hunk 의 새 범위 안이면, 뺀 줄은 그 줄이 끼어 있던 자리(다음 새 줄)가 범위 안(지운 묶음이면 그 자리)이면 고른다.
 * 이미 스테이징한 줄은 작업 트리 diff 에 없으므로 고르지 않는다 — 고를 것이 없으면 빈 목록이다.
 */
export function stageSelection(file: Pick<DiffFile, "hunks">, hunk: DiffGroup): HunkSelection[] {
  const inside = (anchor: number) =>
    hunk.newFrom === hunk.newTo
      ? anchor === hunk.newFrom
      : anchor >= hunk.newFrom && anchor < hunk.newTo;
  const out: HunkSelection[] = [];
  for (const h of file.hunks) {
    let next = h.newLines === 0 ? h.newStart + 1 : h.newStart;
    const picked: number[] = [];
    let changes = 0;
    h.lines.forEach((line, i) => {
      if (line.kind === "context") {
        next = (line.newLine ?? next) + 1;
        return;
      }
      changes++;
      if (line.kind === "added") {
        const n = line.newLine ?? next;
        if (inside(n)) picked.push(i);
        next = n + 1;
      } else if (inside(next)) picked.push(i);
    });
    if (picked.length > 0) out.push({ hunk: h, lines: picked.length === changes ? null : picked });
  }
  return out;
}
