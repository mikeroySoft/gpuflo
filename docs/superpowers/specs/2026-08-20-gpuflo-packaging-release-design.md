# Gpuflo Packaging and Release Channel Design

**Status:** Approved in grilling on 2026-08-20

## Purpose

Gpuflo ships as one small local Linux program and one narrow reusable Rust library. Packaging must preserve that shape rather than create installers, services, feature matrices, or platform promises that the product does not need.

The initial release uses two channels only:

1. a prebuilt binary archive attached to a GitHub Release; and
2. the same Cargo package published to crates.io.

The GitHub archive serves users who want one binary without a Rust toolchain. crates.io supports both `cargo install gpuflo` and consumers of the semver-supported canonical model and `Monitor` interface.

## Version identity

One SemVer version identifies the Cargo package, library, binary, and release artifacts.

For version `X.Y.Z`:

- the immutable source tag is `vX.Y.Z`;
- `Cargo.toml` declares `X.Y.Z`;
- `gpuflo --version` reports `X.Y.Z`;
- crates.io publishes `X.Y.Z`;
- the GitHub Release is `vX.Y.Z`;
- archive names contain `vX.Y.Z`; and
- the validation manifest names the same tag and commit.

The machine-output `schema_version` remains its own integer compatibility major and does not mirror package SemVer. A machine-schema breaking change requires its approved schema-major change and an appropriate package SemVer change.

Pre-release versions such as `v0.2.0-rc.1` are used only when a distributable candidate is needed for hardware qualification. There is no nightly channel, rolling channel, or fixed release cadence.

## GitHub Release archive

The initial prebuilt target is:

```text
x86_64-unknown-linux-gnu
```

Its archive is:

```text
gpuflo-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
```

The archive root contains exactly the useful distributable files:

```text
gpuflo
LICENSE
THIRD_PARTY_NOTICES.txt
README.md
```

The binary is built from the release tag by GitHub Actions after the approved deterministic release gate passes. The archive must satisfy the validation contract and must not contain:

- ROCm or AMD SMI libraries;
- an installer or self-update program;
- a default configuration file;
- a service unit, daemon, socket, or desktop entry;
- shell-profile modifications;
- device rules or permission scripts; or
- debug symbols unless separately published for a concrete diagnostic need.

A `SHA256SUMS` file beside the archive records its SHA-256 digest. GitHub workflow provenance is retained. Initial releases do not introduce GPG key management, a custom signing service, duplicate checksum formats, or a separate provenance system.

The implementation records the oldest glibc baseline actually supported by the release build. That baseline is selected from the final dependency/toolchain and CI build environment, then verified before publication; this specification does not invent a version in advance.

## crates.io publication

The repository remains one Cargo package named `gpuflo` with library and binary targets. The published crate contains the source and metadata required for:

```text
cargo install gpuflo
```

and for Rust dependency reuse of the supported library interface.

There is no initial Cargo feature matrix:

- the package builds the library and binary together;
- the TUI is not split behind a feature;
- AMD SMI remains optional through runtime loading, not a Cargo feature;
- no ROCm SDK is linked or vendored; and
- no `minimal`, `tui`, `amdsmi`, `vendored`, or backend features are promised.

A feature is added only when a real consumer cannot use the package cleanly without it. Publishing must not split the package, widen the public interface, or expose source/reducer/UI internals.

The crate metadata includes the repository, license, README, categories/keywords kept to the obvious minimum, and the minimum supported Rust version once implementation dependencies establish it. The release gate tests that MSRV and current stable Rust as required by the validation contract.

## Host prerequisites

Installation only places package files. Neither channel may configure the host.

The minimum runtime remains:

- Linux; and
- at least one AMD PCI/DRM device bound to `amdgpu`.

The package does not require ROCm userspace, the `amd-smi` executable, the AMD SMI library, `/dev/kfd`, root, network access, or membership in `render` or `video` to start with kernel-backed telemetry.

Documentation may explain that optional AMD SMI enrichment and process attribution depend on sources and permissions already described by the capability contract. It must not instruct an installer to:

- install ROCm automatically;
- add users to groups;
- create or replace udev rules;
- load kernel modules;
- change device permissions; or
- tune or mutate GPU state.

## Target policy

Only a target that passes live hardware qualification may be described as supported or receive a prebuilt binary.

Initial policy:

| Target | Initial status | Distribution |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | Supported after required live qualification | GitHub archive and crates.io source build |
| `aarch64-unknown-linux-gnu` | Compile-check when CI can do so without target stubs | No prebuilt artifact until live qualification |
| musl targets | Unverified | Not published until AMD SMI loading and host integration are proven |
| Windows, WSL-specific, macOS, non-amdgpu | Out of scope | None |

Cross-compilation proves compilation only; it is not hardware qualification. Adding a target later is an additive packaging change when it satisfies the same release contract.

## License and provenance artifacts

Every distribution includes the project `LICENSE` and generated `THIRD_PARTY_NOTICES.txt`.

The notices are generated from the locked Cargo dependency graph during release and checked against a small accepted-license policy. The repository does not add an xtask crate, Hawkeye, or a general compliance framework solely for this task; a direct release command or focused CI step is sufficient.

Copied or closely translated flow and rocm-cli implementations retain auditable per-file provenance and the exact upstream copyright/license notices approved by the reuse decision. The notices identify the upstream URL and pinned revision. Inspiration or general design ideas do not receive false copied-code claims.

The release does not use AMD, ROCm, flow, btop, or another upstream project as gpuflo branding, and it makes no trademark claim based on an open-source license.

## Documentation artifacts

The initial release documentation surface is deliberately small:

- `README.md` covers installation from the archive and crates.io, runtime prerequisites, first invocation, output modes, optional enrichment, and uninstallation;
- `gpuflo --help` is the authoritative CLI reference; and
- `gpuflo --version` provides release identity.

The first release does not generate manual pages or Bash/Zsh/Fish completions. The fixed CLI is small enough that those artifacts would add more release machinery than user value. They may be added after the flag surface stabilizes and demonstrated demand exists.

There is no install shell script, `curl | sh` path, self-update command, update notification, or network version check. Users update through `cargo install` or by replacing the release binary.

## Release sequence

A release is published from one immutable tag:

1. choose the SemVer version and update package metadata;
2. run the deterministic release gate;
3. produce the validation manifest and required hardware qualification evidence;
4. generate dependency notices;
5. create the immutable `vX.Y.Z` tag;
6. build the target archive from that tag;
7. verify archive contents, binary version, runtime dependency expectations, and checksum;
8. publish the crate with the same version;
9. publish the GitHub Release, archive, `SHA256SUMS`, notices-bearing artifact, and validation evidence; and
10. smoke-install once from the published crate and once from the downloaded archive.

If either channel fails after the immutable crate version is published, fix the release process and publish a new SemVer patch. Do not replace crates.io contents or silently move a release tag.

## Distro packaging policy

No distro package is maintained initially: no `.deb`, `.rpm`, PPA, COPR, AUR package, Homebrew formula, Nix package, Flatpak, Snap, AppImage, or container image.

Third-party distro maintainers may package gpuflo from tagged source under its license. An official distro channel is added only when there is a maintainer, a tested target, and a clear user need. It must consume the same package and must not introduce a daemon, mandatory ROCm dependency, permission mutation, or alternate product behavior.

## Explicit non-goals

Initial packaging excludes:

- multi-package or workspace publication;
- platform-specific installers;
- bundled ROCm or AMD SMI components;
- automatic permission or driver configuration;
- Cargo backend/feature combinations;
- static/musl claims without qualification;
- unqualified AArch64 binaries;
- distro repository maintenance;
- containers, services, daemons, or desktop launchers;
- manual pages and shell completions;
- self-update and network version checks;
- GPG infrastructure or custom artifact signing; and
- replacing the concise validation manifest with release automation machinery.

## Acceptance criteria

The packaging decision is satisfied when:

- one version identifies source, crate, binary, archive, and validation evidence;
- the GitHub archive gives an x86-64 Linux user one binary plus required license/readme artifacts;
- crates.io supports `cargo install` and the approved library reuse seam from the same package;
- installation never requires or mutates ROCm, permissions, drivers, services, network, or GPU state;
- only live-qualified targets receive support claims and prebuilt binaries;
- every archive has SHA-256 verification and workflow provenance;
- upstream and dependency notices are complete and auditable;
- no optional-feature, distro, installer, completion, man-page, or update machinery is added without demonstrated need; and
- the separate validation contract gates every publication.

## Evidence

- [Choose the minimal Rust architecture](./2026-08-20-gpuflo-rust-architecture-design.md)
- [Define the validation and release contract](./2026-08-20-gpuflo-validation-release-design.md)
- [Identify the reusable code boundary](https://github.com/mikeroysoft/gpuflo/issues/2)
- [Reuse boundary research](https://github.com/mikeroysoft/gpuflo/blob/research/reuse-boundary/research/reuse-boundary.md)
- [Define capability, failure, and permission behavior](./2026-08-20-gpuflo-capability-failure-design.md)
