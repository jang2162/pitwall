// S2 프로젝트 찾기 팝업(10-ui §2.2, F2). `pitwall <path>` 가 등록된 프로젝트를 찾지 못했을 때 루트 후보(`project.candidates`)를
// 상단 검색 팝업으로 보여 준다. 후보 · 직접 고른 폴더는 S3 으로 간다. 단일 파일 뷰어 창은 아직 없어 「파일만 열기」는 끈다.

import { Command } from "cmdk";

import { useKeymapPopup } from "../keymap/keymap-provider";
import { useRpcQuery } from "../rpc/host";
import { SEARCH_ITEM_CLASS, SearchPopup } from "../ui/search-popup";
import type { FindProjectRequest } from "./native";
import { candidateReasons, errorText } from "./projects";
import { STRINGS } from "./strings";

const T = STRINGS.find;
const PICK_FOLDER = "__pick-folder";
const FILE_ONLY = "__file-only";

export function FindProjectPopup({
  request,
  onPick,
  onPickFolder,
  onCancel,
}: {
  request: FindProjectRequest;
  onPick(root: string): void;
  onPickFolder(): void;
  onCancel(): void;
}) {
  useKeymapPopup(true);
  const candidates = useRpcQuery("project.candidates", { path: request.path });
  const list = candidates.data ?? [];
  return (
    <SearchPopup
      open
      onOpenChange={(open) => !open && onCancel()}
      label={T.label}
      title={T.title(request.path)}
      placeholder={T.search}
      empty={T.empty}
    >
      {candidates.isPending && <Command.Loading className="px-3 py-1">{T.loading}</Command.Loading>}
      {candidates.error && (
        <div role="alert" className="px-3 py-1 text-ui-accent">
          {STRINGS.errors.action(STRINGS.errors.candidates, errorText(candidates.error))}
        </div>
      )}
      {list.map((candidate, i) => (
        <Command.Item
          key={candidate.path}
          value={`candidate:${candidate.path}`}
          keywords={[candidate.path]}
          onSelect={() => onPick(candidate.path)}
          className={SEARCH_ITEM_CLASS}
        >
          <span className="min-w-0 flex-1 truncate" title={candidate.path}>
            {candidate.path}
          </span>
          <span className="flex-none text-[11px] text-ui-muted-foreground">
            {candidateReasons(candidate)}
            {i === 0 && ` (${T.recommended})`}
          </span>
        </Command.Item>
      ))}
      {list.length > 0 && <Command.Separator className="my-1 h-px bg-ui-border" alwaysRender />}
      <Command.Item
        value={PICK_FOLDER}
        forceMount
        onSelect={onPickFolder}
        className={SEARCH_ITEM_CLASS}
      >
        {T.pickFolder}
      </Command.Item>
      <Command.Item
        value={FILE_ONLY}
        forceMount
        disabled
        title={T.fileOnlyNotYet}
        className={SEARCH_ITEM_CLASS}
      >
        <span className="flex-1">{T.fileOnly}</span>
        <span className="text-[11px]">{T.fileOnlyNotYet}</span>
      </Command.Item>
    </SearchPopup>
  );
}
