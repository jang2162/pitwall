// 심볼 인덱스 픽스처 — 바꾸면 스냅숏(tests/snapshots/)도 바뀐다.

interface Props {
  title: string;
}

type Size = "small" | "large";

export function Title({ title }: Props) {
  return <h1 className="title">{title}</h1>;
}

export const Badge = ({ title }: Props) => <span>{title}</span>;

export class Panel {
  size: Size = "small";

  render() {
    return <div>{this.size}</div>;
  }
}
