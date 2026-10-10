# Contributing to Floaty

Thank you for your interest in contributing to Floaty! We welcome contributions, whether they are bug reports, documentation improvements, new widgets/plugins, or code enhancements.

Floaty is built with a focus on reliability, trust, and honesty: because Floaty interacts directly with the user's desktop files and Windows shell, every change must respect user data and desktop integrity.

---

## Code of Conduct

This project adheres to the Contributor Covenant Code of Conduct. By participating, you are expected to uphold this code. Please read [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) for details.

---

## Development Setup

### Prerequisites

- **Operating System:** Windows 10 or 11 (Win32 APIs, WebView2, and WASAPI loopback are required).
- **Node.js:** Node 20 or higher.
- **Rust:** Stable toolchain (`rustup default stable`).
- **C++ Build Tools:** Visual Studio C++ Build Tools (for MSVC targets).

### Initial Setup

1. Fork and clone the repository:
   ```powershell
   git clone https://github.com/erictan2001/floaty.git
   cd floaty
   ```

2. Install dependencies:
   ```powershell
   npm install
   ```

3. Run in development mode:
   ```powershell
   npm run tauri dev
   ```

`tauri dev` will start Vite on `http://localhost:1420` with HMR and launch a debug build of Floaty.

---

## Testing & Quality Verification

Before submitting any Pull Request, ensure all automated quality gates pass:

```powershell
# 1. Run frontend lint, formatting, plugin integrity, and type checks
npm run lint

# 2. Run TypeScript unit tests (Vitest)
npm test

# 3. Verify frontend build
npm run build

# 4. Run Rust test suite (from repo root or src-tauri)
cargo test --manifest-path src-tauri/Cargo.toml

# 5. Check Rust formatting and linter
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
```

For testing settings panes headlessly:
```powershell
npm run verify:panes
```

See [verify/README.md](verify/README.md) for details on the headless Chrome DevTools Protocol (CDP) test harness.

---

## Which checks need a desktop

- **Every PR** (`.github/workflows/ci.yml`, hosted runner, no desktop): the format,
  plugin-id and `tsc --noEmit` steps (the parts of `npm run lint`), `npm test`,
  `npm run build`, `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --lib`,
  `npm run verify:panes` and `npm run verify:palette`.
- **Nightly only** (`.github/workflows/nightly.yml`): the full `cargo test` (not just
  `--lib`), `npm run verify:trail-page`, and the probes that drive the running app,
  including `verify:app` and `verify:drag`. On a hosted runner those report "could not
  run" and are recorded as skipped, not green.
- **Needs a live desktop** with the app running and its debug port open on 9222
  (`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 npm run tauri dev`):
  `npm run verify:app`, and every other probe that attaches to the running app. `verify:drag`
  also needs two screens. See [verify/README.md](verify/README.md#checking-the-running-app).

---

## Guidelines for Changes

### 1. File & Desktop Safety
- **Never manipulate real user files during unit tests or probes.** Use isolated test fixtures (e.g. `tempfile::TempDir` in Rust, or `Fixture` in `verify/runtime.mjs`).
- Never perform silent or destructive operations on disk outside the configured `files_root`.

### 2. Architecture & Modules
- The Rust backend owns disk operations, Windows shell integration, monitor coordinate mappings, and persistent records.
- The TypeScript frontend owns widget DOM rendering, animations, and hit-testing rectangles.
- Consult [docs/CONCEPTS.md](docs/CONCEPTS.md) and [docs/DEVELOPING.md](docs/DEVELOPING.md) to understand core invariants.
- If proposing architectural changes, check existing ADRs in [docs/adr/](docs/adr/).

### 3. Commit Conventions
We follow Conventional Commits format:
- `feat(scope): description`
- `fix(scope): description`
- `docs(scope): description`
- `refactor(scope): description`
- `chore(scope): description`

---

## Submitting a Pull Request

1. Create a feature branch:
   ```powershell
   git checkout -b feat/your-feature-name
   ```
2. Commit your changes following conventional commit syntax.
3. Ensure all tests and format checks pass.
4. Push to your fork and submit a Pull Request against the `main` branch.
5. Provide a clear description of the problem solved, testing methodology, and any UX changes.

Maintainers: see [docs/BRANCH-PROTECTION.md](docs/BRANCH-PROTECTION.md) for how `main` is protected.
