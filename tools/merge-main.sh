#!/bin/sh
# Merges main into the cutting-edge branch.
#
# This branch keeps its own README.md, and under docs/ only its own files: whatever main changes in those, or adds
# to docs/, is left out. A file this branch deleted stays deleted. Any other conflict is left for a person.
set -e
cd "$(git rev-parse --show-toplevel)"
[ "$(git branch --show-current)" = cutting-edge ] || { echo "run this on cutting-edge"; exit 1; }
git config merge.ours.driver true
ours_docs=" $(git ls-files docs | tr '\n' ' ') "
git fetch -q origin
git merge --no-edit --no-commit origin/main || true
git rev-parse -q --verify MERGE_HEAD >/dev/null || { echo "nothing to merge"; exit 0; }

# Our README.
git checkout HEAD -- README.md
# Files main touched that this branch does not have (deleted here, or new under docs/): leave them out.
for f in $(git diff --cached --name-only --diff-filter=ACMRTU HEAD; git diff --name-only --diff-filter=U); do
  if git cat-file -e "HEAD:$f" 2>/dev/null; then
    continue                                  # this branch has the file: merged normally
  fi
  case "$f" in docs/*) ;; *)
    git cat-file -e "$(git merge-base HEAD MERGE_HEAD):$f" 2>/dev/null || continue  # new outside docs/: take it
  esac
  case "$ours_docs" in *" $f "*) continue ;; esac
  git rm -q --cached --ignore-unmatch -- "$f"; rm -f -- "$f"
done

if [ -n "$(git diff --name-only --diff-filter=U)" ]; then
  echo "conflicts left for a person:"; git diff --name-only --diff-filter=U; exit 1
fi
git commit -q --no-edit --cleanup=strip && echo "merged origin/main $(git rev-parse --short MERGE_HEAD 2>/dev/null || git rev-parse --short origin/main)"
