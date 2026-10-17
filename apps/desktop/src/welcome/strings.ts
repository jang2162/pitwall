// 웰컴 창(S1 · S2 · S3)이 보여 주는 문자열. i18n(M0-14)이 들어오면 이 파일을 Fluent 리소스로 옮긴다 — 화면 코드에는 문자열을
// 두지 않고 여기서만 가져다 쓴다. 값이 끼는 문자열은 함수다(Fluent 의 변수 자리).

import type { Platform } from "@pitwall/keymap";

export const STRINGS = {
  nav: {
    label: "웰컴",
    projects: "프로젝트",
    remoteHosts: "원격 호스트",
    extensions: "확장",
    /** 아직 없는 탭에 붙는 설명. */
    notYet: "아직 없다",
  },
  list: {
    label: "프로젝트 목록",
    search: "프로젝트 검색",
    empty: "등록된 프로젝트가 없다. 새 프로젝트를 만들거나 폴더를 연다.",
    noMatch: "맞는 프로젝트가 없다",
    favorite: "즐겨찾기",
    remote: "원격",
  },
  buttons: {
    newProject: "새 프로젝트…",
    openFolder: "폴더 열기…",
  },
  menu: {
    open: "열기",
    reveal: (platform: Platform) =>
      platform === "mac"
        ? "Finder 에서 보기"
        : platform === "win"
          ? "탐색기에서 보기"
          : "파일 관리자에서 보기",
    remove: "목록에서 제거",
  },
  actions: {
    newProject: "새 프로젝트",
    openFolder: "폴더 열기",
    open: "선택한 프로젝트 열기",
    reveal: (platform: Platform) => `선택한 프로젝트를 ${STRINGS.menu.reveal(platform)}`,
    remove: "선택한 프로젝트를 목록에서 제거",
    create: "프로젝트 만들기",
  },
  drop: "폴더를 놓으면 새 프로젝트를 만든다",
  storage: {
    recovered: (reason: string, backup: string) =>
      `프로젝트 목록 파일이 손상돼 새로 만들었다(${reason}). 옛 파일: ${backup}`,
    readOnly: (reason: string) => `프로젝트 목록을 읽기만 한다 — 바꿀 수 없다(${reason})`,
    memory: (reason: string) =>
      `프로젝트 목록 파일을 열지 못했다 — 목록이 비어 보이고 바꿀 수 없다(${reason})`,
  },
  find: {
    label: "프로젝트 찾기",
    title: (path: string) => `${path} 를 열 프로젝트`,
    search: "폴더 경로",
    empty: "맞는 후보가 없다",
    loading: "후보를 찾는 중…",
    recommended: "추천",
    worktreeMain: "워크트리의 메인 저장소",
    pickFolder: "폴더 직접 선택…",
    fileOnly: "프로젝트 없이 파일만 열기",
    fileOnlyNotYet: "단일 파일 뷰어 창은 아직 없다",
  },
  create: {
    title: (name: string) => `새 프로젝트: ${name}`,
    name: "이름",
    root: "루트",
    changeRoot: "변경",
    chooseRoot: "선택…",
    rootNone: "고르지 않음",
    configMode: "설정 저장",
    repository: ".pitwall/ 에 저장(커밋)",
    personal: "개인 전용",
    detected: "감지됨",
    detectedEmpty: "감지된 항목이 없다",
    cancel: "취소",
    submit: "만들기",
  },
  errors: {
    action: (what: string, message: string) => `${what} 실패: ${message}`,
    open: "프로젝트 열기",
    remove: "목록에서 제거",
    pickFolder: "폴더 선택",
    reveal: "파일 관리자에서 보기",
    create: "프로젝트 만들기",
    findProject: "프로젝트 찾기 요청 읽기",
    candidates: "루트 후보 찾기",
  },
} as const;
