# Floaty is built for strangers to depend on

Floaty is thirteen days old, written by one person with heavy AI assistance, and that
would normally argue for treating it as a personal tool that happens to be published.
We are not doing that. The target is a product strangers can depend on, and the maturity
order that follows is: a stranger installs it and uses it for a week without hitting a
bug that makes them uninstall; the project can be put down for six months and picked up
without archaeology; a contributor can land a change without asking first.

What maturity must never cost: no telemetry, no crash reporting, no support SLA, no
cloud dependency, and no process or CI step that would be skipped on a busy day. A
maturity plan that adds maintenance which then gets abandoned leaves the project less
mature than it is now, so anything adopted has to be worth keeping on a bad week.

## Considered Options

**A personal tool that is safe to publish** — the alternative recommended during the
session, and much cheaper: hermetic tests, honest docs, clean releases, no compatibility
promises. Rejected because the project already ships signed releases for two
architectures with a working auto-updater and a versioned plugin API
(`PLUGIN_API_VERSION` / `PLUGIN_API_MIN`). The parts a stranger actually touches already
exist, and those are the parts carrying the risk.

## Consequences

"Strangers depend on it" is not yet defined, and the definition decides most of the
remaining work: if it means *trustworthiness*, the failures that matter are data loss, a
crash and a bricked update; if it means *support*, that is a promise this ADR already
rules out.

The plugin API needs a stated deprecation window and a record of what changed between
versions. The same week `docs/PLUGINS.md` promised the surface "stays stable", a
documented setting (`pet_speed`) was removed from `api.settings()` and nothing anywhere
records it — the version stayed at 2. A version number without a policy does not catch
that; a changelog and a stated window would have.
