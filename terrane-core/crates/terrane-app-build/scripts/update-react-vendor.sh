#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
usage: update-react-vendor.sh [react-version]

Updates terrane-app-build/vendor/react from official React npm package files.

Default mode downloads package tarballs from the npm registry:
  scripts/update-react-vendor.sh 18.3.1

Offline/test mode copies from unpacked package directories:
  REACT_PACKAGE_DIR=/path/to/react \
  REACT_DOM_PACKAGE_DIR=/path/to/react-dom \
    scripts/update-react-vendor.sh 18.3.1
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

version="${1:-${REACT_VERSION:-18.3.1}}"
case "$version" in
  ""|*[!0-9A-Za-z._-]*)
    echo "bad React version: $version" >&2
    exit 2
    ;;
esac

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
crate_dir="$(cd "$script_dir/.." && pwd)"
vendor_dir="$crate_dir/vendor/react"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/terrane-react-vendor.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

stage_dir="$tmp_dir/react"
mkdir -p "$stage_dir"

copy_from_package_dir() {
  local package_dir="$1"
  local package_name="$2"
  local source_rel="$3"
  local output_name="$4"
  local source_path="$package_dir/$source_rel"

  if [[ ! -f "$source_path" ]]; then
    echo "$package_name package is missing $source_rel at $source_path" >&2
    exit 1
  fi

  cp "$source_path" "$stage_dir/$output_name"
}

copy_from_tarball() {
  local package_name="$1"
  local source_rel="$2"
  local output_name="$3"
  local tarball="$tmp_dir/$package_name.tgz"
  local unpack_dir="$tmp_dir/$package_name"

  curl -fsSL "https://registry.npmjs.org/$package_name/-/$package_name-$version.tgz" -o "$tarball"
  mkdir -p "$unpack_dir"
  tar -xzf "$tarball" -C "$unpack_dir"

  copy_from_package_dir "$unpack_dir/package" "$package_name" "$source_rel" "$output_name"
}

copy_package_file() {
  local package_env="$1"
  local package_name="$2"
  local source_rel="$3"
  local output_name="$4"
  local package_dir="${!package_env:-}"

  if [[ -n "$package_dir" ]]; then
    copy_from_package_dir "$package_dir" "$package_name" "$source_rel" "$output_name"
  else
    copy_from_tarball "$package_name" "$source_rel" "$output_name"
  fi
}

copy_package_file REACT_PACKAGE_DIR react umd/react.production.min.js react.production.min.js
copy_package_file REACT_PACKAGE_DIR react LICENSE LICENSE.react.txt
copy_package_file REACT_DOM_PACKAGE_DIR react-dom umd/react-dom.production.min.js react-dom.production.min.js
copy_package_file REACT_DOM_PACKAGE_DIR react-dom LICENSE LICENSE.react-dom.txt
printf '%s\n' "$version" > "$stage_dir/VERSION"

for required in \
  react.production.min.js \
  react-dom.production.min.js \
  LICENSE.react.txt \
  LICENSE.react-dom.txt \
  VERSION
do
  if [[ ! -s "$stage_dir/$required" ]]; then
    echo "vendor update produced empty or missing file: $required" >&2
    exit 1
  fi
done

rm -rf "$vendor_dir"
mv "$stage_dir" "$vendor_dir"
echo "updated $vendor_dir to React $version"
