// diff 뷰 컴포넌트(docs/design/13-m2-plan.md M2-15 · §2 「공유 부품의 주인」): 파일 하나의 diff 를 `@codemirror/merge` 로 그린다.
// 커밋 상세 · 두 커밋 비교(이 행), 커밋 툴 윈도우의 미리보기(M2-16) · 파일 히스토리(M2-18) · 머지 도구(M2-19)가 이것을 쓴다.
//
// - 변경은 Rust 가 계산한 hunk 다(`git.diff`, 결정 M2-D9). merge 에는 `diffConfig.override` 로 묶음마다 변경 하나를 넘기고, 칠하는
//   것은 hunk 의 줄 번호다(diff-model.ts · diff-decorations.ts — merge 의 덩어리는 git 의 줄보다 넓을 수 있다).
// - 나란히 보기(`split`)는 `MergeView`(두 뷰가 한 스크롤 안), 통합 보기(`unified`)는 새 쪽 문서 하나에 뺀 줄 블록을 끼운 뷰다 —
//   통합 보기도 `unifiedMergeView` 를 쓰지만 그 뺀 덩어리는 숨기고(diffTheme) 모델의 블록을 그린다. 바뀌지 않은 긴 구간은 접는다
//   (앞뒤 3줄을 남기고 4줄 이상이면 — merge 의 `collapseUnchanged` 기본값, IntelliJ 와 같다).
// - 보기를 바꾸면 뷰를 새로 만들고, 화면 맨 위의 줄(새 쪽 줄 번호로 바꿔 든다)이 다시 맨 위에 오게 굴린다.
// - `F7` / `⇧F7`(액션 `diff.nextChange` · `diff.previousChange`, 키는 `when = "diffFocus"`)은 포커스가 든 diff 뷰의 커서 다음 · 앞
//   묶음으로 간다. 포커스를 받은 뷰가 `activeDiff` 에 자기를 건다.
// - 확장 지점(`slots.hunkControls`): 변경 묶음 옆 칸(M2-16 의 hunk · 줄 스테이징 체크 · 되돌리기)은 이 뷰가 그리지 않는다. 쪽마다
//   CodeMirror 확장을 받아 그 뷰에 끼운다 — 모델(묶음 · 줄)과 두 문서를 받으므로 거터 · 위젯을 묶음 자리에 달 수 있다.
// - 두 쪽 모두 읽기 전용이다(편집하는 diff 는 M2-17 의 인라인 팝업 · M2-19 의 머지 문서).

import { type Extension, EditorState, Text } from "@codemirror/state";
import { MergeView, unifiedMergeView } from "@codemirror/merge";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightSpecialChars,
  lineNumbers,
} from "@codemirror/view";
import type { DiffFile } from "@pitwall/rpc-client";
import { useEffect, useRef } from "react";
import { create } from "zustand";

import { editorTheme } from "../../editor/extensions/theme";
import {
  buildDiffModel,
  diffOverride,
  type DiffModel,
  lineStart,
  nextGroup,
  normalizeText,
  oldToNew,
} from "./diff-model";
import {
  charHighlights,
  deletedBlocks,
  diffTheme,
  lineDecorations,
  oldLineNumbers,
} from "./diff-decorations";
import { revisionHighlight, revisionSyntax, type StaticSyntaxSource } from "./revision-syntax";

export type DiffMode = "split" | "unified";

/** 바뀌지 않은 구간 접기(merge 의 기본값과 같다 — 앞뒤 3줄을 남기고, 4줄 이상일 때). */
export const COLLAPSE = { margin: 3, minSize: 4 } as const;

/** 그릴 diff: 파일 하나(hunk)와 두 쪽의 전체 텍스트. 텍스트는 읽은 그대로(CRLF 포함)여도 된다. */
export interface DiffContent {
  readonly file: DiffFile;
  readonly oldText: string;
  readonly newText: string;
  /** Linguist 언어 이름(`lang.detect`). 없으면 강조하지 않는다. */
  readonly language: string | null;
}

/** 확장 지점이 받는 것. */
export interface DiffSlotContext {
  readonly model: DiffModel;
  readonly file: DiffFile;
  /** 이 확장을 끼울 뷰: 나란히 보기의 옛 쪽 · 새 쪽, 또는 통합 보기(새 쪽 문서). */
  readonly side: "a" | "b" | "unified";
  readonly a: Text;
  readonly b: Text;
}

export interface DiffViewSlots {
  /** 변경 묶음 옆 칸(M2-16 의 스테이징 · 되돌리기). 쪽마다 확장을 돌려준다. */
  hunkControls?(context: DiffSlotContext): Extension;
}

/** 바깥(액션 · 시험 · 측정)이 쓰는 손잡이. */
export interface DiffController {
  readonly mode: DiffMode;
  readonly model: DiffModel;
  /** 지금 보기의 뷰들. 나란히 보기는 `[a, b]`, 통합 보기는 `[뷰]`. */
  views(): readonly EditorView[];
  /** 다음(1) · 앞(-1) 변경 묶음으로 커서를 옮기고 굴린다. 옮겼으면 참. */
  go(dir: 1 | -1): boolean;
  /** 화면 맨 위의 줄(새 쪽 줄 번호). */
  topLine(): number;
  /** 새 쪽 줄 `line` 이 화면 맨 위에 오게 굴린다. */
  scrollToLine(line: number): void;
}

/** 포커스를 마지막으로 받은 diff 뷰(`F7` 액션이 쓴다). 뷰가 사라지면 비운다. */
export const activeDiff = create<{ current: DiffController | undefined }>(() => ({
  current: undefined,
}));

interface Built {
  controller: DiffController;
  destroy(): void;
}

const readOnly = [EditorState.readOnly.of(true), EditorState.lineSeparator.of("\n")];

function common(phrases: Record<string, string>): Extension[] {
  return [
    lineNumbers(),
    highlightSpecialChars(),
    highlightActiveLine(),
    drawSelection(),
    editorTheme,
    diffTheme,
    readOnly,
    EditorState.phrases.of(phrases),
  ];
}

interface BuildOptions {
  content: DiffContent;
  mode: DiffMode;
  parent: HTMLElement;
  syntax?: StaticSyntaxSource;
  slots?: DiffViewSlots;
  phrases: Record<string, string>;
}

/** 뷰를 만든다(React 밖 — 측정 하네스도 이것을 부른다). */
export function buildDiffView({
  content,
  mode,
  parent,
  syntax,
  slots,
  phrases,
}: BuildOptions): Built {
  const model = buildDiffModel(content.file);
  const aText = normalizeText(content.oldText);
  const bText = normalizeText(content.newText);
  const a = Text.of(aText.split("\n"));
  const b = Text.of(bText.split("\n"));
  const override = diffOverride(model, a, b);
  const synA = syntax && revisionSyntax(syntax, content.language, aText);
  const synB = syntax && revisionSyntax(syntax, content.language, bText);
  const slot = (side: DiffSlotContext["side"]) =>
    slots?.hunkControls?.({ model, file: content.file, side, a, b }) ?? [];

  let views: EditorView[];
  let scroller: HTMLElement;
  let destroy: () => void;
  if (mode === "split") {
    const merge = new MergeView({
      a: {
        doc: a,
        extensions: [
          common(phrases),
          revisionHighlight(synA),
          EditorView.decorations.of(lineDecorations(model, "a", a)),
          charHighlights(model, "a", a, b),
          slot("a"),
        ],
      },
      b: {
        doc: b,
        extensions: [
          common(phrases),
          revisionHighlight(synB),
          EditorView.decorations.of(lineDecorations(model, "b", b)),
          charHighlights(model, "b", a, b),
          slot("b"),
        ],
      },
      parent,
      highlightChanges: false,
      gutter: false,
      collapseUnchanged: COLLAPSE,
      diffConfig: { override },
    });
    merge.dom.classList.add("pw-diff-split");
    // merge 의 바깥 상자가 스크롤 영역이다(두 뷰는 높이가 내용만큼) — 탭 높이를 채운다.
    merge.dom.style.height = "100%";
    views = [merge.a, merge.b];
    scroller = merge.dom;
    destroy = () => merge.destroy();
  } else {
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: b,
        extensions: [
          oldLineNumbers(model),
          common(phrases),
          revisionHighlight(synB),
          unifiedMergeView({
            original: a,
            highlightChanges: false,
            gutter: false,
            mergeControls: false,
            syntaxHighlightDeletions: false,
            collapseUnchanged: COLLAPSE,
            diffConfig: { override },
          }),
          EditorView.decorations.of(lineDecorations(model, "unified", b)),
          EditorView.decorations.of(
            deletedBlocks(model, a, b, synA && ((f, t) => synA.highlights(f, t))),
          ),
          charHighlights(model, "unified", a, b),
          slot("unified"),
        ],
      }),
    });
    views = [view];
    scroller = view.scrollDOM;
    destroy = () => view.destroy();
  }
  // 문서 맨 위가 스크롤 영역의 어디서 시작하는가(굴리기 전에 한 번 잰다 — 굴린 뒤의 값은 스크롤에 따라 움직인다).
  const main = views[views.length - 1]!;
  const docOffset = main.documentTop - scroller.getBoundingClientRect().top + scroller.scrollTop;
  const releaseA = synA?.retain();

  const cursorLine = (): number | undefined => {
    const focused = views.find((v) => v.hasFocus);
    if (!focused) return undefined;
    const n = focused.state.doc.lineAt(focused.state.selection.main.head).number;
    return mode === "split" && focused === views[0] ? oldToNew(model, n) : n;
  };

  const controller: DiffController = {
    mode,
    model,
    views: () => views,
    go(dir) {
      const from = cursorLine() ?? (dir === 1 ? controller.topLine() - 1 : controller.topLine());
      const i = nextGroup(model, from, dir);
      if (i === undefined) return false;
      const g = model.groups[i]!;
      const focusedA = mode === "split" && views[0]!.hasFocus && g.oldFrom < g.oldTo;
      const view = focusedA ? views[0]! : main;
      const pos = focusedA ? lineStart(a, g.oldFrom) : lineStart(b, g.newFrom);
      view.dispatch({
        selection: { anchor: pos },
        effects: EditorView.scrollIntoView(pos, { y: "center" }),
      });
      view.focus();
      return true;
    },
    topLine() {
      // 1px 아래를 본다 — 맨 위 줄의 위 경계가 스크롤 값의 반올림으로 앞 줄 끝에 걸리지 않게.
      const block = main.lineBlockAtHeight(Math.max(0, scroller.scrollTop - docOffset) + 1);
      return main.state.doc.lineAt(block.from).number;
    },
    scrollToLine(line) {
      const doc = main.state.doc;
      const block = main.lineBlockAt(lineStart(doc, Math.min(Math.max(1, line), doc.lines)));
      scroller.scrollTop = Math.ceil(block.top + docOffset);
    },
  };

  const onFocus = () => activeDiff.setState({ current: controller });
  parent.addEventListener("focusin", onFocus);
  return {
    controller,
    destroy() {
      parent.removeEventListener("focusin", onFocus);
      if (activeDiff.getState().current === controller) activeDiff.setState({ current: undefined });
      releaseA?.();
      destroy();
    },
  };
}

export interface DiffViewProps {
  content: DiffContent;
  mode: DiffMode;
  syntax?: StaticSyntaxSource;
  slots?: DiffViewSlots;
  /** merge 의 「바뀌지 않은 줄 $개」 같은 문구(`EditorState.phrases`). */
  phrases: Record<string, string>;
  /** 뷰를 만들 때마다(보기를 바꿨을 때 포함) 손잡이를 받는다. */
  onController?(controller: DiffController | undefined): void;
}

/**
 * 파일 하나의 diff. `mode` 가 바뀌면 뷰를 다시 만들고 화면 맨 위 줄을 잇는다. `content` 가 바뀌면(다른 파일 · 공백 무시) 처음부터
 * 그린다.
 */
export function DiffView({ content, mode, syntax, slots, phrases, onController }: DiffViewProps) {
  const host = useRef<HTMLDivElement>(null);
  /** 보기를 바꾸기 직전의 맨 위 줄(새 쪽). 같은 내용일 때만 잇는다. */
  const carried = useRef<{ content: DiffContent; line: number } | undefined>(undefined);
  const onControllerRef = useRef(onController);
  onControllerRef.current = onController;

  useEffect(() => {
    const parent = host.current;
    if (!parent) return;
    const built = buildDiffView({ content, mode, parent, syntax, slots, phrases });
    const keep = carried.current;
    if (keep?.content === content) {
      built.controller.scrollToLine(keep.line);
      // 높이를 잰 뒤에 한 번 더 — 처음 그린 높이는 짐작값이라 맞지 않을 수 있다.
      built.controller.views()[0]!.requestMeasure({
        read: () => null,
        write: () => built.controller.scrollToLine(keep.line),
      });
    }
    carried.current = undefined;
    onControllerRef.current?.(built.controller);
    return () => {
      carried.current = { content, line: built.controller.topLine() };
      onControllerRef.current?.(undefined);
      built.destroy();
    };
  }, [content, mode, syntax, slots, phrases]);

  // 키맵 컨텍스트(src/keymap/context.ts): `F7` / `⇧F7` 바인딩이 `when = "diffFocus"` 로 쓴다.
  return (
    <div
      ref={host}
      data-keymap-context="diffFocus"
      data-diff-mode={mode}
      className="pw-diff-host h-full min-h-0 overflow-hidden"
    />
  );
}
