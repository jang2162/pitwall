// 키맵 파일(docs/design/10-ui.md §4.4 「정의 형식」, §4.5 `keymap.toml`)을 읽고 병합한다. 병합은 이 파일 한 곳에서만 한다 —
// 네이티브 메뉴의 accelerator 도 이 결과에서 만든다(`menuAccelerators`).
//
// 파일 형식(기본 키맵과 사용자 덮어쓰기가 같다):
//   schema = 1                 # 없으면 설정 계층이 처음 읽을 때 맨 위에 넣어 파일을 다시 쓴다(03 §5)
//
//   [[bindings]]
//   action = "action.find"
//   key = "cmd+shift+a"        # 모든 OS. 아래 OS별 키가 있으면 그 OS 에서는 그것이 이긴다
//   mac = "cmd+shift+a"        # OS별 표기(§4.1). win · linux 도 같다
//   when = "editorFocus"       # 선택
//   mode = "add"               # 사용자 파일에서만. 없으면 "replace"
//
//   [[unbind]]                 # 사용자 파일에서만. action · key 중 하나 이상
//   action = "vcs.popup"
//   key = "ctrl+v"
//
// 그 OS 의 키가 없는 바인딩은 그 OS 에서 없는 것으로 본다. 모르는 필드는 오류다 — 오타(`acton`)가 조용히 무시되지 않게.
// 그 OS 키보드에 없는 키(macOS 의 Insert)는 오류가 아니라 경고다(`warnings`) — 바인딩은 남지만 누를 수 없다.

import { parse as parseToml } from "smol-toml";

import {
  type KeySequence,
  missingKey,
  parseKeySequence,
  type Platform,
  sameSequence,
  strokeKey,
  toAccelerator,
} from "./keys";
import { parseWhen, type WhenExpr } from "./when";

export type BindingSource = "default" | "user";

export interface Binding {
  readonly action: string;
  readonly sequence: KeySequence;
  /** 파일에 적힌 키 표기(오류 · 키맵 화면용). */
  readonly key: string;
  readonly when?: string;
  readonly whenExpr?: WhenExpr;
  readonly source: BindingSource;
}

export interface UserBinding extends Binding {
  readonly mode: "replace" | "add";
}

export interface Unbind {
  readonly action?: string;
  readonly sequence?: KeySequence;
  readonly when?: string;
}

export interface UserKeymap {
  readonly bindings: readonly UserBinding[];
  readonly unbind: readonly Unbind[];
  /** 막지는 않지만 알릴 것(그 OS 에 없는 키 등). */
  readonly warnings: readonly string[];
}

export class KeymapFileError extends Error {
  readonly problems: readonly string[];
  constructor(problems: readonly string[]) {
    super(problems.join("\n"));
    this.name = "KeymapFileError";
    this.problems = problems;
  }
}

/** 이 파일 형식의 스키마 버전(`docs/design/03-architecture.md` 「모든 설정 파일 첫 줄에 `schema = <정수>`」). */
export const KEYMAP_SCHEMA = 1;

const BINDING_FIELDS = new Set(["action", "key", "mac", "win", "linux", "when", "mode"]);
const UNBIND_FIELDS = new Set(["action", "key", "mac", "win", "linux", "when"]);

type Table = Record<string, unknown>;

function isTable(v: unknown): v is Table {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

class Reader {
  readonly problems: string[] = [];
  readonly warnings: string[] = [];
  constructor(readonly platform: Platform) {}

  string(t: Table, field: string, where: string): string | undefined {
    const v = t[field];
    if (v === undefined) return undefined;
    if (typeof v !== "string") {
      this.problems.push(`${where}.${field}: 문자열이어야 한다`);
      return undefined;
    }
    return v;
  }

  fields(t: Table, allowed: Set<string>, where: string) {
    for (const k of Object.keys(t)) {
      if (!allowed.has(k)) this.problems.push(`${where}: 모르는 필드 ${k}`);
    }
  }

  /** 이 OS 의 키 표기. OS별 키가 하나라도 있는데 이 OS 것이 없으면 `key` 로 떨어진다. */
  keyText(t: Table, where: string): string | undefined {
    return this.string(t, this.platform, where) ?? this.string(t, "key", where);
  }

  sequence(text: string, where: string): KeySequence | undefined {
    try {
      return parseKeySequence(text);
    } catch (e) {
      this.problems.push(`${where}: ${(e as Error).message}`);
      return undefined;
    }
  }

  when(t: Table, where: string): { when?: string; whenExpr?: WhenExpr } {
    const when = this.string(t, "when", where);
    if (when === undefined) return {};
    try {
      return { when, whenExpr: parseWhen(when) };
    } catch (e) {
      this.problems.push(`${where}.when: ${(e as Error).message}`);
      return {};
    }
  }

  tables(root: Table, field: string): [Table, string][] {
    const v = root[field];
    if (v === undefined) return [];
    if (!Array.isArray(v)) {
      this.problems.push(`${field}: [[${field}]] 표의 배열이어야 한다`);
      return [];
    }
    return v.flatMap((item, i): [Table, string][] => {
      const where = `${field}[${i}]`;
      if (!isTable(item)) {
        this.problems.push(`${where}: 표여야 한다`);
        return [];
      }
      return [[item, where]];
    });
  }

  binding(t: Table, where: string, source: BindingSource): Binding | undefined {
    this.fields(t, BINDING_FIELDS, where);
    const action = this.string(t, "action", where);
    if (!action) this.problems.push(`${where}.action: 없다`);
    const key = this.keyText(t, where);
    const hasAnyKey = ["key", "mac", "win", "linux"].some((f) => t[f] !== undefined);
    if (!hasAnyKey) this.problems.push(`${where}: key(또는 mac · win · linux)가 없다`);
    const when = this.when(t, where);
    // 다른 OS 의 표기도 검사한다 — 그 OS 에서 처음 열었을 때 깨진 것을 알게 되면 늦다.
    for (const f of ["key", "mac", "win", "linux"]) {
      const text = this.string(t, f, where);
      if (text !== undefined && text !== key) this.sequence(text, `${where}.${f}`);
    }
    if (!action || key === undefined) return undefined;
    const field = `${where}.${t[this.platform] !== undefined ? this.platform : "key"}`;
    const sequence = this.sequence(key, field);
    if (!sequence) return undefined;
    const missing = missingKey(sequence, this.platform);
    if (missing) this.warnings.push(`${field}: "${key}" 의 ${missing} 는 이 OS 키보드에 없다`);
    return { action, sequence, key, ...when, source };
  }
}

/** TOML 원문을 객체로. 문법 오류는 `KeymapFileError`. */
export function parseKeymapToml(text: string): Table {
  try {
    return parseToml(text) as Table;
  } catch (e) {
    throw new KeymapFileError([(e as Error).message]);
  }
}

function checkRoot(r: Reader, root: unknown, allowed: Set<string>): root is Table {
  if (!isTable(root)) {
    r.problems.push("키맵 파일의 최상위가 표가 아니다");
    return false;
  }
  for (const k of Object.keys(root)) {
    if (!allowed.has(k)) r.problems.push(`모르는 최상위 키 ${k}`);
  }
  if (root.schema !== undefined && root.schema !== KEYMAP_SCHEMA) {
    r.problems.push(`schema = ${String(root.schema)} 는 모르는 버전이다(이 앱은 ${KEYMAP_SCHEMA})`);
  }
  return true;
}

/**
 * 기본 키맵(앱 내장, `extensions/keymap-default/keymaps/default.toml`)을 읽는다. 이 OS 의 키가 없는 바인딩은 뺀다.
 * 틀리면 `KeymapFileError` — 앱에 묶여 나가는 파일이므로 시험이 잡는다.
 */
export function readDefaultKeymap(root: unknown, platform: Platform): Binding[] {
  return readDefaultKeymapWithWarnings(root, platform).bindings;
}

/** `readDefaultKeymap` 과 같고 경고도 돌려준다(기본 키맵에 경고가 없는지 시험이 본다). */
export function readDefaultKeymapWithWarnings(
  root: unknown,
  platform: Platform,
): { bindings: Binding[]; warnings: string[] } {
  const r = new Reader(platform);
  const out: Binding[] = [];
  if (checkRoot(r, root, new Set(["schema", "bindings"]))) {
    for (const [t, where] of r.tables(root, "bindings")) {
      if (t.mode !== undefined) r.problems.push(`${where}.mode: 기본 키맵에는 쓰지 않는다`);
      const b = r.binding(t, where, "default");
      if (b) out.push(b);
    }
  }
  if (r.problems.length > 0) throw new KeymapFileError(r.problems);
  return { bindings: out, warnings: r.warnings };
}

/**
 * 사용자 덮어쓰기(`$CONFIG/keymap.toml`, 설정 계층이 JSON 으로 바꾼 값)를 읽는다. `undefined` 면 파일이 없는 것이다.
 * 하나라도 틀리면 `KeymapFileError` 로 전부 버린다 — 일부만 반영된 키맵을 만들지 않는다(설정 계층과 같은 규칙,
 * crates/pw-core/src/config/source.rs 의 `ConfigSource::load`).
 */
export function readUserKeymap(root: unknown, platform: Platform): UserKeymap {
  if (root === undefined) return { bindings: [], unbind: [], warnings: [] };
  const r = new Reader(platform);
  const bindings: UserBinding[] = [];
  const unbind: Unbind[] = [];
  if (checkRoot(r, root, new Set(["schema", "bindings", "unbind"]))) {
    for (const [t, where] of r.tables(root, "bindings")) {
      const mode = r.string(t, "mode", where) ?? "replace";
      if (mode !== "replace" && mode !== "add") {
        r.problems.push(`${where}.mode: "replace" 나 "add" 여야 한다`);
      }
      const b = r.binding(t, where, "user");
      if (b) bindings.push({ ...b, mode: mode as "add" });
    }
    for (const [t, where] of r.tables(root, "unbind")) {
      r.fields(t, UNBIND_FIELDS, where);
      const action = r.string(t, "action", where);
      const keyText = r.keyText(t, where);
      const hasAnyKey = ["key", "mac", "win", "linux"].some((f) => t[f] !== undefined);
      if (!action && !hasAnyKey) r.problems.push(`${where}: action 이나 key 가 있어야 한다`);
      const sequence = keyText === undefined ? undefined : r.sequence(keyText, where);
      const when = r.when(t, where).when;
      // OS별 키만 있고 이 OS 것이 없으면 이 OS 에서는 해당 없음이다 — 액션 전체를 지우지 않는다.
      if (hasAnyKey && keyText === undefined) continue;
      unbind.push({ action, sequence, when });
    }
  }
  if (r.problems.length > 0) throw new KeymapFileError(r.problems);
  return { bindings, unbind, warnings: r.warnings };
}

function unbinds(u: Unbind, b: Binding): boolean {
  return (
    (u.action === undefined || u.action === b.action) &&
    (u.sequence === undefined || sameSequence(u.sequence, b.sequence)) &&
    (u.when === undefined || u.when === b.when)
  );
}

/**
 * 기본 위에 사용자 덮어쓰기를 얹는다(§4.5).
 * 1. `[[unbind]]` 가 가리키는 기본 바인딩을 지운다.
 * 2. `mode = "replace"`(기본값) 바인딩이 있는 액션은 그 액션의 기본 바인딩을 모두 지운다 — 사용자 파일 안의 같은 액션
 *    바인딩끼리는 지우지 않는다(한 액션에 키를 여럿 줄 수 있다).
 * 3. 사용자 바인딩을 파일 순서대로 뒤에 붙인다.
 * 결과의 순서가 의미를 갖는다: 같은 키 · 같은 조건 깊이면 **뒤의 것이 이긴다**(엔진 `pick`). 그래서 사용자가 기본 키를
 * 다른 액션에 주면 unbind 없이도 사용자 것이 이긴다.
 */
export function mergeKeymap(base: readonly Binding[], user: UserKeymap): Binding[] {
  const replaced = new Set(user.bindings.filter((b) => b.mode === "replace").map((b) => b.action));
  return [
    ...base.filter((b) => !replaced.has(b.action) && !user.unbind.some((u) => unbinds(u, b))),
    ...user.bindings.map(({ mode: _mode, ...b }) => b),
  ];
}

/**
 * 늘 다른 바인딩에 가려져 실행될 수 없는 바인딩을 뺀다. 메뉴 accelerator · 액션 찾기 라벨은 이것을 쓴다 — 가려진 키를
 * 보이면 거짓 표시다(사용자가 ⇧⌘A 를 설정에 주면 액션 찾기 옆의 ⇧⌘A 는 동작하지 않는다).
 * 가려졌다고 보는 것은 조건 없는 바인딩뿐이다(조건이 있으면 그 조건이 참일 때 이긴다, engine.ts `best`):
 * - 뒤에 같은 키의 조건 없는 다른 액션 바인딩이 있다(같은 깊이에서는 뒤가 이긴다).
 * - 한 번 누르기인데 조건 없는 연속 키가 그 누름으로 시작한다(연속 키가 이긴다).
 */
export function reachableBindings(bindings: readonly Binding[]): Binding[] {
  return bindings.filter((b, i) => {
    if (b.when) return true;
    const later = bindings.slice(i + 1);
    if (
      later.some((o) => !o.when && o.action !== b.action && sameSequence(o.sequence, b.sequence))
    ) {
      return false;
    }
    if (b.sequence.kind === "strokes" && b.sequence.strokes.length === 1) {
      const first = strokeKey(b.sequence.strokes[0]!);
      const prefixOf = bindings.some(
        (o) =>
          !o.when &&
          o.sequence.kind === "strokes" &&
          o.sequence.strokes.length > 1 &&
          strokeKey(o.sequence.strokes[0]!) === first,
      );
      if (prefixOf) return false;
    }
    return true;
  });
}

/** 액션마다 키 목록(병합 순서). 액션 찾기 · 툴팁에 보인다. 첫 항목이 대표 키다. */
export function keysByAction(bindings: readonly Binding[]): Map<string, Binding[]> {
  const out = new Map<string, Binding[]>();
  for (const b of bindings) {
    const list = out.get(b.action) ?? [];
    list.push(b);
    out.set(b.action, list);
  }
  return out;
}

/**
 * 네이티브 메뉴에 둘 accelerator. 메뉴 항목이 있는 액션만 받는다(`menuActions`). 액션마다 가려지지 않은
 * (`reachableBindings`) 조건 없는 첫 한 번 누르기 바인딩을 쓰고, 없으면 `null`(메뉴에 단축키를 표시하지 않는다) —
 * 연속 키 · 더블 탭 · 그 OS 에 없는 키는 메뉴가 표현하지 못한다.
 */
export function menuAccelerators(
  bindings: readonly Binding[],
  menuActions: readonly string[],
  platform: Platform,
): Record<string, string | null> {
  const reachable = reachableBindings(bindings);
  const out: Record<string, string | null> = {};
  for (const action of menuActions) {
    const b = reachable.find(
      (x) => x.action === action && !x.when && toAccelerator(x.sequence, platform),
    );
    out[action] = b ? toAccelerator(b.sequence, platform)! : null;
  }
  return out;
}
