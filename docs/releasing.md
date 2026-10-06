# Release checklist

The [release workflow](../.github/workflows/release.yml) runs on pushes of tags matching `v*`. It builds and publishes the x86_64 GitHub release assets, but it does not write release notes or publish to crates.io.

1. Prepare the source in a `chore: prepare vX.Y.Z` commit. Bump `version` in `Cargo.toml` and the `gpuflo` entry in `Cargo.lock`; replace `Unreleased` with the tag date in the `CHANGELOG.md` entry. This **prepare commit is the validated commit**. Run the release checks against it: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --all-targets --locked`, `python3 scripts/notices.py` (and check `git diff --exit-code THIRD_PARTY_NOTICES.txt`), `cargo package --locked --allow-dirty`, `cargo +1.96 build --all-targets --locked`, `cargo +1.96 test --lib --locked`, and `cargo build --release --locked --target x86_64-unknown-linux-gnu`. Record the hardware qualification evidence.
2. In its **own commit**, write `validation/X.Y.Z.md` for the validated source. Include these exact lines (substitute the actual version, the prepare commit's full 40-character lowercase hex SHA, and the archive binary's maximum required GLIBC symbol version):

   ```text
   Release authorization: qualified
   - Version: X.Y.Z
   - Commit: <40-hex>
   - Maximum required GLIBC: GLIBC_x.y
   ```

   The workflow checks the authorization and version with exact-line `grep`, extracts the 40-hex commit with `sed`, and checks the GLIBC line with exact-line `grep` against the built binary. The tagged tree may differ from the validated commit only under `validation/`; the workflow checks `git diff --quiet <validated-commit> <tagged-commit> -- . ':(exclude)validation'`. Do not change source, changelog, or version after validation without repeating it.
3. Create and push an **annotated** `vX.Y.Z` tag on the manifest commit. The `v*` push starts the workflow. It checks that the tag equals `v` plus the `Cargo.toml` package version, that `validation/X.Y.Z.md` exists, and that the tagged tree matches the validated source as above.
4. Check the workflow result. It runs format, clippy, tests, notices, package, and MSRV gates on the tag, builds the release binary, verifies `--version` and archive contents, and publishes `gpuflo-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` (binary, `LICENSE`, `THIRD_PARTY_NOTICES.txt`, and `README.md`), `SHA256SUMS`, `GLIBC_BASELINE.txt`, and `validation/*.md` to the GitHub release.
5. **Paste the dated CHANGELOG entry into the GitHub release body by hand.** The workflow's `softprops/action-gh-release` step sets files only, not a body or `body_path`.
6. **Run `cargo publish --locked` by hand from the tag** with the maintainer's crates.io credentials. Publication is not automated; the workflow has only a commented-out `cargo publish --locked`.
7. After release, check the published archive with `install.sh` and verify `cargo install gpuflo --locked` from crates.io.
