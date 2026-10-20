// A line comment.
import { readFile } from "node:fs";

interface Point {
  readonly x: number;
  y?: string;
}

export class Shape<T> implements Point {
  readonly x = 0x1f;
  constructor(private name: T) {}
  area(): number {
    return this.x * 2;
  }
}

export const re = /ab+c/g;
export type Id = string | null;
export { readFile };
