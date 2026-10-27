// Git 로그의 왼쪽 브랜치 패널(docs/design/13-m2-plan.md M2-14, 10-ui.md 「Git」 마우스 표의 「브랜치 패널」 행): HEAD · 로컬 · 원격 ·
// 태그(`git.refs`). 고른 값은 ref 전체 이름(`refs/heads/…`, HEAD 는 `HEAD`)이다 — 짧은 이름이면 같은 이름의 태그가 브랜치보다 먼저
// 풀린다. 클릭 = 그 브랜치로 거르기(다시 누르면 풀기), `⌘+클릭`(Win/Linux `Ctrl`) = 여럿, 더블클릭 = 체크아웃(로컬은 그
// 브랜치, 원격은 추적 브랜치를 만들어). 원격 브랜치가 수천 개일 수 있어 행은 TanStack Virtual 로 보이는 것만 그린다.

import type { GitRefs } from "@pitwall/rpc-client";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronDown, ChevronRight, GitBranch, Star, Tag } from "lucide-react";
import { type MouseEvent, useMemo, useRef, useState } from "react";

import { useKeymap } from "../../keymap/keymap-provider";
import type { GitLogStrings } from "./strings";

const ROW = 22;

type Section = "local" | "remote" | "tags";

export type PanelRow =
  | { kind: "all" }
  | { kind: "head"; name: string }
  | { kind: "section"; section: Section; count: number; open: boolean }
  | { kind: "ref"; section: Section; name: string; refName: string; current: boolean };

/** 패널의 행(접힌 무리의 항목은 빠진다). */
export function panelRows(refs: GitRefs | undefined, closed: ReadonlySet<Section>): PanelRow[] {
  if (!refs?.workTree) return [];
  const rows: PanelRow[] = [{ kind: "all" }];
  if (refs.head && refs.head.kind !== "unborn") rows.push({ kind: "head", name: "HEAD" });
  const groups: [Section, { name: string; refName: string; current: boolean }[]][] = [
    ["local", refs.branches.map((b) => ({ name: b.name, refName: b.refName, current: b.head }))],
    [
      "remote",
      refs.remoteBranches.map((b) => ({ name: b.name, refName: b.refName, current: false })),
    ],
    ["tags", refs.tags.map((t) => ({ name: t.name, refName: t.refName, current: false }))],
  ];
  for (const [section, items] of groups) {
    if (items.length === 0) continue;
    const open = !closed.has(section);
    rows.push({ kind: "section", section, count: items.length, open });
    if (open) for (const item of items) rows.push({ kind: "ref", section, ...item });
  }
  return rows;
}

/** 행이 고르는 값(ref 전체 이름, HEAD 는 `HEAD`). */
function refValue(row: Extract<PanelRow, { kind: "head" | "ref" }>): string {
  return row.kind === "head" ? "HEAD" : row.refName;
}

/** 고른 브랜치에 `name` 을 넣거나 뺀다. `toggle` 이 아니면 그것 하나만(이미 그것 하나면 풀기). */
export function pickRef(chosen: readonly string[], name: string, toggle: boolean): string[] {
  if (toggle) return chosen.includes(name) ? chosen.filter((n) => n !== name) : [...chosen, name];
  return chosen.length === 1 && chosen[0] === name ? [] : [name];
}

export interface BranchPanelProps {
  refs: GitRefs | undefined;
  chosen: readonly string[];
  onChoose(refs: string[]): void;
  onCheckout(section: "local" | "remote", name: string): void;
  strings: GitLogStrings["branches"];
}

export function BranchPanel({ refs, chosen, onChoose, onCheckout, strings }: BranchPanelProps) {
  const mac = useKeymap().platform === "mac";
  const [closed, setClosed] = useState<ReadonlySet<Section>>(new Set(["tags"]));
  const rows = useMemo(() => panelRows(refs, closed), [refs, closed]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW,
    overscan: 10,
    initialRect: { width: 200, height: 600 },
  });
  const title: Record<Section, string> = {
    local: strings.local,
    remote: strings.remote,
    tags: strings.tags,
  };
  const click = (e: MouseEvent, row: PanelRow) => {
    if (row.kind === "all") return onChoose([]);
    if (row.kind === "section") {
      const next = new Set(closed);
      if (next.has(row.section)) next.delete(row.section);
      else next.add(row.section);
      return setClosed(next);
    }
    onChoose(pickRef(chosen, refValue(row), mac ? e.metaKey : e.ctrlKey));
  };
  return (
    <div className="flex h-full min-w-0 flex-col">
      <div className="px-2 py-1 text-[11px] font-semibold text-ui-muted-foreground uppercase">
        {strings.title}
      </div>
      <div
        ref={scrollRef}
        role="tree"
        aria-label={strings.title}
        aria-multiselectable
        className="min-h-0 flex-1 overflow-auto"
      >
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((item) => {
            const row = rows[item.index]!;
            const selected =
              row.kind === "all"
                ? chosen.length === 0
                : row.kind !== "section" && chosen.includes(refValue(row));
            const label =
              row.kind === "all"
                ? strings.all
                : row.kind === "head"
                  ? strings.head
                  : row.kind === "section"
                    ? `${title[row.section]} (${row.count})`
                    : row.name;
            return (
              <div
                key={item.key}
                role="treeitem"
                aria-selected={row.kind === "section" ? undefined : selected}
                aria-expanded={row.kind === "section" ? row.open : undefined}
                data-ref={row.kind === "ref" || row.kind === "head" ? refValue(row) : undefined}
                title={row.kind === "ref" ? row.name : undefined}
                className={`absolute left-0 flex w-full cursor-default items-center gap-1 truncate pr-2 text-[12px] hover:bg-ui-hover ${selected ? "bg-ui-active" : ""}`}
                style={{
                  top: item.start,
                  height: ROW,
                  paddingLeft: row.kind === "ref" ? 22 : 6,
                }}
                onClick={(e) => click(e, row)}
                onDoubleClick={() => {
                  if (row.kind === "ref" && row.section !== "tags")
                    onCheckout(row.section, row.name);
                }}
              >
                {row.kind === "section" &&
                  (row.open ? <ChevronDown size={12} /> : <ChevronRight size={12} />)}
                {row.kind === "head" && <Star size={12} />}
                {row.kind === "ref" &&
                  (row.section === "tags" ? <Tag size={12} /> : <GitBranch size={12} />)}
                <span
                  className={`truncate ${row.kind === "ref" && row.current ? "font-semibold" : ""}`}
                >
                  {label}
                </span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
