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
