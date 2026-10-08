## Description

<!-- Briefly describe the purpose of this change and the problem it solves. -->

## Type of Change

- [ ] Bug fix (non-breaking change which fixes an issue)
- [ ] New feature or widget (non-breaking change which adds functionality)
- [ ] Refactor or code cleanup
- [ ] Documentation update
- [ ] CI/CD or tooling enhancement

## Quality Checklist

- [ ] `npm test` passes (Vitest unit tests)
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml` passes (backend tests)
- [ ] `cargo fmt --manifest-path src-tauri/Cargo.toml --check` passes
- [ ] `npm run check:plugins` and `npx tsc --noEmit` pass
- [ ] `npm run build` succeeds cleanly
- [ ] Does not perform destructive operations on real desktop files without user confirmation
- [ ] Updated documentation or changelog if applicable
