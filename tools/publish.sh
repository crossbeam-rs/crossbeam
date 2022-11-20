#!/bin/bash
set -euo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

# Publish a new release.
#
# USAGE:
#    ./tools/publish.sh <CRATE> <VERSION>

bail() {
    echo >&2 "error: $*"
    exit 1
}

crate="${1:?}"
version="${2:?}"
version="${version#v}"
tag="${crate}-${version}"
if [[ ! "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z\.-]+)?(\+[0-9A-Za-z\.-]+)?$ ]]; then
    bail "invalid version format '${version}'"
fi
if [[ $# -gt 2 ]]; then
    bail "invalid argument '$3'"
fi

# Make sure there is no uncommitted change.
git diff --exit-code
git diff --exit-code --staged

# Make sure the same release has not been created in the past.
if gh release view "${tag}" &>/dev/null; then
    bail "tag '${tag}' has already been created and pushed"
fi

if ! git branch | grep -q '\* master'; then
    bail "current branch is not 'master'"
fi

git tag "${tag}"

(
    if [[ "${crate}" != "crossbeam" ]]; then
        cd "${crate}"
    fi
    cargo +stable publish
)

git push origin --tags
