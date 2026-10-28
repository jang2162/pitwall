// 창 하나의 거터 · blame 호스트(13-m2-plan M2-17): 문서마다의 모델(gutter-model.ts `GutterDoc`)을 탭 사이에 나눠 주고, 거터 ·
// blame 확장이 부를 것(문자열 · 액션 · 알림 · 스테이징)을 모은다. 창은 Host RPC 로 만든 것을(git-gutter.tsx), 시험은 가짜 소스로
// 만든 것을 에디터 확장의 context(`EditorFeatureContext.git`)로 넘긴다.
//
// 행 사이 이음매(액션 id 로만 부른다 — 그 코드를 import 하지 않는다):
//   registry.run("git.log.reveal", { rev })    blame 주석 클릭 → Git 로그에서 그 커밋 선택(M2-18)
//   registry.run("git.showFileHistory", { path }) 주석 칸 우클릭 「파일 히스토리」(M2-18 의 ⌃⌥H, `path` 는 문서의 절대 경로)
// 둘 다 `isAvailable` 일 때만 보인다(누를 수 있다).
//
// 스테이징(`plan` · `stage`): 미저장 편집이 있으면 하지 않는다(팝업이 버튼을 끄고 까닭을 보인다). 저장된 문서면 작업 트리 diff ·
// HEAD 와 index 의 그 파일(저장소 꼴)을 받은 뒤 디스크의 `version` 을 읽어 문서의 것과 견준다 — 다르면(그사이 디스크가 바뀜) 부르지
// 않는다. 고른 줄은 stage-plan.ts `planStage` — 그 hunk 하나만 정확히 담을 수 없으면 부르지 않고 까닭을 보인다(`inexact`).
//
// blame 은 창 하나에서 동시에 `BLAME_SLOTS` 개만 돈다(`slots`) — 가장 오래된 것부터 끊는다(gutter-model.ts 머리 주석).

import type { DiffGroup } from "../diff/diff-model";
import { ERROR_CODE, type GitBlob, type HunkSelection } from "@pitwall/rpc-client";

import { BlameSlots, GutterDoc, type GutterSource, type GutterText } from "./gutter-model";
import { docLines, type FileLines } from "./line-diff";
import { planStage, storedLines } from "./stage-plan";
import type { GitGutterStrings } from "./strings";

/** Git 로그에서 커밋을 고르는 액션(M2-18 이 Git 로그 툴 윈도우에 더한다). */
export const REVEAL_ACTION = "git.log.reveal";
/** 파일 히스토리(⌃⌥H, M2-18 — `{ path?, worktree? }`). */
export const HISTORY_ACTION = "git.showFileHistory";
/** 리비전의 파일을 blame 주석과 함께 연다(git-gutter.tsx 가 등록한다). */
export const BLAME_REVISION_ACTION = "git.blameRevision";

export type StageOutcome =
  | { kind: "staged" }
  | { kind: "dirty" }
  | { kind: "already" }
  /** git 의 diff 와 정렬이 달라 그 hunk 하나만 정확히 담을 수 없다. */
  | { kind: "inexact" }
  | { kind: "diskChanged" }
  | { kind: "failed"; message: string };

/** 스테이징하기 전의 판정: 부를 수 있으면 `ready`, 아니면 그 까닭(`StageOutcome` 의 나머지). */
export type StageCheck =
  | { kind: "ready"; rel: string; hunks: HunkSelection[] }
  | Exclude<StageOutcome, { kind: "staged" }>;

export interface GutterHostDeps {
  source: GutterSource;
  strings: GitGutterStrings;
  available(id: string): boolean;
  run(id: string, args?: unknown): boolean;
  /** 문서가 디스크에 있다고 아는 `version`(documents.ts)과 지금 디스크의 `version`. 같아야 스테이징한다. */
  versions(path: string): Promise<{ document: string | null; disk: string | null }>;
  notify(level: "info" | "warning" | "error", title: string, body?: string): void;
  /** 브라우저 언어(날짜 · 상대 시각). */
  locale?: string;
}

interface Held {
  doc: GutterDoc;
  count: number;
}

export class GutterHost {
  private readonly docs = new WeakMap<GutterText, Held>();
  /** 창 하나에서 동시에 도는 blame 의 자리. */
  readonly slots = new BlameSlots();

  constructor(readonly deps: GutterHostDeps) {}

  get strings(): GitGutterStrings {
    return this.deps.strings;
  }

  get source(): GutterSource {
    return this.deps.source;
  }

  /** 문서의 모델을 잡는다. 같은 `live` 를 여러 뷰가 잡으면 하나를 나눈다. 돌려준 함수로 놓는다 — 마지막에 놓으면 버린다. */
  acquire(path: string, live: GutterText): { doc: GutterDoc; release(): void } {
    let held = this.docs.get(live);
    if (!held || held.doc.disposed || held.doc.path !== path) {
      held?.doc.dispose();
      held = { doc: new GutterDoc(this.deps.source, path, live, this.slots), count: 0 };
      this.docs.set(live, held);
    }
    held.count++;
    const mine = held;
    let released = false;
    return {
      doc: mine.doc,
      release: () => {
        if (released) return;
        released = true;
        if (--mine.count > 0) return;
        mine.doc.dispose();
        if (this.docs.get(live) === mine) this.docs.delete(live);
      },
    };
  }

  available(id: string): boolean {
    return this.deps.available(id);
  }

  run(id: string, args?: unknown): boolean {
    return this.deps.run(id, args);
  }

  /** hunk 하나를 스테이징할 수 있는지 본다(머리 주석). 부를 RPC 의 인자까지 만든다. */
  async plan(doc: GutterDoc, hunk: DiffGroup, live: GutterText): Promise<StageCheck> {
    if (doc.dirty) return { kind: "dirty" };
    const rel = this.deps.source.relative(doc.path);
    const rev = doc.rev;
    if (rel === undefined || rev === undefined)
      return { kind: "failed", message: "outside the repository" };
    try {
      const [file, head, index] = await Promise.all([
        this.deps.source.workTreeDiff(rel),
        // 커밋이 없는 브랜치면 HEAD 에 아무것도 없다.
        this.deps.source.head() === null
          ? Promise.resolve<FileLines>({ lines: [], eol: true })
          : stored(this.deps.source, rev, rel, true),
        stored(this.deps.source, null, rel, false),
      ]);
      const { document, disk } = await this.deps.versions(doc.path);
      if (doc.dirty) return { kind: "dirty" };
      if (document === null || document !== disk) return { kind: "diskChanged" };
      if (!head || !index) return { kind: "inexact" };
      const plan = planStage({
        head,
        index,
        diff: file,
        hunk,
        workTreeLines: docLines(live.text).lines.length,
      });
      return plan.kind === "ready" ? { kind: "ready", rel, hunks: plan.hunks } : plan;
    } catch (e) {
      return { kind: "failed", message: e instanceof Error ? e.message : String(e) };
    }
  }

  /** hunk 하나를 스테이징한다 — 부르기 직전에 다시 판정한다. */
  async stage(doc: GutterDoc, hunk: DiffGroup, live: GutterText): Promise<StageOutcome> {
    const check = await this.plan(doc, hunk, live);
    if (check.kind !== "ready") return check;
    try {
      await this.deps.source.stageLines(check.rel, check.hunks);
      return { kind: "staged" };
    } catch (e) {
      return { kind: "failed", message: e instanceof Error ? e.message : String(e) };
    }
  }

  /** 시각(유닉스 초) → 날짜(주석 칸). */
  date(seconds: number): string {
    return new Date(seconds * 1000).toLocaleDateString(this.deps.locale);
  }

  /** 시각 → 날짜와 시각(툴팁). */
  dateTime(seconds: number): string {
    return new Date(seconds * 1000).toLocaleString(this.deps.locale);
  }

  /** 시각 → 「3일 전」(인라인 blame). */
  relative(seconds: number, now = Date.now()): string {
    const diff = seconds - now / 1000;
    const format = new Intl.RelativeTimeFormat(this.deps.locale, { numeric: "auto" });
    const units: [Intl.RelativeTimeFormatUnit, number][] = [
      ["year", 365 * 86400],
      ["month", 30 * 86400],
      ["week", 7 * 86400],
      ["day", 86400],
      ["hour", 3600],
      ["minute", 60],
    ];
    for (const [unit, size] of units)
      if (Math.abs(diff) >= size) return format.format(Math.round(diff / size), unit);
    return format.format(Math.round(diff), "second");
  }
}

/**
 * 리비전(`null` = index)의 그 파일을 저장소 꼴의 줄로. 없으면 `missing` 이 참일 때 빈 파일(HEAD 에 없는 새 파일), 아니면 `undefined`.
 * 텍스트가 아니거나 UTF-8 이 아니면 `undefined` — 줄을 바이트로 견줄 수 없다.
 */
async function stored(
  source: GutterSource,
  rev: string | null,
  path: string,
  missing: boolean,
): Promise<FileLines | undefined> {
  let blob: GitBlob;
  try {
    blob = await source.blob({ rev, path });
  } catch (e) {
    const code = (e as { code?: unknown } | null)?.code;
    if (missing && code === ERROR_CODE.NOT_FOUND) return { lines: [], eol: true };
    throw e;
  }
  const j = blob.judgment;
  if (blob.content?.kind !== "text" || !j || j.binary || j.lossy || j.encoding !== "UTF-8")
    return undefined;
  return storedLines(blob.content.text);
}
