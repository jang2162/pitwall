import { describe, expect, it } from "vitest";

import { chooseDefaultKeymap, DEFAULT_KEYMAP_TOML, registryKeymapProblem } from "./default-keymap";

describe("registryKeymapProblem", () => {
  const fromRegistry = chooseDefaultKeymap(
    { content: DEFAULT_KEYMAP_TOML, extension: "dev.pitwall.keymap-default" },
    "mac",
  );

  it("does not report a refetch error while the registry keymap is in use", () => {
    expect(
      registryKeymapProblem({ data: { content: "" }, error: new Error("later") }, fromRegistry),
    ).toBeUndefined();
  });

  it("reports a failed first fetch and an unreadable registry file", () => {
    const bundled = chooseDefaultKeymap(undefined, "mac");
    expect(registryKeymapProblem({ error: new Error("no host") }, bundled)).toBe("Error: no host");
    const broken = chooseDefaultKeymap(
      { content: "schema = 1\n[[bindings]]\naction = 1\n", extension: "x" },
      "mac",
    );
    expect(registryKeymapProblem({ data: {}, error: null }, broken)).toBe(broken.error);
    expect(broken.error).toBeTruthy();
  });
});
