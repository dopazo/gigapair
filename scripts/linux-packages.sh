#!/usr/bin/env bash
# Builds the .deb and .rpm from the Linux archive that dist produced.
# Needs nfpm (https://nfpm.goreleaser.com) on the PATH.
#
#   scripts/linux-packages.sh <archive.tar.xz> <version> <output dir>
#
# Run it from anywhere; paths in packaging/linux/nfpm.yaml are relative to the
# repository root.
set -euo pipefail

archive=${1:?usage: linux-packages.sh <archive.tar.xz> <version> <output dir>}
version=${2:?missing version}
out=${3:?missing output dir}

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

tar -xJf "$archive" -C "$work"
stage=$(dirname "$(find "$work" -name gigapair-app -type f | head -n 1)")
[ -f "$stage/gigapaird" ] || { echo "binaries not found in $archive" >&2; exit 1; }

mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$root"
# nfpm does not expand variables in `contents`, so fill in the folder here.
sed "s#\${STAGE}#$stage#g" packaging/linux/nfpm.yaml > "$work/nfpm.yaml"
export VERSION=$version
for fmt in deb rpm; do
  nfpm package --config "$work/nfpm.yaml" --packager "$fmt" --target "$out/gigapair-x86_64-unknown-linux-gnu.$fmt"
done
ls -l "$out"
