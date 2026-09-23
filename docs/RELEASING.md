# Shipping a release

Cutting a version, signing it, and how an installed copy updates itself.
You only need this page when you are publishing.

## Building a release

```powershell
npm run tauri build
```

The release build runs the frontend checks first (the same `npm run build`: plugin
ids, `tsc`, Vite), then compiles in release and bundles whatever `bundle.targets`
in `tauri.conf.json` asks for — `"all"` here, which on Windows means both
installers:

| Output | What it is |
| --- | --- |
| `src-tauri\target\release\bundle\nsis\Floaty_<version>_<arch>-setup.exe` | NSIS installer — `arm64` or `x64`, following the target |
| `src-tauri\target\release\bundle\msi\Floaty_<version>_<arch>_en-US.msi` | WiX (MSI) installer |
| `src-tauri\target\release\floaty.exe` | the application on its own |

Without `--target` the build is for the machine you are on, so on this ARM64 machine the
installer it leaves is the `arm64` one; the workflow asks for both explicitly (see
[Releasing](#releasing)).

The first build needs network and some patience: Tauri downloads the WiX and NSIS
toolchains and every crate compiles from scratch. Later builds reuse `target/`.
Windows only, and nothing is signed unless you sign it.

Both architectures build from the same command with a target:

| Target | What it is |
| --- | --- |
| `x86_64-pc-windows-msvc` | Intel and AMD — what you get by default |
| `aarch64-pc-windows-msvc` | Windows on ARM |

```powershell
rustup target add aarch64-pc-windows-msvc
npm run tauri build -- --target aarch64-pc-windows-msvc --bundles nsis
```

ARM64 is NSIS only: the WiX path for arm64 is not worth relying on, and the installer
this project points people at is the NSIS one. The ARM64 MSVC toolset is part of a
normal Visual Studio install with the C++ workload.

One trap worth knowing before it bites: `tauri::generate_context!()` reads
`frontendDist` *while the crate compiles*, so a Rust step with no frontend build behind
it panics — `cargo test` and `cargo check` included:

```
error: proc macro panicked
    --> src\lib.rs:5702:16
    = help: message: The `frontendDist` configuration is set to `"../dist"` but this path doesn't exist
```

Run `npm run build` first, or let `npm run tauri build` do it — that is what its
`beforeBuildCommand` is for.

## Signing it

Builds are unsigned: Windows has no certificate to check, so the first run gets
SmartScreen's "Windows protected your PC" (*More info* → *Run anyway*) and the
installer's publisher shows as unknown. That is a certificate, not a code change —
with one in the machine's certificate store, Tauri signs both the executable and
the installers through `signtool`. In `src-tauri/tauri.conf.json`:

```json
"bundle": {
  "windows": {
    "digestAlgorithm": "sha256",
    "certificateThumbprint": "<thumbprint of your certificate>",
    "timestampUrl": "http://timestamp.digicert.com"
  }
}
```

Use `signCommand` (with a `%1` placeholder for the binary) for any other signing
tool. The thumbprint is per-machine, which is why this repository carries none.
Updater archives are signed with a key of their own instead — different key, different
check, different reader: [Releasing an update](#releasing-an-update).

## Releasing

A tag is the release. Write the version down once, tag it, push:

```powershell
node scripts/set-version.mjs 0.2.0   # tauri.conf.json, Cargo.toml, package.json
git commit -am "release 0.2.0"
git tag v0.2.0
git push origin main --tags
```

`.github/workflows/release.yml` then checks the tree (`check-plugins`, `tsc`, and
`cargo test` — a tag whose tests fail gets no release), builds the bundles for **both
Windows architectures** — `aarch64-pc-windows-msvc` (NSIS) on GitHub's `windows-11-arm`
runner and `x86_64-pc-windows-msvc` (NSIS) on `windows-latest` — and attaches them and the
signed updater feed (`latest.json` and its `.sig` files) to the GitHub release for that
tag. The targets are the point: the updater feed is keyed by the triple of the machine
running the app, so a release has to carry an entry for each machine it means to update —
[Releasing an update](#releasing-an-update) has the detail. Each leg runs on its native
runner, and they run one after the other because tauri-action *merges* its entry into the
feed already attached to the release instead of replacing it. The frontend is built
*before* any Rust step — `cargo test` included — because the crate compiles `dist/` into
itself (see [Release](#building-a-release)); each leg runs the tests natively, on the target it is
releasing. The version in the bundles is
taken from the tag, so the release page and the file you download cannot disagree about
which release they are. The same workflow can be run by hand against a tag that already
exists; set `releaseDraft: true` in it if you would rather review a release before it
goes public, and see [Signing](#signing-it) for what a signed *installer* needs (the
thumbprint is per-machine, so it belongs in a repository secret).

## Releasing an update

A tagged release publishes two things: the installer, and the feed a running copy reads to
find it. `plugins > updater > endpoints` in `src-tauri/tauri.conf.json` is
`https://github.com/erictan2001/floaty/releases/latest/download/latest.json`, so the
manifest and the signed archive it names have to be attached to the latest *non-draft*
release — the workflow publishes with `releaseDraft: false` — or that URL 404s and a check
reports "could not check" rather than "up to date". There is no feed to host: the release
is the CDN. `bundle.createUpdaterArtifacts` in the same file is what makes the signed
archives exist at all; with it off, the build produces installers only and `latest.json`
never appears.

**Two repository secrets** (Settings → Secrets and variables → Actions).
`TAURI_SIGNING_PRIVATE_KEY` is the base64 contents of the key file below — CI has no
`~/.tauri`, so the secret holds the text, not a path. `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
is that key's password, which here is empty: Tauri reads an unset password as an empty
string in CI, so if the settings form refuses an empty value, leaving the secret out is the
same thing. A release with no private key fails the build rather than quietly attaching
unsigned updater archives.

**The key has no password.** It is `C:\Users\<you>\.tauri\floaty.key`, with `floaty.key.pub`
beside it, made by `npm run tauri signer generate` with the password left blank. That is
convenient — there is nothing for CI to be handed beyond the key — and weaker than a
password would be: whoever reads the key, or the secret, can sign an update that every
installed copy accepts as genuine. Regenerating the pair with a password means putting the
new public key into `src-tauri/tauri.conf.json` and updating both secrets; and note that a
release signed with a *new* key is refused by every copy still carrying the old public key,
so those installs have to fetch the installer by hand once.

**The public key has to stay in step with the private key.** The `pubkey` in the config is
what a running copy verifies a download against. If it is not the key that signed the
release, every update is refused, and the bundler says so at build time — "The updater
secret key from `TAURI_SIGNING_PRIVATE_KEY` does not match the public key from `plugins >
updater > pubkey` … won't be accepted at runtime". That mismatch is not a warning to ship
past.

**A release carries both triples.** The updater looks up its manifest entry by the target
of the machine *running* the app, not of the machine that built the release. This machine is
ARM64, so it asks for `windows-aarch64`, and a manifest that only carries `windows-x86_64`
fails the check with `None of the fallback platforms [windows-aarch64] were found in the
response platforms object` — and an ARM64-only release fails an x64 copy the same way in
reverse. That is why the workflow builds both: `aarch64-pc-windows-msvc` on GitHub's
`windows-11-arm` runner and `x86_64-pc-windows-msvc` on `windows-latest`, each on its
native runner, each attaching its own artifacts, signatures and manifest entry. The legs
are serialised (`max-parallel: 1`) because tauri-action reads the `latest.json` already on
the release and keeps its `platforms` before adding its own: run in parallel, the two legs
would each read a feed missing the other's key and the release would end up
single-architecture.

**The update check is manual.** Nothing phones home: no request on launch, no polling while
it runs. General tab → **Updates** → *check for updates* is the only thing that touches the
feed, and *install and restart* the only thing that downloads — the archive is verified
against the public key above and the app restarts into it. A check that could not be made
says exactly that instead of reporting you as up to date.

---

[<- Back to the README](../README.md)
