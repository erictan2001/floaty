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

Those three files are the only ones that carry a version, and the bump lands at tag
time, so `main` carries the version being worked on and never claims one older than the newest tag ([ADR 0003](adr/0003-two-version-numbers.md)).

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
which release they are.

That release is a **draft**, and nothing publishes it until the feed has been checked.
tauri-action both builds and uploads, so a release that went public on upload would be
public before anything had looked at it. Each leg smoke-tests the installers it has just
built, then reads the feed back off the draft the way an installed copy reads it; only the
last leg — the `x86_64` one, by which point both architectures are in the feed — re-runs
the whole check for both platforms and, if every one of them passes, takes the draft out of
draft. A release cannot go public unverified: the invocation that publishes is the one that
checked. [What is checked before a release goes out](#what-is-checked-before-a-release-goes-out)
has the list, and [Withdrawing a bad release](#withdrawing-a-bad-release) has what the
draft buys you.

The same workflow can be run by hand against a tag that already exists (Actions →
**release** → *Run workflow*, naming the tag). The draft-then-publish ordering is not a
setting to remember — it is what the workflow does — and see [Signing](#signing-it) for
what a signed *installer* needs (the thumbprint is per-machine, so it belongs in a
repository secret).

## Releasing an update

A tagged release publishes two things: the installer, and the feed a running copy reads to
find it. `plugins > updater > endpoints` in `src-tauri/tauri.conf.json` is
`https://github.com/erictan2001/floaty/releases/latest/download/latest.json`, so the
manifest and the signed archive it names have to be attached to the latest *published*
release — the workflow keeps the release a draft until the checks pass and publishes it
then — or that URL 404s and a check reports "could not check" rather than "up to date".
That address has to be the stable one and not a per-release URL, which is one of the things
the guard refuses a feed for: withdrawing a bad release is the escape hatch
([below](#withdrawing-a-bad-release)), and a per-release address would not survive it.
There is no feed to host: the release is the CDN. `bundle.createUpdaterArtifacts` in the
same file is what makes the signed archives exist at all; with it off, the build produces
installers only and `latest.json` never appears.

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
says exactly that instead of reporting you as up to date. An update that fails leaves the
install it was replacing running rather than half-applied.

## What is checked before a release goes out

Eight checks, all of them before publication rather than after it: there is no server
behind the updater, so nothing can be turned off remotely
([ADR 0002](adr/0002-withdraw-bad-releases.md)). They live in
`scripts/verify-updater-feed.mjs`, and each one prints what it looked at and the value it
saw, so a failure names the value that was wrong rather than only that something was.

Seven of them read the feed back off the draft release the way an installed copy reads it:

- the manifest's version — and the version in `src-tauri/tauri.conf.json` — is the version
  the tag names;
- every artifact the manifest points at is on the release and is not zero bytes;
- the `.sig` beside each artifact is there, is not empty, and is byte-for-byte the
  signature the manifest carries for it;
- that signature verifies against `plugins > updater > pubkey` in
  `src-tauri/tauri.conf.json` under the same minisign rules a running copy uses: the key id
  has to match, the artifact is hashed with BLAKE2b-512 when the signature is prehashed,
  and the global signature has to cover the trusted comment;
- every URL in the manifest belongs to this repository and names this tag — a feed entry
  left over from an older release is the failure this catches;
- the platform keys that leg has to see are present: `windows-aarch64` on the ARM64 leg,
  both `windows-x86_64` and `windows-aarch64` on the last one;
- the endpoint is the stable `.../releases/latest/download/latest.json` address, not one
  that only exists for a single release — withdrawal is the escape hatch, and a
  per-release address would not survive it.

The eighth runs before anything is uploaded: the installers that were just built exist, are
not empty, are the file types they claim to be (MZ for an `.exe`), and each has a `.sig`
that verifies over its file.

Locally, `npm run verify:updater` is the entry point to the same guard — it runs the
`--artifacts` mode, so it wants the tag and the files:

```powershell
$env:TAG = "v0.2.0"
npm run verify:updater -- "src-tauri/target/release/bundle/nsis/Floaty_0.2.0_x64-setup.exe,src-tauri/target/release/bundle/nsis/Floaty_0.2.0_x64-setup.exe.sig"
```

It reads the config, checks the endpoint and the public key, then each artifact and its
signature. With no `--repo` there is no release to read, so it is the pre-upload half of
the checks rather than the whole set.

Two things are deliberately not covered, and neither should be read into the checks:

- **The installer is not installed and launched.** This is a GUI app — a tray icon and
  desktop-layer widgets — and a hosted Windows runner has no desktop session to launch it
  into, so an install-and-launch step would be a check that only looks like it tested
  something. What is checked instead is the part that decides whether an update is ever
  accepted: the files, their signatures, and the feed.
- **A transient network failure fails the run closed.** The guard downloads the release
  assets so that it verifies the signatures over the bytes the release actually serves, so
  a hiccup in that download fails the check and leaves the release a draft. That needs a
  re-run, not a fix; nothing is published on a bad run.

## Withdrawing a bad release

The escape hatch is withdrawal, not a kill switch: delete the release and its
`latest.json`, then publish a fixed one ([ADR 0002](adr/0002-withdraw-bad-releases.md)).
A copy that has not updated fails its check visibly and keeps running the build it already
has — an update that fails leaves the old install running rather than replaced — and the
update check is manual ([above](#releasing-an-update)), so nothing changes underneath a
running app while that is sorted out.

That only works if the bad release never went public, and the workflow is arranged to keep
it that way: the release is a draft until the checks pass, so one that fails them is
already withdrawn by staying a draft, without anyone doing it by hand. While it is a draft,
`/releases/latest` keeps serving the *previous* release — which is what an installed copy
should see, rather than a feed that carries one architecture and not the other yet, or one
whose signature does not verify. When you do have a bad *published* release, delete it and
its `latest.json`: the next release takes over the same address, which is why the endpoint
is a stable one rather than a per-release URL.

---

[<- Back to the README](../README.md)
