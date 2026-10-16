// 키 이벤트 → 액션(docs/design/10-ui.md §4.4). 창의 keydown · keyup 을 받아 바인딩을 고르고 레지스트리로 실행한다.
//
// - 매칭은 `KeyboardEvent.code` 로 한다(keys.ts 머리 주석).
// - IME 조합 중(`isComposing`, 또는 keyCode 229)인 키는 보지 않는다 — 한글을 조합하다 Enter 로 확정하는 키가 단축키로
//   실행되지 않게(§4.4 「IME 조합 중」). keyCode 229 를 함께 보는 이유: Safari(WebKit)는 조합을 끝낸 Enter 의 keydown 을
//   `isComposing: false`, keyCode 229 로 보낸다.
//   예외: `isComposing` 이 거짓이고 ⌘ 나 Ctrl 이 눌린 keydown 은 keyCode 229 여도 `code` 로 맞춘다. 입력기가 ⌘ 조합을
//   「입력기가 처리한 키」(229)로 보고하면 한글 입력 상태의 ⌘1 이 통째로 무시되기 때문이다 — ⌘ · Ctrl 조합은 글자를
//   조합하지 않는다. 실제 WebKit 이 그렇게 보고하는지는 사람이 한글 입력 상태에서 확인할 항목이다.
// - 누르고 있어 반복되는 keydown(`repeat`)은 액션이 반복을 허락할 때(`allowsRepeat`)만 실행한다. 툴 윈도우 토글처럼
//   반복되면 깜빡이는 액션이 기본이라, 허락하지 않으면 키만 가져가고 실행하지 않는다.
// - 연속 키: 첫 누름이 어떤 연속 키의 앞부분이면 그 키를 가져가고(`pending`) 다음 누름을 `chordTimeoutMs` 동안 기다린다.
//   같은 첫 누름이 한 번 누르기 바인딩이기도 하면 연속 키가 이긴다(VS Code 와 같다). 다음 누름이 이어지지 않으면 기다림을
//   버리고 그 누름을 새로 본다.
// - 더블 탭: 수정 키만 눌렀다 떼기를 두 번. 첫 뗌부터 두 번째 누름까지, 그리고 각 누름의 길이가 `doubleTapMs` 이내여야
//   한다. 두 번째 뗌에서 실행한다 — 두 번째 누름 채로 다른 키를 치면(Shift 로 대문자) 탭이 아니다.

import {
  type KeySequence,
  MODIFIER_CODES,
  type Modifier,
  type Stroke,
  strokeFromEvent,
  strokeKey,
} from "./keys";
import type { Binding } from "./keymap-file";
import { type ContextLayers, evaluateWhen, whenDepth } from "./when";

export interface KeyEventLike {
  readonly code: string;
  readonly key: string;
  readonly metaKey: boolean;
  readonly ctrlKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
  readonly isComposing?: boolean;
  readonly keyCode?: number;
  readonly repeat?: boolean;
}

export interface EngineOptions {
  /** 바인딩의 액션을 실행한다. 실행할 수 없으면(등록 안 됨 · 비활성) 거짓. */
  run(action: string): boolean;
  /** 이 창에서 지금 실행할 수 있는 액션인가. 아니면 그 바인딩은 없는 것으로 본다. */
  isEnabled(action: string): boolean;
  /** 누르고 있어 반복되는 keydown 으로도 실행하는가. 없으면 거짓. */
  allowsRepeat?(action: string): boolean;
  /** 지금의 컨텍스트 층(안쪽부터). */
  context(): ContextLayers;
  now?(): number;
  /** 연속 키의 다음 누름을 기다리는 시간. */
  chordTimeoutMs?: number;
  /** 더블 탭의 간격 · 누름 길이 한도. */
  doubleTapMs?: number;
}

/** 이벤트를 가져갔는가(호출자는 `preventDefault` 한다), 실행한 액션, 연속 키를 기다리는 중인가. */
export interface KeyOutcome {
  readonly handled: boolean;
  readonly action?: string;
  readonly pending?: boolean;
}

/** 기본값. 연속 키는 VS Code 처럼 무한정 기다리지 않는다 — 잊힌 첫 누름이 다음 단축키를 먹지 않게. */
export const DEFAULT_CHORD_TIMEOUT_MS = 1500;
/** 웹뷰에서는 OS 의 더블 클릭 간격을 읽을 수 없어 고정값을 둔다. */
export const DEFAULT_DOUBLE_TAP_MS = 400;

const NOT_HANDLED: KeyOutcome = { handled: false };

interface Tap {
  modifier: Modifier;
  downAt: number;
  upAt?: number;
  second: boolean;
}

export function isComposingEvent(e: KeyEventLike): boolean {
  if (e.isComposing === true) return true;
  return e.keyCode === 229 && !e.metaKey && !e.ctrlKey;
}

export class KeymapEngine {
  private bindings: readonly Binding[] = [];
  private pending?: { strokes: Stroke[]; at: number };
  private tap?: Tap;
  private readonly now: () => number;
  private readonly chordTimeoutMs: number;
  private readonly doubleTapMs: number;

  constructor(private readonly options: EngineOptions) {
    this.now = options.now ?? (() => performance.now());
    this.chordTimeoutMs = options.chordTimeoutMs ?? DEFAULT_CHORD_TIMEOUT_MS;
    this.doubleTapMs = options.doubleTapMs ?? DEFAULT_DOUBLE_TAP_MS;
  }

  /** 병합된 바인딩(keymap-file.ts `mergeKeymap`)을 바꾼다. 기다리던 연속 키는 버린다. */
  setBindings(bindings: readonly Binding[]) {
    this.bindings = bindings;
    this.pending = undefined;
    this.tap = undefined;
  }

  keydown(e: KeyEventLike): KeyOutcome {
    if (isComposingEvent(e)) {
      this.tap = undefined;
      return NOT_HANDLED;
    }
    const modifier = MODIFIER_CODES[e.code];
    if (modifier) {
      this.modifierDown(modifier, e);
      return NOT_HANDLED;
    }
    this.tap = undefined;
    const stroke = strokeFromEvent(e);
    if (!stroke) return NOT_HANDLED;

    const now = this.now();
    const repeat = e.repeat === true;
    if (this.pending && now - this.pending.at <= this.chordTimeoutMs) {
      const outcome = this.match([...this.pending.strokes, stroke], now, repeat);
      if (outcome.handled) return outcome;
    }
    this.pending = undefined;
    return this.match([stroke], now, repeat);
  }

  keyup(e: KeyEventLike): KeyOutcome {
    const modifier = MODIFIER_CODES[e.code];
    const tap = this.tap;
    if (!modifier || !tap || tap.modifier !== modifier || isComposingEvent(e)) return NOT_HANDLED;
    const now = this.now();
    if (now - tap.downAt > this.doubleTapMs) {
      this.tap = undefined;
      return NOT_HANDLED;
    }
    if (!tap.second) {
      tap.upAt = now;
      return NOT_HANDLED;
    }
    this.tap = undefined;
    const binding = this.pick(
      (b) => b.sequence.kind === "double" && b.sequence.modifier === modifier,
    );
    return binding ? this.run(binding) : NOT_HANDLED;
  }

  private modifierDown(modifier: Modifier, e: KeyEventLike) {
    if (e.repeat) return;
    // 다른 수정 키를 함께 누르고 있으면 탭이 아니다(⌘ 를 누른 채 Shift 두 번 등).
    const others = { cmd: e.metaKey, ctrl: e.ctrlKey, alt: e.altKey, shift: e.shiftKey };
    others[modifier] = false;
    if (Object.values(others).some(Boolean)) {
      this.tap = undefined;
      return;
    }
    const now = this.now();
    const tap = this.tap;
    if (
      tap &&
      tap.modifier === modifier &&
      !tap.second &&
      tap.upAt !== undefined &&
      now - tap.upAt <= this.doubleTapMs
    ) {
      this.tap = { modifier, downAt: now, second: true };
      return;
    }
    this.tap = { modifier, downAt: now, second: false };
  }

  private match(strokes: Stroke[], now: number, repeat = false): KeyOutcome {
    const keys = strokes.map(strokeKey);
    const startsWith = (seq: KeySequence) =>
      seq.kind === "strokes" &&
      seq.strokes.length >= keys.length &&
      keys.every((k, i) => strokeKey(seq.strokes[i]!) === k);
    const layers = this.options.context();
    const live = this.bindings.filter(
      (b) =>
        startsWith(b.sequence) &&
        this.options.isEnabled(b.action) &&
        (!b.whenExpr || evaluateWhen(b.whenExpr, layers)),
    );
    if (
      live.some((b) => b.sequence.kind === "strokes" && b.sequence.strokes.length > keys.length)
    ) {
      this.pending = { strokes, at: now };
      return { handled: true, pending: true };
    }
    this.pending = undefined;
    const binding = this.best(live, layers);
    if (!binding) return NOT_HANDLED;
    if (repeat && !this.options.allowsRepeat?.(binding.action)) return { handled: true };
    return this.run(binding);
  }

  private pick(test: (b: Binding) => boolean): Binding | undefined {
    const layers = this.options.context();
    return this.best(
      this.bindings.filter(
        (b) =>
          test(b) &&
          this.options.isEnabled(b.action) &&
          (!b.whenExpr || evaluateWhen(b.whenExpr, layers)),
      ),
      layers,
    );
  }

  /** 조건이 가장 안쪽 층을 보는 것(`whenDepth`), 같으면 병합 순서에서 뒤의 것. */
  private best(candidates: readonly Binding[], layers: ContextLayers): Binding | undefined {
    let best: Binding | undefined;
    let bestDepth = -1;
    for (const b of candidates) {
      const depth = whenDepth(b.whenExpr, layers);
      if (depth >= bestDepth) {
        best = b;
        bestDepth = depth;
      }
    }
    return best;
  }

  private run(binding: Binding): KeyOutcome {
    return this.options.run(binding.action)
      ? { handled: true, action: binding.action }
      : NOT_HANDLED;
  }
}
