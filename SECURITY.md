# Security Policy

Floaty takes the security and safety of user systems seriously. Floaty has direct integration with the Windows Shell, manipulates desktop files, captures system audio via WASAPI loopback, and executes third-party JavaScript plugins.

## Supported Versions

Security fixes are released for the latest published version of Floaty:

| Version | Supported          |
| ------- | ------------------ |
| 0.2.x   | :white_check_mark: |
| < 0.2.0 | :x:                |

## Reporting a Vulnerability

If you discover a security vulnerability in Floaty, **please do not report it in a public GitHub issue.**

Instead, please report security issues via one of the following methods:

1. **GitHub Security Advisories (Recommended):** Use the "Report a vulnerability" button under the **Security** tab of the Floaty GitHub repository.
2. **Private Communication:** Contact the maintainer directly through their GitHub profile or security contact.

### What to Include

Please provide:
- A description of the vulnerability and its potential impact.
- Step-by-step instructions or proof-of-concept code to reproduce the issue.
- Details of your environment (Windows version, Floaty version, multi-monitor configuration).
- Any proposed mitigations or fixes, if available.

### Response Timeline

- **Initial Acknowledgement:** Within 48 hours.
- **Assessment & Triage:** Within 5 business days.
- **Fix & Disclosure:** Fixes will be prepared in private and released in a coordinated update.

## Security Architecture & Invariants

When contributing or auditing code, keep the following security properties in mind:
- **Desktop Isolation:** Floaty must only manipulate files located within the configured `files_root` or the user's Desktop folder. Deletions must go to the Recycle Bin rather than permanently deleting without user confirmation.
- **Plugin Approvals:** Third-party plugins must be fingerprinted and explicitly approved by the user before execution.
- **Updater Integrity:** The automatic updater enforces Minisign cryptographic signature verification using the embedded public key before applying any update bundle.

## Plugin Threat Model

A third-party plugin is code that runs with floaty's access. Approval records what the user read. It does not limit what the code does. The points below were checked against the source; anything not verified is marked as such.

### What plugin JavaScript can reach

- **Its own widget DOM.** The module is imported into every window that draws or configures it: the overlay (`desktop-overlay` / `top-overlay`) in overlay mode, `widget-<id>` in per-window mode (`spawn_widget`), and the settings window (`renderSettings`, `renderWidgetControls`). It shares that page's JavaScript realm.
- **IPC, the allowlist is not a boundary.** `capabilities/default.json` applies to all windows (`"windows": ["*"]`) but grants only core window calls and dialog. It does not list floaty's own commands. Those come from `generate_handler!` in `lib.rs`. Tauri 2.11 checks app commands against capabilities only when the app has an app manifest (`has_app_manifest`, `webview/mod.rs`), and this app has none. So any local webview can call them. Remote origins are still rejected. This is from reading the source, not a runtime test. `createScopedPluginApi` (`src/widgets/plugin.ts`) filters only the `api.invoke` object it hands a plugin to `ALLOWED_PLUGIN_COMMANDS`. Code that imports `@tauri-apps/api/core` or uses the Tauri internals directly is not filtered. Unverified: whether something else blocks that path. Treat the allowlist as a guard against mistakes.
- **Files.** The asset protocol scope in `tauri.conf.json` covers `$APPDATA`, `$RESOURCE`, `$DESKTOP`, `$DOCUMENT`, `$DOWNLOAD`, `$PICTURES` and `$HOME/**`. The CSP allows `asset:` in `connect-src`, so a plugin can read any file in that scope by fetching its asset URL. Writes are not covered by the scope. Writes through floaty's own commands are not audited here.
- **Network.** The CSP has no `https:` source in `script-src` or `connect-src`, so remote scripts and arbitrary fetches should be blocked. Unverified at runtime.

### Approval is a fingerprint, not a sandbox

- An installed plugin is not imported until it is trusted (`plugin_approved` in `plugin_commands.rs`, `loadPlugins` in `plugin.ts`). Built-ins are always trusted.
- Approving stores `folder_fingerprint(dir)` in `settings.plugin_trust[id]`. The fingerprint covers every file under the folder, sorted by relative path, and is hashed with SHA-256 and stored as `sha256:<hex>`. Approvals recorded under the earlier 64-bit FNV format no longer match, so those plugins must be approved again. The hash shows the code is unchanged since approval; it does not show that the code is safe.
- Installing from a `.zip` is the approval: the install dialog shows what the archive holds. A folder dropped in by hand stays unapproved until you press the Plugins tab button.
- `plugin_trust` is part of floaty's settings in the app-data folder, which your user account can write. Anything that can write that file can add an approval.

### What invalidates approval

- Changing any byte of any file in the folder, including `plugin.json` and `index.js`.
- Adding, removing or renaming a file or subfolder. Paths are part of the hash.
- Timestamp changes alone do not.
- Unapproving or uninstalling the plugin.
- The check runs when the plugin list is built, so the next window load or rescan applies it. Unverified: whether a plugin already running in a window is re-checked. It is not re-imported until reload.

### What to install

- **Do:** read the whole folder before approving, including `index.js`. Install only from authors you trust. Re-read after every update, since an update is a new approval.
- **Don't:** install an archive or folder from a source you cannot identify. Don't approve a folder you did not put there. Don't assume an approval covers code you have not read.
- A plugin can read files in the asset scope above, including your home folder, and can call the commands in the allowlist. Install it only if you would trust it with that.
