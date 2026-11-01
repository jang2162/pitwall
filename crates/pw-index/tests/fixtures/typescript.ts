// 심볼 인덱스 픽스처 — 바꾸면 스냅숏(tests/snapshots/)도 바뀐다.

export interface Shape {
  name: string;
  area(): number;
}

export type Pair = [number, number];

export enum Color {
  Red,
  Green = 2,
}

export abstract class Base implements Shape {
  abstract area(): number;
  name = "base";
}

export class Circle extends Base {
  radius: number;

  constructor(radius: number) {
    super();
    this.radius = radius;
  }

  area(): number {
    return Math.PI * this.radius * this.radius;
  }
}

export function makeCircle(r: number): Circle {
  return new Circle(r);
}

export const square = (x: number): number => x * x;

export namespace Geometry {
  export function origin(): Pair {
    return [0, 0];
  }
}

declare function external(value: string): void;
