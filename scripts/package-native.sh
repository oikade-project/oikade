#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<EOF
usage: $0 --binary PATH --adapter-binary PATH --version VERSION --commit SHA \
  --built-at RFC3339 --os NAME --arch NAME --output PATH
EOF
  exit 2
}

binary=""
adapter_binary=""
version=""
commit=""
built_at=""
target_os=""
target_arch=""
output=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary) binary="${2:-}"; shift 2 ;;
    --adapter-binary) adapter_binary="${2:-}"; shift 2 ;;
    --version) version="${2:-}"; shift 2 ;;
    --commit) commit="${2:-}"; shift 2 ;;
    --built-at) built_at="${2:-}"; shift 2 ;;
    --os) target_os="${2:-}"; shift 2 ;;
    --arch) target_arch="${2:-}"; shift 2 ;;
    --output) output="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done

[[ -f "$binary" && -x "$binary" && ! -L "$binary" ]] || usage
[[ -f "$adapter_binary" && -x "$adapter_binary" && ! -L "$adapter_binary" ]] || usage
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || usage
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || usage
[[ "$target_os" =~ ^[a-z0-9]+$ && "$target_arch" =~ ^[a-z0-9]+$ ]] || usage
[[ -n "$output" ]] || usage

repository="$(cd "$(dirname "$0")/.." && pwd -P)"
mkdir -p "$output"
output="$(cd "$output" && pwd -P)"

python3 - "$built_at" <<'PY'
import datetime
import sys

try:
    value = datetime.datetime.fromisoformat(sys.argv[1].replace("Z", "+00:00"))
except ValueError as error:
    raise SystemExit(f"package-native: --built-at must be RFC3339: {error}")
if value.tzinfo is None:
    raise SystemExit("package-native: --built-at must include a timezone")
PY

expected_version="oikade $version (commit $commit, built $built_at)"
actual_version="$("$binary" version)"
[[ "$actual_version" == "$expected_version" ]] || {
  echo "package-native: binary build metadata does not match package inputs" >&2
  exit 1
}

adapter_metadata="$("$adapter_binary" --oikade-metadata)"
python3 - "$version" "$adapter_metadata" <<'PY'
import json
import sys

try:
    metadata = json.loads(sys.argv[2])
except json.JSONDecodeError as error:
    raise SystemExit(f"package-native: invalid Matter adapter metadata: {error}")
expected = {
    "adapter_id": "oikade.matter",
    "adapter_version": sys.argv[1],
    "min_api_version": 1,
    "max_api_version": 1,
    "protocols": ["matter"],
}
if metadata != expected:
    raise SystemExit(f"package-native: Matter adapter metadata mismatch: {metadata!r}")
PY

if command -v sha256sum >/dev/null 2>&1; then
  adapter_sha256="$(sha256sum "$adapter_binary" | awk '{print $1}')"
else
  adapter_sha256="$(shasum -a 256 "$adapter_binary" | awk '{print $1}')"
fi

artifact="oikade_${version}_${target_os}_${target_arch}"
archive="$output/$artifact.tar.gz"
checksum="$archive.sha256"
[[ ! -e "$archive" && ! -L "$archive" && ! -e "$checksum" && ! -L "$checksum" ]] || {
  echo "package-native: refusing to replace an existing package" >&2
  exit 1
}

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/oikade-native-package.XXXXXX")"
cleanup() { rm -rf "$work_dir"; }
trap cleanup EXIT HUP INT TERM

bundle="$work_dir/$artifact"
mkdir -p "$bundle/bin" "$bundle/libexec/oikade" "$bundle/share/doc/oikade"
install -m 0755 "$binary" "$bundle/bin/oikade"
install -m 0755 "$adapter_binary" "$bundle/libexec/oikade/oikade-matter-adapter"
install -m 0644 "$repository/LICENSE" "$bundle/share/doc/oikade/LICENSE"
install -m 0644 "$repository/NOTICE" "$bundle/share/doc/oikade/NOTICE"
install -m 0644 "$repository/TRADEMARKS.md" "$bundle/share/doc/oikade/TRADEMARKS.md"
install -m 0644 "$repository/README.md" "$bundle/share/doc/oikade/README.md"
printf '%s\n' "$adapter_metadata" > "$bundle/share/doc/oikade/adapter-metadata.json"

python3 - \
  "$bundle/share/doc/oikade/release-metadata.json" \
  "$version" "$commit" "$built_at" "$target_os" "$target_arch" \
  "$adapter_sha256" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
metadata = {
    "schema_version": 2,
    "version": sys.argv[2],
    "commit": sys.argv[3],
    "built_at": sys.argv[4],
    "os": sys.argv[5],
    "arch": sys.argv[6],
    "matter_adapter": {
        "adapter_id": "oikade.matter",
        "version": sys.argv[2],
        "binary_sha256": sys.argv[7],
    },
}
path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY

find "$bundle" -exec touch -t 197001010000 {} +
tarfile="$output/$artifact.tar"
if tar --version 2>/dev/null | grep -q 'GNU tar'; then
  tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner \
    -C "$work_dir" -cf "$tarfile" "$artifact"
else
  COPYFILE_DISABLE=1 tar -C "$work_dir" -cf "$tarfile" "$artifact"
fi
gzip -n -f "$tarfile"
if command -v sha256sum >/dev/null 2>&1; then
  digest="$(sha256sum "$archive" | awk '{print $1}')"
else
  digest="$(shasum -a 256 "$archive" | awk '{print $1}')"
fi
printf '%s  %s\n' "$digest" "$(basename "$archive")" > "$checksum"
printf '%s\n' "$archive"
