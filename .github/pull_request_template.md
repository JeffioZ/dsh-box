## What changed

<!-- Describe the user-visible outcome and why it belongs in the desktop shell. -->

## Validation

- [ ] `npm run check`
- [ ] `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`
- [ ] `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml --all-targets`
- [ ] Light/dark themes and reduced motion checked when UI changed

## Risks and recovery

<!-- Call out daily-use regressions, cross-platform gaps, file/update behavior, and rollback. -->

- [ ] No credentials, private sessions, logs, or generated build artifacts are included
