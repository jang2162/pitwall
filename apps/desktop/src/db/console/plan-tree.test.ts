// 실행 계획 트리(plan-tree.ts): PG JSON · MySQL/MariaDB JSON · SQLite 텍스트 · 읽지 못한 글을 들여쓴 노드로.

import { describe, expect, it } from "vitest";

import { type PlanNode, planTree } from "./plan-tree";

/** (깊이, 이름, 비용, 행) 줄로 편다. */
function flat(nodes: PlanNode[], depth = 0): [number, string, string?, string?][] {
  return nodes.flatMap((n) => [
    [depth, n.label, n.cost, n.rows] as [number, string, string?, string?],
    ...flat(n.children, depth + 1),
  ]);
}

describe("planTree", () => {
  it("reads PG's node type, relation, index, cost range, rows and condition", () => {
    const nodes = planTree({
      format: "json",
      text: JSON.stringify([
        {
          Plan: {
            "Node Type": "Hash Join",
            "Startup Cost": 1.1,
            "Total Cost": 30.5,
            "Plan Rows": 100,
            "Hash Cond": "(a.id = b.a_id)",
            Plans: [
              {
                "Node Type": "Seq Scan",
                "Relation Name": "a",
                Alias: "x",
                "Total Cost": 10,
                "Plan Rows": 50,
              },
              {
                "Node Type": "Index Scan",
                "Index Name": "b_pkey",
                "Relation Name": "b",
                Alias: "b",
                "Total Cost": 8,
                "Plan Rows": 2,
              },
            ],
          },
        },
      ]),
    });
    expect(flat(nodes)).toEqual([
      [0, "Hash Join", "1.1..30.5", "100"],
      [1, "Seq Scan on a x", "10", "50"],
      [1, "Index Scan using b_pkey on b", "8", "2"],
    ]);
    expect(nodes[0]!.detail).toBe("(a.id = b.a_id)");
  });

  it("walks MySQL's query block, nested loop and tables", () => {
    const nodes = planTree({
      format: "json",
      text: JSON.stringify({
        query_block: {
          select_id: 1,
          cost_info: { query_cost: "3.20" },
          nested_loop: [
            {
              table: {
                table_name: "t",
                access_type: "ALL",
                rows_examined_per_scan: 10,
                cost_info: { prefix_cost: "1.25" },
              },
            },
            {
              table: {
                table_name: "u",
                access_type: "ref",
                key: "u_t",
                rows_examined_per_scan: 1,
                cost_info: { prefix_cost: "3.20" },
              },
            },
          ],
        },
      }),
    });
    expect(flat(nodes)).toEqual([
      [0, "query_block", "3.20", undefined],
      [1, "t (ALL)", "1.25", "10"],
      [1, "u (ref)", "3.20", "1"],
    ]);
    expect(nodes[0]!.children[1]!.detail).toBe("u_t");
  });

  it("nests SQLite's indented text and keeps unreadable JSON as lines", () => {
    expect(
      flat(
        planTree({
          format: "text",
          text: "SCAN t\n  SEARCH u USING INDEX i (a=?)\nUSE TEMP B-TREE",
        }),
      ),
    ).toEqual([
      [0, "SCAN t", undefined, undefined],
      [1, "SEARCH u USING INDEX i (a=?)", undefined, undefined],
      [0, "USE TEMP B-TREE", undefined, undefined],
    ]);
    expect(flat(planTree({ format: "json", text: "not json\n  child" }))).toEqual([
      [0, "not json", undefined, undefined],
      [1, "child", undefined, undefined],
    ]);
    expect(planTree({ format: "text", text: "" })).toEqual([]);
  });
});
