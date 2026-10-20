#!/usr/bin/env bash
# A comment.
set -euo pipefail

NAME="pitwall"
count=$((1 + 2))

greet() {
  local who="$1"
  echo "hello ${who}" > /dev/null
}

if [[ -n "$NAME" ]]; then
  for f in *.txt; do
    greet "$f" | grep -q 'x'
  done
fi
