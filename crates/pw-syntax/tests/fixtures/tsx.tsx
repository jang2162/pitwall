// A line comment.
import React from "react";

type Props = { title: string; count?: number };

export function Badge({ title, count = 0 }: Props): JSX.Element {
  const label = `${title}: ${count}`;
  return (
    <div className="badge" data-count={count}>
      {label}
    </div>
  );
}
