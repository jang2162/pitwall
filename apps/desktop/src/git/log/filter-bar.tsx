// Git 로그의 필터 바(F5-2, docs/design/10-ui.md 「주요 툴 윈도우 상세」 의 Git 로그 그림 `⌕ 텍스트/해시 브랜치▾ 작성자▾ 날짜▾ 경로▾`).
// 값은 `LogQuery` 이고 RPC 로 옮기는 것은 log-model.ts 의 `logParams` 다. 글자를 치는 칸은 멈춘 뒤(`DEBOUNCE_MS`) 한 번만 다시
// 부른다. ▾ 는 그 칸 하나를 담은 작은 창이고 바깥을 누르거나 Esc 면 닫힌다. 브랜치 ▾ 는 브랜치 패널과 같은 값(고른 브랜치)을
// 보이고 고친다.

import type { GitRefs } from "@pitwall/rpc-client";
import { ChevronDown, Search, X } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";

import { pickRef } from "./branch-panel";
import { EMPTY_QUERY, isHashText, type LogQuery } from "./log-model";
import type { GitLogStrings } from "./strings";

export const DEBOUNCE_MS = 250;

const TOGGLE =
  "rounded-sm border border-transparent px-1 text-[11px] aria-pressed:border-ui-accent aria-pressed:text-ui-accent";
const DROP =
  "flex items-center gap-0.5 rounded-sm px-1.5 py-0.5 text-[12px] hover:bg-ui-hover aria-expanded:bg-ui-active";
const INPUT =
  "rounded-sm border border-ui-border bg-transparent px-1.5 py-0.5 text-[12px] outline-none focus:border-ui-accent";

function Dropdown({
  label,
  active,
  children,
}: {
  label: string;
  active: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  }, [open]);
  return (
    <div ref={ref} className="relative">
      <button
        type="button"
        aria-expanded={open}
        aria-haspopup="dialog"
        className={`${DROP} ${active ? "text-ui-accent" : ""}`}
        onClick={() => setOpen(!open)}
      >
        {label}
        <ChevronDown size={12} />
      </button>
      {open && (
        <div
          role="dialog"
          aria-label={label}
          className="absolute top-full left-0 z-20 mt-1 flex max-h-[320px] min-w-[220px] flex-col gap-1 overflow-auto rounded-md border border-ui-border bg-ui-panel p-2 shadow-lg"
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              setOpen(false);
            }
          }}
        >
          {children}
        </div>
      )}
    </div>
  );
}

/** 글자 칸: 치는 동안은 제 값을 들고, 멈추면 `onCommit`. 바깥 값이 바뀌면(지우기) 따라간다. */
function useDebounced(value: string, onCommit: (v: string) => void): [string, (v: string) => void] {
  const [draft, setDraft] = useState(value);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const commit = useRef(onCommit);
  commit.current = onCommit;
  useEffect(() => setDraft(value), [value]);
  useEffect(() => () => clearTimeout(timer.current), []);
  return [
    draft,
    (v) => {
      setDraft(v);
      clearTimeout(timer.current);
      timer.current = setTimeout(() => commit.current(v), DEBOUNCE_MS);
    },
  ];
}

export interface FilterBarProps {
  query: LogQuery;
  onChange(query: LogQuery): void;
  refs: GitRefs | undefined;
  strings: GitLogStrings["filter"];
}

export function FilterBar({ query, onChange, refs, strings }: FilterBarProps) {
  const latest = useRef(query);
  latest.current = query;
  const set = (patch: Partial<LogQuery>) => onChange({ ...latest.current, ...patch });
  const [text, setText] = useDebounced(query.text, (v) => set({ text: v }));
  const [author, setAuthor] = useDebounced(query.author, (v) => set({ author: v }));
  const [paths, setPaths] = useDebounced(query.paths, (v) => set({ paths: v }));
  const names = [
    ...(refs?.branches.map((b) => b.name) ?? []),
    ...(refs?.remoteBranches.map((b) => b.name) ?? []),
  ];
  const dirty =
    JSON.stringify({ ...query, refs: [...query.refs] }) !==
    JSON.stringify({ ...EMPTY_QUERY, refs: [] });
  return (
    <div
      role="toolbar"
      aria-label={strings.text}
      className="flex flex-wrap items-center gap-1 border-b border-ui-border px-2 py-1"
    >
      <label className="flex min-w-[160px] flex-1 items-center gap-1">
        <Search size={12} className="shrink-0 text-ui-muted-foreground" />
        <input
          aria-label={strings.text}
          placeholder={strings.text}
          value={text}
          spellCheck={false}
          onChange={(e) => setText(e.target.value)}
          className={`${INPUT} w-full`}
          title={isHashText(text) ? strings.hashHint : undefined}
        />
      </label>
      <button
        type="button"
        aria-pressed={query.regex}
        title={strings.regex}
        aria-label={strings.regex}
        className={TOGGLE}
        onClick={() => set({ regex: !query.regex })}
      >
        .*
      </button>
      <button
        type="button"
        aria-pressed={!query.ignoreCase}
        title={strings.matchCase}
        aria-label={strings.matchCase}
        className={TOGGLE}
        onClick={() => set({ ignoreCase: !query.ignoreCase })}
      >
        Aa
      </button>
      <Dropdown
        label={query.refs.length > 0 ? strings.branchCount(query.refs.length) : strings.branch}
        active={query.refs.length > 0}
      >
        {names.map((name) => (
          <label key={name} className="flex items-center gap-1 text-[12px] whitespace-nowrap">
            <input
              type="checkbox"
              checked={query.refs.includes(name)}
              onChange={() => set({ refs: pickRef(latest.current.refs, name, true) })}
            />
            {name}
          </label>
        ))}
      </Dropdown>
      <Dropdown label={strings.author} active={query.author.trim() !== ""}>
        <input
          aria-label={strings.author}
          autoFocus
          value={author}
          spellCheck={false}
          onChange={(e) => setAuthor(e.target.value)}
          className={INPUT}
        />
      </Dropdown>
      <Dropdown label={strings.date} active={query.since !== "" || query.until !== ""}>
        <label className="flex items-center justify-between gap-2 text-[12px]">
          {strings.since}
          <input
            type="date"
            aria-label={strings.since}
            value={query.since}
            onChange={(e) => set({ since: e.target.value })}
            className={INPUT}
          />
        </label>
        <label className="flex items-center justify-between gap-2 text-[12px]">
          {strings.until}
          <input
            type="date"
            aria-label={strings.until}
            value={query.until}
            onChange={(e) => set({ until: e.target.value })}
            className={INPUT}
          />
        </label>
      </Dropdown>
      <Dropdown label={strings.path} active={query.paths.trim() !== ""}>
        <input
          aria-label={strings.path}
          autoFocus
          value={paths}
          spellCheck={false}
          placeholder={strings.pathHint}
          onChange={(e) => setPaths(e.target.value)}
          className={`${INPUT} w-[260px]`}
        />
      </Dropdown>
      {dirty && (
        <button
          type="button"
          aria-label={strings.clear}
          title={strings.clear}
          className="rounded-sm p-0.5 hover:bg-ui-hover"
          onClick={() => onChange(EMPTY_QUERY)}
        >
          <X size={12} />
        </button>
      )}
    </div>
  );
}
