// A line comment.
import { join } from "node:path";

/** Doc comment. */
export async function load(name, options = {}) {
  const path = join("/tmp", `${name}.json`);
  if (options.verbose === true) {
    console.log("loading", path, 42, null);
  }
  return new Map([[name, /x+/u]]);
}

export class Box extends Object {
  static #count = 0;
  get size() { return this.#count; }
}
