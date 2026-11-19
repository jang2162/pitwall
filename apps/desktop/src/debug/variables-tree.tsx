// 변수 · 워치 나무(17-m6-plan M6-18 ②). 자식은 **펼칠 때만** 읽는다(`debug.variables` — 쪽 단위, 기본 100). 처음에는 범위(또는 워치
// 결과)의 줄만 그리고 아무것도 읽지 않는다.
//
// 값은 디버그 대상이 정한 글이다 — 한 줄 칸에 보이게 바꿔 자르고(value-text.ts `oneLine`), 풍선 도움말에는 더 긴 상한으로 싣는다.
// 깊이는 `MAX_DEPTH` 까지(순환 참조를 끝없이 펼치지 않게 — 그 아래는 「너무 깊다」), 한 노드의 자식은 쪽마다 「n개 더」 로 이어 읽는다.

import type { DebugVariable, DebugVariables } from "@pitwall/rpc-client";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useState } from "react";

import type { DebugStrings } from "./strings";
import { cappedText, oneLine } from "./value-text";

/** 한 쪽의 크기. */
export const PAGE = 100;
/** 펼칠 수 있는 깊이. */
export const MAX_DEPTH = 32;

export interface TreeNode {
  name: string;
  value?: string;
  type?: string | null;
  variablesReference: number;
  /** 아는 자식 수(이름 · 번호). 쪽을 더 읽을지 가른다. */
  namedVariables?: number | null;
  indexedVariables?: number | null;
}

export type LoadPage = (
  variablesReference: number,
  start: number,
  count: number,
) => Promise<DebugVariables>;

export function fromVariable(v: DebugVariable): TreeNode {
  return {
    name: v.name,
    value: v.value,
    type: v.type,
    variablesReference: v.variablesReference,
    namedVariables: v.namedVariables,
    indexedVariables: v.indexedVariables,
  };
}

export function VariablesTree({
  roots,
  load,
  T,
  label,
}: {
  roots: readonly TreeNode[];
  load: LoadPage;
  T: DebugStrings;
  label: string;
}) {
  return (
    <ul role="tree" aria-label={label} className="min-w-0 font-mono text-[12px]">
      {roots.map((node, i) => (
        <VariableRow key={`${i}:${node.name}`} node={node} depth={0} load={load} T={T} />
      ))}
    </ul>
  );
}

interface Children {
  items: TreeNode[];
  /** 마지막 쪽이 가득 찼다 — 더 있을 수 있다. */
  more: boolean;
  loading: boolean;
  error?: string;
}

function VariableRow({
  node,
  depth,
  load,
  T,
}: {
  node: TreeNode;
  depth: number;
  load: LoadPage;
  T: DebugStrings;
}) {
  const [open, setOpen] = useState(false);
  const [children, setChildren] = useState<Children>();
  const expandable = node.variablesReference > 0;
  const tooDeep = depth + 1 >= MAX_DEPTH;

  const readPage = (start: number) => {
    setChildren((c) => ({ items: c?.items ?? [], more: false, loading: true }));
    load(node.variablesReference, start, PAGE)
      .then((page) =>
        setChildren((c) => ({
          items: [...(c?.items ?? []), ...page.variables.map(fromVariable)],
          more: page.variables.length >= page.count,
          loading: false,
        })),
      )
      .catch((e: unknown) =>
        setChildren((c) => ({
          items: c?.items ?? [],
          more: false,
          loading: false,
          error: e instanceof Error ? e.message : String(e),
        })),
      );
  };

  const toggle = () => {
    if (!expandable) return;
    const next = !open;
    setOpen(next);
    if (next && !children && !tooDeep) readPage(0);
  };

  const value = node.value === undefined ? undefined : oneLine(node.value);
  const title = node.value === undefined ? undefined : cappedText(node.value).text;
  return (
    <li role="treeitem" aria-expanded={expandable ? open : undefined} aria-level={depth + 1}>
      <button
        type="button"
        data-variable={node.name}
        onClick={toggle}
        title={title}
        style={{ paddingLeft: depth * 12 + 4 }}
        className="flex w-full min-w-0 items-center gap-1 py-px text-left hover:bg-ui-hover"
      >
        <span className="flex w-3 flex-none justify-center text-ui-muted-foreground">
          {expandable && (open ? <ChevronDown size={11} /> : <ChevronRight size={11} />)}
        </span>
        <span className="flex-none text-ui-accent">{oneLine(node.name).text}</span>
        {value && (
          <>
            <span className="flex-none text-ui-muted-foreground">=</span>
            <span className="min-w-0 truncate">{value.text}</span>
            {value.cut && <span className="flex-none text-ui-muted-foreground">…</span>}
          </>
        )}
        {node.type && (
          <span className="ml-auto flex-none truncate pl-2 text-ui-muted-foreground">
            {oneLine(node.type).text}
          </span>
        )}
      </button>
      {open && expandable && (
        <ul role="group">
          {tooDeep && <Note depth={depth + 1}>{T.tooDeep}</Note>}
          {children?.items.map((child, i) => (
            <VariableRow
              key={`${i}:${child.name}`}
              node={child}
              depth={depth + 1}
              load={load}
              T={T}
            />
          ))}
          {children?.error && <Note depth={depth + 1}>{T.readFailed(children.error)}</Note>}
          {children?.more && !children.loading && (
            <li style={{ paddingLeft: (depth + 1) * 12 + 20 }}>
              <button
                type="button"
                data-variables-more=""
                className="text-ui-accent hover:underline"
                onClick={() => readPage(children.items.length)}
              >
                {T.more(PAGE)}
              </button>
            </li>
          )}
        </ul>
      )}
    </li>
  );
}

function Note({ depth, children }: { depth: number; children: string }) {
  return (
    <li style={{ paddingLeft: depth * 12 + 20 }} className="text-ui-muted-foreground">
      {children}
    </li>
  );
}
