// 키 트리 모델(key-tree.ts): 구분자로 묶기 · 펼친 만큼만 줄로 · 이름 순 정렬(스캔 중 큰 그룹은 미룸) · 같은 키는 정보만 바꿈 ·
// 바이트 키는 묶지 않음 · 지우기와 빈 그룹 걷기 · 구분자 바꾸기 · 보유 상한.

import type { KvKeyInfo } from "@pitwall/rpc-client";
import { describe, expect, it } from "vitest";

import { KeyTree, MAX_TREE_KEYS, SORT_WHILE_LOADING } from "./key-tree";

const now = (run: () => void) => run();

function info(key: KvKeyInfo["key"], over: Partial<KvKeyInfo> = {}): KvKeyInfo {
  return { key, kind: "string", ttlMs: null, size: 1, ...over };
}

/** 펼친 줄을 사람이 읽는 꼴로: 그룹은 `이름/(수)`, 키는 마지막 조각. 깊이만큼 들인다. */
function show(tree: KeyTree, expanded: string[] = []): string[] {
  const rows = tree.rows(new Set(expanded));
  return rows.items.map((item, i) => {
    const pad = "  ".repeat(rows.depths[i]!);
    return typeof item === "number"
      ? pad + tree.leafLabel(item)
      : `${pad}${item.name}/(${item.count})`;
  });
}

describe("KeyTree", () => {
  it("groups keys by the separator and lists only the expanded groups, sorted by name", () => {
    const tree = new KeyTree(":", now);
    tree.setLoading(false);
    tree.add(["user:2", "user:1:name", "order:9", "plain", "user:1:mail", "a"].map((k) => info(k)));
    expect(show(tree)).toEqual(["order/(1)", "user/(3)", "a", "plain"]);
    expect(show(tree, ["user:"])).toEqual([
      "order/(1)",
      "user/(3)",
      "  1/(2)",
      "  2",
      "a",
      "plain",
    ]);
    expect(show(tree, ["user:", "user:1:"])).toEqual([
      "order/(1)",
      "user/(3)",
      "  1/(2)",
      "    mail",
      "    name",
      "  2",
      "a",
      "plain",
    ]);
    expect(tree.size).toBe(6);
  });

  it("updates the info of a key that comes twice instead of adding it again", () => {
    const tree = new KeyTree(":", now);
    tree.add([info("k:1", { ttlMs: 5 })]);
    tree.add([info("k:1", { ttlMs: 9, kind: "hash" })]);
    expect(tree.size).toBe(1);
    expect(show(tree, ["k:"])).toEqual(["k/(1)", "  1"]);
    expect(tree.find("k:1")?.info).toMatchObject({ ttlMs: 9, kind: "hash" });
  });

  it("keeps byte keys ungrouped at the top and apart from the same-looking text", () => {
    const tree = new KeyTree(":", now);
    tree.setLoading(false);
    tree.add([info({ hex: "0x3a3a" }), info("0x3a3a"), info({ hex: "0xff" })]);
    expect(tree.size).toBe(3);
    expect(tree.find({ hex: "0x3a3a" })).not.toBe(tree.find("0x3a3a"));
    expect(show(tree)).toEqual(["0x3a3a", "0x3a3a", "0xff"]);
  });

  it("removes a key and drops the groups it leaves empty", () => {
    const tree = new KeyTree(":", now);
    tree.add([info("a:b:c"), info("a:x")]);
    tree.remove("a:b:c");
    expect(show(tree, ["a:", "a:b:"])).toEqual(["a/(1)", "  x"]);
    expect(tree.find("a:b:c")).toBeUndefined();
    // 다시 오면 새로 든다.
    tree.add([info("a:b:c")]);
    expect(show(tree, ["a:", "a:b:"])).toEqual(["a/(2)", "  b/(1)", "    c", "  x"]);
    expect(tree.size).toBe(2);
  });

  it("regroups on a new separator, and an empty separator lists every key flat", () => {
    const tree = new KeyTree(":", now);
    tree.setLoading(false);
    tree.add([info("a:b/c"), info("a/d")]);
    tree.setSeparator("/");
    expect(show(tree, ["a/", "a:b/"])).toEqual(["a/(1)", "  d", "a:b/(1)", "  c"]);
    tree.setSeparator("");
    expect(show(tree)).toEqual(["a/d", "a:b/c"]);
  });

  it("does not sort a big group while scanning and sorts it once the scan settles", () => {
    const tree = new KeyTree(":", now);
    const n = SORT_WHILE_LOADING + 1;
    tree.add(Array.from({ length: n }, (_, i) => info(`k:${String(n - i).padStart(6, "0")}`)));
    const first = () => show(tree, ["k:"])[1]!.trim();
    expect(first()).toBe(String(n).padStart(6, "0"));
    tree.setLoading(false);
    expect(first()).toBe("000001");
  });

  it("holds at most its key limit and reports the dropped ones", () => {
    expect(new KeyTree().maxKeys).toBe(MAX_TREE_KEYS);
    const tree = new KeyTree(":", now, 1_000);
    const batch = 300;
    let dropped = 0;
    for (let start = 0; start < 1_200; start += batch)
      dropped += tree.add(Array.from({ length: batch }, (_, i) => info(`k:${start + i}`)));
    expect(tree.size).toBe(1_000);
    expect(tree.full).toBe(true);
    expect(dropped).toBe(200);
    // 이미 든 키의 정보는 상한에서도 바뀐다.
    expect(tree.add([info("k:0", { ttlMs: 7 })])).toBe(0);
    expect(tree.find("k:0")?.info.ttlMs).toBe(7);
  });

  it("tells subscribers once per frame", () => {
    const frames: (() => void)[] = [];
    const tree = new KeyTree(":", (run) => frames.push(run));
    let told = 0;
    tree.subscribe(() => told++);
    tree.add([info("a")]);
    tree.add([info("b")]);
    tree.setTtl("a", 5);
    expect(frames).toHaveLength(1);
    frames[0]!();
    expect(told).toBe(1);
    expect(tree.find("a")?.info.ttlMs).toBe(5);
  });
});
