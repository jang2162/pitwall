// 창의 실행 구성 목록과 고른 구성(17-m6-plan M6-07 ① — 실행 선택기 · `⌃R` 이 같은 것을 본다). React 밖의 모델이다(run-tabs.ts 와
// 같은 꼴 — 창마다 하나, `windowRunConfigs()`).
//
// - 목록은 `run.config.list { projectId, worktree }`(활성 워크트리 — 공유 구성을 그 워크트리에서 읽는다). 세 묶음: 쓸 수 있는 구성 ·
//   감지된 태스크 · 쓸 수 없는 구성(까닭). 다시 읽는 때: 처음, 활성 워크트리가 바뀔 때, 선택기를 열 때, 저장 · 지우기 뒤, 감지가
//   바뀌었다는 `config.changed`(Host 의 `detect::follow_files`). 실행은 Host 가 그때 구성을 다시 읽으므로 목록이 늦어도 다른 것이
//   돌지 않는다.
// - 고른 구성 · 프로필은 이 기기의 이 프로젝트에 기억한다(localStorage — 읽지 못해도 첫 구성으로 돌아갈 뿐이다).
// - gradle · maven(· mise) 태스크는 사람이 「태스크 읽기」 를 누를 때만 읽는다(결정 D70 — `run.tasks.refresh`). 같은 감지기를 읽는
//   동안에는 다시 누르지 못한다.

import type {
  DetectedRunTask,
  RpcClient,
  RunConfigEntry,
  RunConfigList,
  UnusableRunConfig,
} from "@pitwall/rpc-client";
import { createStore, type StoreApi } from "zustand/vanilla";

export type RunConfigsClient = Pick<RpcClient, "call" | "subscribe">;

/** 고른 구성. */
export interface RunTarget {
  id: string;
  profile: string | null;
}

export interface RunConfigsState {
  list: RunConfigList | undefined;
  /** 목록을 읽지 못한 까닭. */
  error: string | undefined;
  selected: RunTarget | undefined;
  /** 「태스크 읽기」 가 도는 감지기. */
  reading: readonly string[];
  /** 「태스크 읽기」 가 실패한 까닭(감지기별 — 다음 읽기까지). */
  readErrors: Readonly<Record<string, string>>;
}

const KEY = (projectId: string) => `pitwall.run.selected.${projectId}`;

function remembered(projectId: string): RunTarget | undefined {
  try {
    const text = localStorage.getItem(KEY(projectId));
    if (!text) return undefined;
    const value = JSON.parse(text) as Partial<RunTarget>;
    return typeof value.id === "string"
      ? { id: value.id, profile: typeof value.profile === "string" ? value.profile : null }
      : undefined;
  } catch {
    return undefined;
  }
}

function remember(projectId: string, target: RunTarget) {
  try {
    localStorage.setItem(KEY(projectId), JSON.stringify(target));
  } catch {
    // 기억하지 못해도 고른 것은 이 창에 남는다.
  }
}

export function errorText(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) {
    const data = "data" in e && typeof e.data === "string" ? `: ${e.data}` : "";
    return `${String(e.message)}${data}`;
  }
  return String(e);
}

/** 쓸 수 있는 구성 · 감지된 태스크 · 쓸 수 없는 구성의 이름. */
export interface RunGroups {
  configs: RunConfigEntry[];
  detected: DetectedRunTask[];
  unusable: UnusableRunConfig[];
}

export class RunConfigs {
  readonly store: StoreApi<RunConfigsState> = createStore<RunConfigsState>()(() => ({
    list: undefined,
    error: undefined,
    selected: undefined,
    reading: [],
    readErrors: {},
  }));
  private where:
    | { client: RunConfigsClient; projectId: string; worktree: string | null }
    | undefined;
  /** 마지막으로 보낸 목록 요청 — 늦게 온 옛 답을 버린다. */
  private generation = 0;

  /** 창이 Host · 프로젝트 · 활성 워크트리를 정하고 목록을 읽는다. 걷는 함수(이벤트 구독 해제)를 돌려준다. */
  connect(client: RunConfigsClient, projectId: string, worktree: string | null): () => void {
    this.where = { client, projectId, worktree };
    if (!this.store.getState().selected) {
      const target = remembered(projectId);
      if (target) this.store.setState({ selected: target });
    }
    void this.reload();
    let stopped = false;
    let unsubscribe: (() => Promise<void>) | undefined;
    client
      .subscribe({ projectId, topics: ["config.changed"] }, () => {
        if (!stopped) void this.reload();
      })
      .then((s) => {
        unsubscribe = () => s.unsubscribe();
        if (stopped) void s.unsubscribe().catch(() => {});
      })
      .catch((e: unknown) => console.warn("[run] config events not followed", e));
    return () => {
      stopped = true;
      void unsubscribe?.().catch(() => {});
      if (this.where?.client === client) this.where = undefined;
    };
  }

  /** 목록을 다시 읽는다. */
  async reload(): Promise<void> {
    const where = this.where;
    if (!where) return;
    const generation = ++this.generation;
    try {
      const list = await where.client.call("run.config.list", {
        projectId: where.projectId,
        worktree: where.worktree,
      });
      if (generation === this.generation) this.store.setState({ list, error: undefined });
    } catch (e) {
      if (generation === this.generation) this.store.setState({ error: errorText(e) });
    }
  }

  groups(): RunGroups {
    const list = this.store.getState().list;
    return {
      configs: list?.configs ?? [],
      detected: list?.detected ?? [],
      unusable: list?.unusable ?? [],
    };
  }

  /** 구성을 고른다(실행하지 않는다). */
  select(target: RunTarget): void {
    this.store.setState({ selected: target });
    if (this.where) remember(this.where.projectId, target);
  }

  /**
   * 지금 구성: 고른 것이 목록에 있으면 그것(없어진 프로필이면 프로필 없이), 쓸 수 없게 됐으면 그 까닭, 고른 것이 없으면 첫 구성.
   */
  current():
    | { kind: "usable"; entry: RunConfigEntry; profile: string | null }
    | { kind: "unusable"; entry: UnusableRunConfig }
    | undefined {
    const { selected } = this.store.getState();
    const { configs, unusable } = this.groups();
    if (selected) {
      const entry = configs.find((c) => c.id === selected.id);
      if (entry) {
        const profiles = entry.config.profiles ?? {};
        const profile =
          selected.profile !== null && selected.profile in profiles ? selected.profile : null;
        return { kind: "usable", entry, profile };
      }
      const broken = unusable.find((u) => u.id === selected.id);
      if (broken) return { kind: "unusable", entry: broken };
    }
    const first = configs[0];
    return first ? { kind: "usable", entry: first, profile: null } : undefined;
  }

  /** 「태스크 읽기」(D70 — 누를 때만). 읽은 뒤 목록을 다시 읽는다. */
  async readTasks(detector: string): Promise<void> {
    const where = this.where;
    const state = this.store.getState();
    if (!where || state.reading.includes(detector)) return;
    const { [detector]: _, ...readErrors } = state.readErrors;
    this.store.setState({ reading: [...state.reading, detector], readErrors });
    try {
      const result = await where.client.call("run.tasks.refresh", {
        projectId: where.projectId,
        worktree: where.worktree,
        detector,
      });
      if (result.error)
        this.store.setState((s) => ({
          readErrors: { ...s.readErrors, [detector]: result.error! },
        }));
    } catch (e) {
      this.store.setState((s) => ({ readErrors: { ...s.readErrors, [detector]: errorText(e) } }));
    } finally {
      this.store.setState((s) => ({ reading: s.reading.filter((d) => d !== detector) }));
      await this.reload();
    }
  }

  /** 이 창이 붙은 곳(편집기 · 실행이 같은 값을 쓴다). */
  place(): { client: RunConfigsClient; projectId: string; worktree: string | null } | undefined {
    return this.where;
  }
}

let current: RunConfigs | undefined;

/** 이 창의 실행 구성 목록(창마다 웹뷰가 따로라 모듈에 하나). */
export function windowRunConfigs(): RunConfigs {
  current ??= new RunConfigs();
  return current;
}

/** 시험: 창의 모델을 새로 만든다. */
export function resetWindowRunConfigs(): void {
  current = undefined;
}
