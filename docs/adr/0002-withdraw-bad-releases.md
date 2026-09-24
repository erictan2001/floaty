# Withdraw a bad release; do not build a kill switch

Floaty's updater reads a signed manifest published alongside the GitHub release, and there
is no server behind it. A strict, future-proof update path is worth breaking the current
one for, so the escape hatch for a bad release is to *withdraw* it - delete the release and
its `latest.json` and publish a fixed one - rather than to add a remote flag. Clients that
have not updated fail visibly and keep running the build they already have.

## Consequences

Existing installs (v0.1.0, v0.1.1) may not satisfy a stricter updater and may need a manual
reinstall. That cost was accepted explicitly when this was decided.

Nothing may depend on an endpoint that has to stay alive - the same reason there is no
telemetry and no crash reporting. The guards therefore live before publication rather than
after it: smoke-test the built installer, refuse a manifest whose version disagrees with the
tag it was published under, and leave the old install running when an update fails.
