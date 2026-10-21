import { describe, expect, it } from "vitest";

import { parseWindowKind } from "./window-kind";

describe("parseWindowKind", () => {
  it("reads the kind Rust puts in the query", () => {
    expect(parseWindowKind("?window=welcome")).toBe("welcome");
    expect(parseWindowKind("?window=project")).toBe("project");
    expect(parseWindowKind("?window=settings")).toBe("settings");
    expect(parseWindowKind("?window=file&path=%2Fx%2Fa.txt")).toBe("file");
  });

  it("falls back to welcome for a missing or unknown kind", () => {
    expect(parseWindowKind("")).toBe("welcome");
    expect(parseWindowKind("?window=nope")).toBe("welcome");
  });
});
