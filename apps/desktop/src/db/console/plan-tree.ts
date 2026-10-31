// 실행 계획(`db.query.explain`, 14-m3-plan.md M3-15)을 들여쓴 트리(노드 · 비용 · 행 수)로. 그림(노드 그래프)은 이후다(§3).
//
// - PG(`EXPLAIN (FORMAT JSON)`): `[{ "Plan": { "Node Type", "Relation Name", "Startup Cost", "Total Cost", "Plan Rows", "Plans" } }]`.
// - MySQL · MariaDB(`EXPLAIN FORMAT=JSON`): `query_block` 아래의 묶음(`nested_loop` · `ordering_operation` · `grouping_operation`
//   · `duplicates_removal` · `union_result` …)과 `table { table_name, access_type, rows_examined_per_scan | rows, cost_info }`.
//   알려진 이름에 기대지 않고 객체 · 배열을 따라 내려간다 — 판에 따라 이름이 늘어도 노드가 빠지지 않는다.
// - SQLite(`EXPLAIN QUERY PLAN`, 텍스트): 드라이버가 두 칸씩 들여 쓴 줄 — 들여쓰기로 트리를 만든다.
// - JSON 으로 읽지 못한 글은 줄마다 노드 하나(원문 그대로).

import type { ExplainPlan } from "@pitwall/rpc-client";

export interface PlanNode {
  label: string;
  /** 비용(PG `startup..total`, MySQL `prefix_cost` 등). */
  cost?: string;
  /** 어림 행 수. */
  rows?: string;
  /** 그 밖의 한 줄(조건 · 인덱스 등). */
  detail?: string;
  children: PlanNode[];
}

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

function isObject(v: Json | undefined): v is { [key: string]: Json } {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function text(v: Json | undefined): string | undefined {
  if (v === undefined || v === null) return undefined;
  return typeof v === "object" ? undefined : String(v);
}

function pgNode(plan: { [key: string]: Json }): PlanNode {
  const type = text(plan["Node Type"]) ?? "?";
  const target =
    text(plan["Relation Name"]) ?? text(plan["Function Name"]) ?? text(plan["CTE Name"]);
  const alias = text(plan.Alias);
  const index = text(plan["Index Name"]);
  let label = type;
  if (index) label += ` using ${index}`;
  if (target) label += ` on ${target}${alias && alias !== target ? ` ${alias}` : ""}`;
  const startup = text(plan["Startup Cost"]);
  const total = text(plan["Total Cost"]);
  const condition =
    text(plan.Filter) ??
    text(plan["Index Cond"]) ??
    text(plan["Hash Cond"]) ??
    text(plan["Join Filter"]) ??
    text(plan["Merge Cond"]);
  const children = Array.isArray(plan.Plans) ? plan.Plans.filter(isObject).map(pgNode) : [];
  return {
    label,
    ...(total !== undefined && { cost: startup !== undefined ? `${startup}..${total}` : total }),
    ...(text(plan["Plan Rows"]) !== undefined && { rows: text(plan["Plan Rows"]) }),
    ...(condition !== undefined && { detail: condition }),
    children,
  };
}

/** MySQL · MariaDB 의 객체 하나(이름 `key`)를 노드로. 표는 표 이름 · 접근 방법, 그 밖은 이름 그대로. */
function mysqlNode(key: string, value: { [key: string]: Json }): PlanNode {
  const children: PlanNode[] = [];
  for (const [k, v] of Object.entries(value)) {
    if (k === "cost_info") continue;
    if (isObject(v)) children.push(mysqlNode(k, v));
    else if (Array.isArray(v))
      for (const item of v) {
        if (!isObject(item)) continue;
        // `nested_loop: [{ table: … }]` — 원소가 이름 하나짜리 묶음이면 그 안을 바로 노드로.
        const entries = Object.entries(item);
        const only = entries.length === 1 ? entries[0] : undefined;
        if (only && isObject(only[1])) children.push(mysqlNode(only[0], only[1]));
        else children.push(mysqlNode(k, item));
      }
  }
  const cost = isObject(value.cost_info)
    ? (text(value.cost_info.prefix_cost) ??
      text(value.cost_info.query_cost) ??
      text(value.cost_info.read_cost))
    : undefined;
  const rows = text(value.rows_examined_per_scan) ?? text(value.rows);
  const table = key === "table" ? text(value.table_name) : undefined;
  const access = text(value.access_type);
  const label = table ? `${table}${access ? ` (${access})` : ""}` : key;
  const detail =
    text(value.attached_condition) ??
    text(value.key) ??
    (key === "query_block" ? text(value.message) : undefined);
  return {
    label,
    ...(cost !== undefined && { cost }),
    ...(rows !== undefined && { rows }),
    ...(detail !== undefined && { detail }),
    children,
  };
}

/** 두 칸 들여쓰기의 줄들을 트리로(SQLite, 읽지 못한 글). */
function indentedNodes(lines: readonly string[]): PlanNode[] {
  const roots: PlanNode[] = [];
  const stack: { depth: number; node: PlanNode }[] = [];
  for (const line of lines) {
    if (line.trim() === "") continue;
    const depth = line.length - line.trimStart().length;
    const node: PlanNode = { label: line.trim(), children: [] };
    while (stack.length > 0 && stack[stack.length - 1]!.depth >= depth) stack.pop();
    const parent = stack[stack.length - 1];
    (parent ? parent.node.children : roots).push(node);
    stack.push({ depth, node });
  }
  return roots;
}

export function planTree(plan: ExplainPlan): PlanNode[] {
  if (plan.format === "text") return indentedNodes(plan.text.split(/\r?\n/));
  let parsed: Json;
  try {
    parsed = JSON.parse(plan.text) as Json;
  } catch {
    return indentedNodes(plan.text.split(/\r?\n/));
  }
  // PG: 문장 하나에 `[{ Plan }]`.
  if (Array.isArray(parsed) && parsed.every((p) => isObject(p) && isObject(p.Plan)))
    return parsed.map((p) => pgNode((p as { Plan: { [key: string]: Json } }).Plan));
  if (isObject(parsed))
    return Object.entries(parsed).flatMap(([k, v]) => (isObject(v) ? [mysqlNode(k, v)] : []));
  return indentedNodes(plan.text.split(/\r?\n/));
}
