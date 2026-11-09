#!/usr/bin/env bash
# Copyright (C) 2026 ItsAlphaNeon
# SPDX-License-Identifier: GPL-3.0-or-later
# This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

# Works out what the release workflow is building: the channel, version and tag (docs/releasing.md), and whether there
# is anything to build. Run from a checkout with its history and tags. Reads, as GitHub Actions sets them:
#
#   GITHUB_REF, GITHUB_REF_NAME   what the run is on: a tag vMAJOR.MINOR.PATCH, or main for a nightly
#   FORCE=true                    a nightly even if main has not moved
#   DRY_RUN=true                  a rehearsal: builds but publishes nothing, so it may run on any branch
#   REHEARSE_TAG=v1.2.3           with DRY_RUN: plan as that tag, which need not exist yet
#
# Prints what it decided, and writes channel, version, tag, previous, sha and skip to $GITHUB_OUTPUT.
set -euo pipefail

ref=${GITHUB_REF:-}
name=${GITHUB_REF_NAME:-}
if [ -n "${REHEARSE_TAG:-}" ]; then
    if [ "${DRY_RUN:-}" != true ]; then
        echo "::error::A tag can only be rehearsed in a dry run: push the tag to release it"
        exit 1
    fi
    ref=refs/tags/$REHEARSE_TAG
    name=$REHEARSE_TAG
fi

sha=$(git rev-parse HEAD)
skip=false
if [[ "$ref" == refs/tags/* ]]; then
    channel=stable
    tag=$name
    if ! [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "::error::$tag is not a release tag: they are vMAJOR.MINOR.PATCH"
        exit 1
    fi
    version=${tag#v}
    workspace=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml | tr -d '\r')
    if [ "$version" != "$workspace" ]; then
        echo "::error::The tag $tag does not match the workspace version $workspace in Cargo.toml"
        exit 1
    fi
    # A release is of something that landed. A tag is easily pushed from the wrong branch, and there is no taking back
    # what installed copies have downloaded.
    if [ "${DRY_RUN:-}" != true ] && ! git merge-base --is-ancestor "$sha" origin/main; then
        echo "::error::$tag is not on main"
        exit 1
    fi
    previous=$( (git tag -l 'v*.*.*' --sort=-v:refname | grep -vx "$tag" || true) | head -n1)
else
    if [ "$ref" != refs/heads/main ] && [ "${DRY_RUN:-}" != true ]; then
        echo "::error::Nightlies are built from main (this run is on $ref)"
        exit 1
    fi
    channel=nightly
    day=$(date -u +%Y.%-m.%-d)
    n=$(( $(git tag -l "nightly-$day.*" | wc -l) + 1 ))
    version=$day.$n
    tag=nightly-$version
    previous=$(git tag -l 'nightly-*.*.*.*' --sort=-v:refname | head -n1)
    if [ -n "$previous" ] && [ "$(git rev-list -n1 "$previous")" = "$sha" ] && [ "${FORCE:-}" != true ] && [ "${DRY_RUN:-}" != true ]; then
        echo "main has not changed since $previous: no nightly"
        skip=true
    fi
fi

echo "$channel $version ($tag at $sha), after ${previous:-nothing}${DRY_RUN:+, dry run}"
{
    echo "channel=$channel"
    echo "version=$version"
    echo "tag=$tag"
    echo "previous=$previous"
    echo "sha=$sha"
    echo "skip=$skip"
} >>"${GITHUB_OUTPUT:-/dev/null}"
