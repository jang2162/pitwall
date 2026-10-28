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
//   창 하나에서 동시에 도는 blame 은 `BLAME_SLOTS` 개다(`BlameSlots`, 호스트가 나눈다) — 넘으면 가장 오래된 것을 끊고, 끊긴 문서는
//   그 뷰가 포커스를 받거나 주석 칸을 켤 때(`requestBlame`) 다시 받는다. 커밋 직후 열린 탭마다 blame 이 한꺼번에 뜨지 않게.

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
import {
  type DiffStats,
  diffLines,
  docLines,
  type FileLines,
  keysOf,
  splitLines,
} from "./line-diff";

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
  /** 아직 부르지 않았거나, 다른 문서의 blame 에 자리를 내주어 끊겼다(`requestBlame` 으로 다시). */
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

/** 창 하나에서 동시에 도는 blame 의 수. */
export const BLAME_SLOTS = 2;

/** 동시에 도는 blame 의 자리(호스트가 창마다 하나 둔다). */
export class BlameSlots {
  private readonly running: { stop(): void }[] = [];

  constructor(readonly size = BLAME_SLOTS) {}

  /** 자리를 얻는다. 자리가 없으면 가장 오래된 것의 `stop` 을 부르고 그 자리를 쓴다. 돌려준 함수로 내놓는다. */
  enter(stop: () => void): () => void {
    const seat = { stop };
    this.running.push(seat);
    while (this.running.length > this.size) this.running.shift()!.stop();
    return () => {
      const at = this.running.indexOf(seat);
      if (at >= 0) this.running.splice(at, 1);
    };
  }

  /** 지금 도는 수(시험). */
  get count(): number {
    return this.running.length;
  }
}

/** 모델이 붙는 문서(`LiveText` 의 일부). */
export interface GutterText {
  readonly text: Text;
  readonly separator: string | undefined;
  readonly dirty: boolean;
  onChange(listener: () => void): () => void;
  /** 미저장 여부가 바뀔 때(저장 · 되돌림). */
  onDirtyChange(listener: () => void): () => void;
}

const EMPTY: FileLines = { lines: [], eol: true };

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
  private base: FileLines = { lines: [], eol: true };
  /** 기준의 견줄 열쇠(`keysOf(base)`). */
  private baseKeys: string[] = [];
  /** 자리를 내주어 끊긴 blame(다시 부를 것). */
  private wanted: { rel: string; rev: string } | undefined;
  private releaseSlot: (() => void) | undefined;
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
  /** 다시 견준 횟수와 마지막 비교의 비용(측정 · 시험 — 키 입력마다의 일이 줄 수에 비례하는 비교 한 번인지 본다). */
  recomputes = 0;
  lastStats: DiffStats = { middle: 0, cost: 0, anchored: 0 };

  constructor(
    private readonly source: GutterSource,
    readonly path: string,
    private readonly live: GutterText,
    private readonly slots?: BlameSlots,
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
    this.releaseSlot?.();
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
    return this.base.lines.slice(from - 1, to - 1);
  }

  /** HEAD 의 파일(작업 트리 꼴, git 처럼 센 줄). */
  get baseFile(): FileLines {
    return this.base;
  }

  /** 기준을 받은 HEAD 커밋(꺼져 있으면 없다). */
  get rev(): string | undefined {
    return this.state.kind === "on" ? this.state.rev : undefined;
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
      this.base = { lines: [], eol: true };
      this.baseKeys = [];
      this.stopBlame();
      this.emit();
    };
    if (rel === undefined || rel === "") return off({ reason: "outside" });
    if (letter === "?") return off({ reason: "untracked" });
    if (head === null) {
      // 커밋이 없는 브랜치: HEAD 에 아무것도 없다. 스테이징한 파일만 빈 기준과 견준다.
      if (letter === "A") return this.useBase(generation, "", EMPTY, "HEAD");
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
            ? this.useBase(generation, "", EMPTY, rev)
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

  private useBase(generation: number, rel: string, base: FileLines, rev: string) {
    if (generation !== this.generation) return;
    this.base = base;
    this.baseKeys = keysOf(base);
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
    const stats: DiffStats = { middle: 0, cost: 0, anchored: 0 };
    const hunks = diffLines(this.baseKeys, keysOf(docLines(this.live.text)), undefined, stats);
    this.recomputes++;
    this.lastStats = stats;
    this.state = { ...this.state, hunks };
    this.emit();
  }

  private stopBlame() {
    this.blameAbort?.abort();
    this.blameAbort = undefined;
    this.releaseSlot?.();
    this.releaseSlot = undefined;
    this.wanted = undefined;
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
    const release = (this.releaseSlot = this.slots?.enter(() => {
      // 다른 문서의 blame 에 자리를 내준다 — 다시 부를 때까지 「부르지 않음」.
      abort.abort();
      this.releaseSlot = undefined;
      this.blameAbort = undefined;
      this.blame = { kind: "idle" };
      this.wanted = { rel, rev };
      this.emit();
    }));
    const done = () => {
      release?.();
      if (this.releaseSlot === release) this.releaseSlot = undefined;
    };
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
        done();
        if (abort.signal.aborted) return;
        this.blame = { kind: "failed", message: errorText(e) };
        this.emit();
        return;
      }
      done();
      if (abort.signal.aborted) return;
      ranges.sort((a, b) => a.line - b.line);
      this.ranges = ranges;
      this.blame = { kind: "done", file, rev };
      this.emit();
    })();
  }

  /** 자리를 내주어 끊긴 blame 을 다시 받는다(뷰가 포커스를 받았다 · 주석 칸을 켰다). 그 밖에는 아무것도 하지 않는다. */
  requestBlame() {
    const wanted = this.wanted;
    if (this.blame.kind !== "idle" || !wanted || this.state.kind !== "on") return;
    this.startBlame(wanted.rel, wanted.rev);
  }

  /** 지금 텍스트의 줄(1 기준)의 blame. blame 을 아직 받지 못했으면 `undefined`. */
  lineBlame(line: number): LineBlame | undefined {
    if (this.blame.kind !== "done" || this.state.kind !== "on") return undefined;
    const base = baseLineOf(this.state.hunks, line);
    if (base === undefined) return { kind: "uncommitted" };
    // 파일 끝 줄바꿈 뒤의 빈 줄(CodeMirror 의 마지막 줄)은 git 에게 줄이 아니다 — 주석을 달지 않는다.
    if (line > docLines(this.live.text).lines.length) return undefined;
    const range = rangeAt(this.ranges, base);
    const commit = range && this.commits.get(range.commit);
    if (range && commit) return { kind: "commit", commit, range };
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
 * [되돌리기]: 문서에서 hunk 하나를 HEAD 의 줄로 바꾸는 변경(CodeMirror `ChangeSpec`). 바꾸는 자리는 hunk 의 새 줄 `[newFrom, newTo)`
 * 의 첫 줄 시작부터 `newTo` 줄 시작(그 줄이 없으면 문서 끝)까지 — 줄바꿈까지다. 넣는 것은 HEAD 의 줄마다 줄바꿈을 붙인 것이고,
 * HEAD 의 파일 끝까지 닿는데 HEAD 에 끝 줄바꿈이 없으면 마지막 줄바꿈을 뺀다. 줄은 문서의 줄 구분으로 넣는다 — 저장할 때 문서의
 * 줄바꿈으로 바뀐다(live-text.ts `serialize`). 바뀌는 것은 그 hunk 의 줄과 그 끝의 줄바꿈뿐이다.
 */
export function revertChange(
  doc: Text,
  hunk: DiffGroup,
  /** HEAD 의 파일(`GutterDoc.baseFile`). */
  base: FileLines,
  lineBreak: string,
): { from: number; to: number; insert: string } {
  const lineStart = (n: number) => (n <= doc.lines ? doc.line(n).from : doc.length);
  const from = lineStart(hunk.newFrom);
  const to = lineStart(hunk.newTo);
  let insert = base.lines
    .slice(hunk.oldFrom - 1, hunk.oldTo - 1)
    .map((l) => l + lineBreak)
    .join("");
  if (hunk.oldTo - 1 === base.lines.length && !base.eol && insert !== "")
    insert = insert.slice(0, -lineBreak.length);
  return { from, to, insert };
}
