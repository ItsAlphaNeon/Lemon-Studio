#!/usr/bin/env bash
# Copyright (C) 2026 ItsAlphaNeon
# SPDX-License-Identifier: GPL-3.0-or-later
# This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

# Tests the release workflow's scripts without building or publishing anything (CI runs it on Linux):
#
#   tools/package/test.sh
#
# plan.sh against a throwaway repository, and manifest.py against empty files with release names.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
plan=$here/plan.sh
manifest=$here/manifest.py
# Whichever runs: on Windows a python3 can be the shortcut to the Microsoft Store.
for python in python3 python; do
    "$python" -c '' 2>/dev/null && break
done
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fails=0

# Runs plan.sh with the given VAR=value settings; prints its outputs on one line, "failed" if it refused.
run() {
    local out=$work/out
    : >"$out"
    if (cd "$work/repo" && env -u FORCE -u DRY_RUN -u REHEARSE_TAG GITHUB_OUTPUT="$out" "$@" bash "$plan" >/dev/null 2>&1); then
        tr -d '\r' <"$out" | grep -E '^(channel|version|tag|previous|skip)=' | sed 's/^[a-z]*=//' | paste -sd' ' -
    else
        echo failed
    fi
}

expect() {
    local what=$1 want=$2
    shift 2
    local got
    got=$(run "$@")
    if [ "$got" = "$want" ]; then
        echo "ok    $what"
    else
        echo "FAIL  $what"
        echo "      want: $want"
        echo "      got:  $got"
        fails=$((fails + 1))
    fi
}

git init -q -b main "$work/repo"
cd "$work/repo"
git config user.name test
git config user.email test@example.invalid
git config commit.gpgsign false
git config tag.gpgsign false
printf '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "0.1.0"\nedition = "2024"\n\n[workspace.dependencies]\n' >Cargo.toml
git add Cargo.toml
git commit -qm one
git update-ref refs/remotes/origin/main HEAD
day=$(date -u +%Y.%-m.%-d)
main="GITHUB_REF=refs/heads/main GITHUB_REF_NAME=main"
tag() { echo "GITHUB_REF=refs/tags/$1 GITHUB_REF_NAME=$1"; }

# shellcheck disable=SC2086  # the settings are several words on purpose
{
    expect "the first nightly" "nightly $day.1 nightly-$day.1  false" $main
    expect "a nightly only from main" failed GITHUB_REF=refs/heads/other GITHUB_REF_NAME=other
    expect "a rehearsal from any branch" "nightly $day.1 nightly-$day.1  false" GITHUB_REF=refs/heads/other GITHUB_REF_NAME=other DRY_RUN=true

    git tag "nightly-$day.1"
    expect "main has not moved: no nightly" "nightly $day.2 nightly-$day.2 nightly-$day.1 true" $main
    expect "main has not moved: forced" "nightly $day.2 nightly-$day.2 nightly-$day.1 false" $main FORCE=true
    expect "main has not moved: rehearsed" "nightly $day.2 nightly-$day.2 nightly-$day.1 false" $main DRY_RUN=true

    git commit -q --allow-empty -m two
    git update-ref refs/remotes/origin/main HEAD
    expect "main moved: a second nightly today" "nightly $day.2 nightly-$day.2 nightly-$day.1 false" $main

    expect "the first release" "stable 0.1.0 v0.1.0  false" $(tag v0.1.0)
    expect "a tag must match Cargo.toml" failed $(tag v0.2.0)
    expect "alpha is not in the tag" failed $(tag v0.1.0-alpha.1)
    expect "a tag has three numbers" failed $(tag v0.1)

    git tag v0.0.9
    expect "a release after an earlier one" "stable 0.1.0 v0.1.0 v0.0.9 false" $(tag v0.1.0)
    git tag v0.1.0
    expect "the tag itself is not its previous" "stable 0.1.0 v0.1.0 v0.0.9 false" $(tag v0.1.0)
    git tag -d v0.1.0 >/dev/null

    expect "a tag rehearsed" "stable 0.1.0 v0.1.0 v0.0.9 false" $main DRY_RUN=true REHEARSE_TAG=v0.1.0
    expect "a rehearsal is only a rehearsal" failed $main REHEARSE_TAG=v0.1.0

    git switch -q -c elsewhere
    git commit -q --allow-empty -m three
    expect "a tag off main is refused" failed $(tag v0.1.0)
    expect "but can be rehearsed" "stable 0.1.0 v0.1.0 v0.0.9 false" $(tag v0.1.0) DRY_RUN=true
}

# The manifest: the app's three programs, not the plugin zips or anything else.
mkdir "$work/dist"
v=0.1.0
for f in LemonStudio.exe LemonStudio.AppImage "LemonStudio-$v-macos-aarch64.zip" \
    LemonStudioNightly.exe LemonStudioNightly.AppImage "LemonStudio-2026.1.1.1-macos-aarch64.zip" \
    "LemonStudio-Plugins-$v-windows-x86_64.zip" LICENSE; do
    : >"$work/dist/$f"
done
printf "## What's Changed\n* Mixer: a fix by @someone in https://github.com/o/r/pull/1\n* Another\n\n## New Contributors\n* @someone made their first contribution\n" >"$work/changes.md"
"$python" "$manifest" --channel stable --version "$v" --commit abc --repo o/r --tag "v$v" --notes "$work/changes.md" --dist "$work/dist" --out "$work/stable.json" >/dev/null
if "$python" - "$work/stable.json" <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
assert sorted(m["assets"]) == ["linux-x86_64", "macos-aarch64", "windows-x86_64"], m["assets"]
assert m["notes"] == ["Mixer: a fix", "Another"], m["notes"]
assert m["assets"]["linux-x86_64"]["url"] == "https://github.com/o/r/releases/download/v0.1.0/LemonStudio.AppImage"
assert m["assets"]["windows-x86_64"]["url"] == "https://github.com/o/r/releases/download/v0.1.0/LemonStudio.exe"
assert m["assets"]["macos-aarch64"]["url"] == "https://github.com/o/r/releases/download/v0.1.0/LemonStudio-0.1.0-macos-aarch64.zip"
assert m["page"] == "https://github.com/o/r/releases/tag/v0.1.0"
PY
then
    echo "ok    the manifest"
else
    echo "FAIL  the manifest"
    fails=$((fails + 1))
fi
"$python" "$manifest" --channel nightly --version 2026.1.1.1 --commit abc --repo o/r --tag nightly-2026.1.1.1 --notes "$work/changes.md" --dist "$work/dist" --out "$work/nightly.json" >/dev/null
if "$python" - "$work/nightly.json" <<'PY'
import json, sys
a = json.load(open(sys.argv[1]))["assets"]
base = "https://github.com/o/r/releases/download/nightly-2026.1.1.1/"
assert a["windows-x86_64"]["url"] == base + "LemonStudioNightly.exe", a
assert a["linux-x86_64"]["url"] == base + "LemonStudioNightly.AppImage", a
assert a["macos-aarch64"]["url"] == base + "LemonStudio-2026.1.1.1-macos-aarch64.zip", a
PY
then
    echo "ok    a nightly's programs have their own names"
else
    echo "FAIL  a nightly's programs have their own names"
    fails=$((fails + 1))
fi
echo "The first build on this channel." >"$work/first.md"
"$python" "$manifest" --channel stable --version "$v" --commit abc --repo o/r --tag "v$v" --notes "$work/first.md" --dist "$work/dist" --out "$work/first.json" >/dev/null
if "$python" -c 'import json, sys; sys.exit(json.load(open(sys.argv[1]))["notes"] != [])' "$work/first.json"; then
    echo "ok    the first build lists no changes"
else
    echo "FAIL  the first build lists no changes"
    fails=$((fails + 1))
fi

if [ "$fails" -ne 0 ]; then
    echo "$fails failed"
    exit 1
fi
echo "all passed"
