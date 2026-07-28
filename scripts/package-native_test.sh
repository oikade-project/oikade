#!/usr/bin/env bash
set -euo pipefail

repository="$(cd "$(dirname "$0")/.." && pwd -P)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/oikade-native-package-test.XXXXXX")"
cleanup() { rm -rf "$test_root"; }
trap cleanup EXIT HUP INT TERM

version="0.3.0-dev"
commit="0123456789abcdef0123456789abcdef01234567"
built_at="2026-07-27T14:00:00Z"

oikade_binary="$test_root/oikade"
cat > "$oikade_binary" <<EOF
#!/bin/sh
if [ "\${1:-}" != "version" ]; then exit 2; fi
printf '%s\n' 'oikade $version (commit $commit, built $built_at)'
EOF
chmod 0755 "$oikade_binary"

adapter_binary="$test_root/oikade-matter-adapter"
cat > "$adapter_binary" <<EOF
#!/bin/sh
if [ "\${1:-}" != "--oikade-metadata" ]; then exit 2; fi
printf '%s\n' '{"adapter_id":"oikade.matter","adapter_version":"$version","min_api_version":1,"max_api_version":1,"protocols":["matter"]}'
EOF
chmod 0755 "$adapter_binary"

package_set="$test_root/packages"
mkdir "$package_set"
for platform in linux_amd64 linux_arm64 darwin_arm64; do
  os_name="${platform%_*}"
  arch="${platform#*_}"
  "$repository/scripts/package-native.sh" \
    --binary "$oikade_binary" \
    --adapter-binary "$adapter_binary" \
    --version "$version" \
    --commit "$commit" \
    --built-at "$built_at" \
    --os "$os_name" \
    --arch "$arch" \
    --output "$package_set" >/dev/null
done

manifest="$test_root/SHA256SUMS"
"$repository/scripts/assemble-native-release.sh" \
  --input "$package_set" \
  --output "$manifest" \
  --version "$version" \
  --commit "$commit" >/dev/null
[[ "$(wc -l < "$manifest" | tr -d '[:space:]')" == "3" ]] || {
  echo "FAIL: native package manifest must contain three archives" >&2
  exit 1
}

tampered="$test_root/tampered"
cp -R "$package_set" "$tampered"
printf '%064d  %s\n' 0 "oikade_${version}_linux_amd64.tar.gz" \
  > "$tampered/oikade_${version}_linux_amd64.tar.gz.sha256"
if "$repository/scripts/assemble-native-release.sh" \
  --input "$tampered" \
  --output "$test_root/tampered-SHA256SUMS" \
  --version "$version" \
  --commit "$commit" >/dev/null 2>&1; then
  echo "FAIL: tampered package checksum was accepted" >&2
  exit 1
fi

bad_adapter="$test_root/bad-adapter"
printf '%s\n' '#!/bin/sh' 'printf "%s\n" "{}"' > "$bad_adapter"
chmod 0755 "$bad_adapter"
if "$repository/scripts/package-native.sh" \
  --binary "$oikade_binary" \
  --adapter-binary "$bad_adapter" \
  --version "$version" \
  --commit "$commit" \
  --built-at "$built_at" \
  --os linux \
  --arch amd64 \
  --output "$test_root/bad-output" >/dev/null 2>&1; then
  echo "FAIL: adapter with mismatched metadata was accepted" >&2
  exit 1
fi

adapter_link="$test_root/adapter-link"
ln -s "$adapter_binary" "$adapter_link"
if "$repository/scripts/package-native.sh" \
  --binary "$oikade_binary" \
  --adapter-binary "$adapter_link" \
  --version "$version" \
  --commit "$commit" \
  --built-at "$built_at" \
  --os linux \
  --arch amd64 \
  --output "$test_root/link-output" >/dev/null 2>&1; then
  echo "FAIL: symbolic-link adapter input was accepted" >&2
  exit 1
fi

echo "PASS: native packages contain both workspace binaries and exact build metadata"
