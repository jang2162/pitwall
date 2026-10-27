// 시험용: canvas 2D 기록기(happy-dom 에는 canvas 2D 가 없다)와 로그 행 고정물(pw-git 이 쓴 testdata/lanes.json 의 그래프에 가짜
// 해시 · 제목 · 작성자를 붙인 것).

import type { GraphRow, LogRef, LogRow } from "@pitwall/rpc-client";

import type { Ctx } from "./graph-render";
import fixture from "./testdata/lanes.json";

export interface Stroke {
  style: string;
  width: number;
  alpha: number;
  /** moveTo · lineTo 의 점. */
  points: [number, number][];
}

/** 그린 것을 적는 가짜 2D 문맥. 글자 폭은 글자 수 × 7 px. */
export class RecordingContext implements Ctx {
  strokeStyle: string | CanvasGradient | CanvasPattern = "";
  fillStyle: string | CanvasGradient | CanvasPattern = "";
  lineWidth = 1;
  globalAlpha = 1;
  font = "";
  textBaseline: CanvasTextBaseline = "alphabetic";
  textAlign: CanvasTextAlign = "start";
  texts: { text: string; x: number; y: number; style: string }[] = [];
  fills: { style: string; x: number; y: number; w: number; h: number }[] = [];
  strokes: Stroke[] = [];
  private path: [number, number][] = [];

  /** 다음 그림을 위해 비운다. */
  clear() {
    this.texts = [];
    this.fills = [];
    this.strokes = [];
  }
  beginPath() {
    this.path = [];
  }
  moveTo(x: number, y: number) {
    this.path.push([x, y]);
  }
  lineTo(x: number, y: number) {
    this.path.push([x, y]);
  }
  stroke() {
    this.strokes.push({
      style: String(this.strokeStyle),
      width: this.lineWidth,
      alpha: this.globalAlpha,
      points: this.path,
    });
  }
  fill() {}
  arc() {}
  rect() {}
  clip() {}
  save() {}
  restore() {}
  setTransform() {}
  clearRect() {}
  strokeRect() {}
  fillRect(x: number, y: number, w: number, h: number) {
    this.fills.push({ style: String(this.fillStyle), x, y, w, h });
  }
  fillText(text: string, x: number, y: number) {
    this.texts.push({ text, x, y, style: String(this.fillStyle) });
  }
  measureText(text: string) {
    return { width: text.length * 7 } as TextMetrics;
  }
}

/** 행 번호의 가짜 전체 해시. 앞 8자리(짧은 해시)가 행 번호라 행마다 다르다 — 앞을 16진 글자로 채우면 `a3` 과 `3` 이 같아진다. */
export const hashOf = (i: number) => i.toString(16).padStart(8, "0").padEnd(40, "a");

const graphs = fixture as unknown as Record<string, { graphs: GraphRow[] }>;

/** 고정물 `name` 의 행. 첫 행은 지금 브랜치 `main`, 셋째 행은 브랜치 `feat/x`, 다섯째 행은 원격 `origin/old` 와 태그 `v1`. */
export function fixtureRows(name: "synth" | "fanout"): LogRow[] {
  return graphs[name]!.graphs.map((graph, i) => {
    const refs: LogRef[] =
      i === 0
        ? [{ name: "main", kind: "branch", current: true }]
        : i === 2
          ? [{ name: "feat/x", kind: "branch", current: false }]
          : i === 4
            ? [
                { name: "origin/old", kind: "remote", current: false },
                { name: "v1", kind: "tag", current: false },
              ]
            : [];
    return {
      hash: hashOf(i),
      parents: graph.down.map((_, k) => hashOf(i + 1 + k)),
      subject: `subject ${i}`,
      authorName: `author ${i % 3}`,
      authorEmail: `a${i % 3}@example.com`,
      authorTime: 1_700_000_000 - i * 60,
      refs,
      graph,
    };
  });
}
