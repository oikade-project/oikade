#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 --input PATH --output PATH --version VERSION --commit SHA" >&2
  exit 2
}

input=""
output=""
version=""
commit=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --input) input="${2:-}"; shift 2 ;;
    --output) output="${2:-}"; shift 2 ;;
    --version) version="${2:-}"; shift 2 ;;
    --commit) commit="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done

[[ -d "$input" && ! -L "$input" ]] || usage
[[ "$output" == /* && ! -e "$output" && ! -L "$output" ]] || usage
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || usage
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || usage

input="$(cd "$input" && pwd -P)"

python3 - \
  "$input" "$output" "$version" "$commit" <<'PY'
import hashlib
import datetime
import json
import pathlib
import re
import sys
import tarfile

source = pathlib.Path(sys.argv[1])
output = pathlib.Path(sys.argv[2])
version = sys.argv[3]
commit = sys.argv[4]
platforms = (("linux", "amd64"), ("linux", "arm64"), ("darwin", "arm64"))
archives = [f"oikade_{version}_{os_name}_{arch}.tar.gz" for os_name, arch in platforms]
expected_names = {name for archive in archives for name in (archive, f"{archive}.sha256")}

entries = list(source.iterdir())
actual_names = {entry.name for entry in entries}
if len(entries) != len(expected_names) or actual_names != expected_names:
    missing = sorted(expected_names - actual_names)
    unexpected = sorted(actual_names - expected_names)
    raise SystemExit(
        "assemble-native-release: package set is incomplete or unexpected "
        f"(missing={missing}, unexpected={unexpected})"
    )
if any(not entry.is_file() or entry.is_symlink() for entry in entries):
    raise SystemExit("assemble-native-release: every input must be a regular file")

manifest = []
built_at = None
for (os_name, arch), archive_name in zip(platforms, archives):
    archive = source / archive_name
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    checksum_text = (source / f"{archive_name}.sha256").read_text(encoding="utf-8")
    match = re.fullmatch(r"([0-9a-f]{64})  ([^\n]+)\n", checksum_text)
    if match is None or match.group(1) != digest or match.group(2) != archive_name:
        raise SystemExit(f"assemble-native-release: invalid checksum for {archive_name}")

    root = archive_name.removesuffix(".tar.gz")
    required_files = {
        "bin/oikade",
        "libexec/oikade/oikade-matter-adapter",
        "share/doc/oikade/LICENSE",
        "share/doc/oikade/NOTICE",
        "share/doc/oikade/README.md",
        "share/doc/oikade/TRADEMARKS.md",
        "share/doc/oikade/adapter-metadata.json",
        "share/doc/oikade/release-metadata.json",
    }
    required_directories = {
        "",
        "bin",
        "libexec",
        "libexec/oikade",
        "share",
        "share/doc",
        "share/doc/oikade",
    }
    try:
        bundle = tarfile.open(archive, mode="r:gz")
    except (OSError, tarfile.TarError) as error:
        raise SystemExit(f"assemble-native-release: invalid archive {archive_name}: {error}")
    with bundle:
        members = bundle.getmembers()
        by_name = {member.name.rstrip("/"): member for member in members}
        if len(by_name) != len(members):
            raise SystemExit(f"assemble-native-release: duplicate members in {archive_name}")
        expected_members = {
            root if relative == "" else f"{root}/{relative}"
            for relative in required_files | required_directories
        }
        if set(by_name) != expected_members:
            raise SystemExit(f"assemble-native-release: unexpected archive shape in {archive_name}")
        for relative in required_files:
            member = by_name.get(f"{root}/{relative}")
            if member is None or not member.isfile():
                raise SystemExit(f"assemble-native-release: missing regular {relative} in {archive_name}")
        for relative in required_directories:
            name = root if relative == "" else f"{root}/{relative}"
            if not by_name[name].isdir():
                raise SystemExit(f"assemble-native-release: {relative or 'root'} is not a directory in {archive_name}")
        for relative in ("bin/oikade", "libexec/oikade/oikade-matter-adapter"):
            if by_name[f"{root}/{relative}"].mode & 0o111 == 0:
                raise SystemExit(f"assemble-native-release: {relative} is not executable in {archive_name}")
        if any(member.issym() or member.islnk() or member.isdev() for member in members):
            raise SystemExit(f"assemble-native-release: unsafe member type in {archive_name}")

        metadata_member = by_name[f"{root}/share/doc/oikade/release-metadata.json"]
        metadata_file = bundle.extractfile(metadata_member)
        if metadata_file is None:
            raise SystemExit(f"assemble-native-release: unreadable metadata in {archive_name}")
        try:
            metadata = json.load(metadata_file)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise SystemExit(f"assemble-native-release: invalid metadata in {archive_name}: {error}")
        expected_fields = {
            "schema_version": 2,
            "version": version,
            "commit": commit,
            "os": os_name,
            "arch": arch,
        }
        if set(metadata) != {*expected_fields, "built_at", "matter_adapter"} or any(
            metadata.get(key) != value for key, value in expected_fields.items()
        ):
            raise SystemExit(f"assemble-native-release: metadata mismatch in {archive_name}")
        adapter = metadata.get("matter_adapter")
        if not isinstance(adapter, dict) or set(adapter) != {
            "adapter_id",
            "version",
            "binary_sha256",
        } or adapter.get("adapter_id") != "oikade.matter":
            raise SystemExit(f"assemble-native-release: adapter identity mismatch in {archive_name}")
        if adapter.get("version") != version:
            raise SystemExit(f"assemble-native-release: adapter version mismatch in {archive_name}")
        if not re.fullmatch(r"[0-9a-f]{64}", str(adapter.get("binary_sha256", ""))):
            raise SystemExit(f"assemble-native-release: adapter digest missing in {archive_name}")
        adapter_binary_member = by_name[f"{root}/libexec/oikade/oikade-matter-adapter"]
        adapter_binary_file = bundle.extractfile(adapter_binary_member)
        if adapter_binary_file is None:
            raise SystemExit(f"assemble-native-release: unreadable adapter binary in {archive_name}")
        if hashlib.sha256(adapter_binary_file.read()).hexdigest() != adapter["binary_sha256"]:
            raise SystemExit(f"assemble-native-release: adapter binary digest mismatch in {archive_name}")
        adapter_metadata_member = by_name[f"{root}/share/doc/oikade/adapter-metadata.json"]
        adapter_metadata_file = bundle.extractfile(adapter_metadata_member)
        if adapter_metadata_file is None:
            raise SystemExit(f"assemble-native-release: unreadable adapter metadata in {archive_name}")
        adapter_metadata = json.load(adapter_metadata_file)
        if adapter_metadata != {
            "adapter_id": "oikade.matter",
            "adapter_version": version,
            "min_api_version": 1,
            "max_api_version": 1,
            "protocols": ["matter"],
        }:
            raise SystemExit(f"assemble-native-release: embedded adapter metadata mismatch in {archive_name}")
        candidate_built_at = metadata.get("built_at")
        if not isinstance(candidate_built_at, str):
            raise SystemExit(f"assemble-native-release: build timestamp missing in {archive_name}")
        try:
            parsed_built_at = datetime.datetime.fromisoformat(
                candidate_built_at.replace("Z", "+00:00")
            )
        except ValueError as error:
            raise SystemExit(
                f"assemble-native-release: invalid build timestamp in {archive_name}: {error}"
            )
        if parsed_built_at.tzinfo is None:
            raise SystemExit(f"assemble-native-release: build timestamp has no timezone in {archive_name}")
        if built_at is None:
            built_at = candidate_built_at
        elif candidate_built_at != built_at:
            raise SystemExit("assemble-native-release: platform build timestamps differ")

    manifest.append(f"{digest}  {archive_name}\n")

output.parent.mkdir(parents=True, exist_ok=True)
with output.open("x", encoding="ascii", newline="") as destination:
    destination.writelines(manifest)
PY

printf '%s\n' "$output"
