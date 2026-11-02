// M4-12 의 rename live 시험 — `relabel` 의 import 하나 · 호출 둘.
import { relabel } from "./m4_12_target";

export function twice(x: number): number {
  return relabel(relabel(x));
}
