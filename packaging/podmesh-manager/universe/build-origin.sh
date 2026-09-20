#!/bin/bash
# Build the origin's administration app and stage what the image needs -- the built files, the
# server and its production dependencies -- as origin.tar next to this script, for the image's
# build context on a host. Deterministic from the lockfile; the image build itself needs no
# network and no npm.
set -euo pipefail
cd "$(dirname "$0")/origin"
npm ci >/dev/null
npm run build >/dev/null
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/origin"
cp -r dist server package.json package-lock.json .npmrc "$stage/origin/"
(cd "$stage/origin" && npm ci --omit=dev >/dev/null && rm -rf node_modules/.package-lock.json)
tar -cf ../origin.tar -C "$stage" origin
echo "origin.tar: $(du -h ../origin.tar | cut -f1), dist $(ls dist/assets | wc -l) assets, $(ls "$stage/origin/node_modules" | wc -l) production packages"
