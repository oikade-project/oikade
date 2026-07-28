# Native packaging

Oikade and its Matter sidecar are two executables in one Cargo workspace. A
native release builds both from the same source commit, Rust toolchain and
`Cargo.lock`, then places them in one attested archive. Users do not install a
separate adapter prerequisite and the daemon never downloads executable code.

## Build and package boundary

The release workflow builds:

```sh
cargo build --locked --release -p oikade -p oikade-matter-adapter
```

`scripts/package-native.sh` accepts those two regular executable files plus the
release version, commit, timestamp and platform identity. It rejects symbolic
links and existing output archives, verifies the daemon's embedded build
identity, and requires the sidecar's side-effect-free `--oikade-metadata`
response to report:

- adapter ID `oikade.matter`;
- the same version as the Oikade release;
- support for adapter API version 1;
- protocol `matter`.

The metadata probe does not receive an RPC descriptor or state directory and
therefore cannot access Matter state.

The resulting layout is:

```text
oikade_VERSION_OS_ARCH/
├── bin/oikade
├── libexec/oikade/oikade-matter-adapter
└── share/doc/oikade/
    ├── LICENSE
    ├── NOTICE
    ├── README.md
    ├── TRADEMARKS.md
    ├── adapter-metadata.json
    └── release-metadata.json
```

The sidecar binary's SHA-256 is recorded in `release-metadata.json`. Release
assembly reopens every platform archive, validates the exact member set and
types, recomputes the embedded sidecar digest, and checks both metadata files.
The runtime resolves only an explicitly configured path, a sidecar adjacent to
the daemon, or the fixed `libexec/oikade` location; it never searches `PATH`.

## Provenance

Tagged builds produce Linux x64, Linux ARM64 and macOS ARM64 archives. Each
archive and the consolidated `SHA256SUMS` manifest receives GitHub/Sigstore
build provenance from the Oikade release workflow. Because both executables
come from one repository and build job, there is no secondary adapter release,
download, checksum pin or signer chain to trust.

Users can verify a package with:

```sh
gh attestation verify oikade_VERSION_OS_ARCH.tar.gz \
  --repo oikade-project/oikade \
  --signer-workflow oikade-project/oikade/.github/workflows/release.yml \
  --source-ref refs/tags/vVERSION \
  --deny-self-hosted-runners
```

Run the packaging contract and adversarial-input checks with:

```sh
./scripts/package-native_test.sh
```
