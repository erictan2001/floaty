# Two version numbers, two policies

The app version and the plugin API version answer different questions and move
independently. The app version is semver and is bumped in `package.json`,
`tauri.conf.json` and `Cargo.toml`. `main` carries the version being worked on - the next
release once work on it starts - and must never claim a version older than the newest tag.
`sync-main-version` enforces the second half of that when a release succeeds. The plugin API version (`PLUGIN_API_VERSION`, mirrored by `WIDGET_API_VERSION` and
enforced by `scripts/check-plugins.mjs`) changes only when the surface a plugin sees
changes.

## Consequences

A plugin-API break requires three things, not one: a bump, a CHANGELOG line, and a
deprecation window of one release, during which the old number still loads and warns. A
version number alone does not catch a break - `pet_speed` was removed from the documented
`api.settings()` surface with no bump and no record - which is why the record is part of the
policy rather than a separate nicety.
