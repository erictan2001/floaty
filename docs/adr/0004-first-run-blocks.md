# The first run blocks; every later root change warns

Adopting a folder means Floaty treats its contents as the desktop, and a merge moves real
files inside it. On the first launch a stranger has no mental model of that, so the app
shows the folder it is about to adopt and what is in it, and waits for confirmation. After
that, changing the root warns and proceeds: a confirmation step you learn to click through
is worse than no step at all.

## Consequences

The first-run path gains a step that cannot be skipped, so the headless probes must either
pre-seed a root or drive the confirmation. The install path stops being fully automatable,
which is a cost worth paying once per machine.
