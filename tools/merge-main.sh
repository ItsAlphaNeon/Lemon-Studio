#!/bin/sh
# Copyright (C) 2026 thacer13
# SPDX-License-Identifier: GPL-3.0-or-later
# This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

# Merges main into the cutting-edge branch.
#
# This branch keeps its own README.md, and under docs/ only its own files: whatever main changes in those, or adds
# to docs/, is left out. A file this branch deleted stays deleted. Any other conflict is left for a person.
set -e
cd "$(git rev-parse --show-toplevel)"
[ "$(git branch --show-current)" = cutting-edge ] || { echo "run this on cutting-edge"; exit 1; }
no_fetch=0
for arg in "$@"; do
  case "$arg" in
    --no-fetch) no_fetch=1 ;;
    *) echo "usage: $0 [--no-fetch]" >&2; exit 2 ;;
  esac
done
git config merge.ours.driver true
ours_docs="$(git -c core.quotepath=false ls-files docs)"
if [ "$no_fetch" -eq 0 ]; then
  git fetch -q origin
fi
merge_status=0
git merge --no-edit --no-commit origin/main || merge_status=$?
if ! git rev-parse -q --verify MERGE_HEAD >/dev/null; then
  if [ "$merge_status" -ne 0 ]; then
    exit "$merge_status"
  fi
  echo "nothing to merge"
  exit 0
fi

# Our README.
git checkout HEAD -- README.md
base="$(git merge-base HEAD MERGE_HEAD)"
target="$(git rev-parse --short MERGE_HEAD)"
# Files main touched that this branch does not have (deleted here, or new under docs/): leave them out.
{
  git -c core.quotepath=false diff --cached --name-only --diff-filter=ACMRTU HEAD
  git -c core.quotepath=false diff --name-only --diff-filter=U
} | sort -u | while IFS= read -r f; do
  [ -n "$f" ] || continue
  if git cat-file -e "HEAD:$f" 2>/dev/null; then
    continue                                  # this branch has the file: merged normally
  fi
  case "$f" in docs/*) ;; *)
    git cat-file -e "$base:$f" 2>/dev/null || continue  # new outside docs/: take it
  esac
  if [ -n "$ours_docs" ] && printf '%s\n' "$ours_docs" | grep -Fxq -- "$f"; then
    continue
  fi
  git rm -q --cached --ignore-unmatch -- "$f"
  rm -f -- "$f"
done

if [ -n "$(git diff --name-only --diff-filter=U)" ]; then
  echo "conflicts left for a person:"
  git diff --name-only --diff-filter=U
  exit 1
fi
git commit -q --no-edit --cleanup=strip && echo "merged origin/main $target"
