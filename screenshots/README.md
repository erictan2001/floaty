# Screenshots

Images referenced by the README and the docs pages. `desktop-screenshot.png`,
`palette.png` and `settings-general.png` are real; the rest are **placeholders** — the
documents already point at these filenames, so dropping a file in with the right name
makes the picture appear. Nothing needs editing. Screenshots are committed compressed:
flatten to RGB (they are opaque) and quantise to a 256-colour palette, which costs
about 1 colour level on average and saves two thirds of the bytes. `tools/compress_screenshots.py`
does it and reports how much each image actually changed.

| File | What it should show | Used by |
| --- | --- | --- |
| `desktop-screenshot.png` | the whole desktop with floaties piled up (exists, 2.6 MB — could be compressed too) | README |
| `palette.png` | the `Ctrl+Alt+Space` launcher open, mid-search (exists, 373 KB) | README |
| `settings-general.png` | the General settings tab (exists, 117 KB) | README |
| `two-screens.png` | a floatie being dragged across a monitor boundary | docs/CONCEPTS.md |
| `widgets.png` | the widget kinds side by side: note, clock, Live2D, visualizer, sysmon | docs/WIDGETS.md |
| `diagnostics-pane.png` | the Diagnostics pane with its log tail and monitor table | docs/DEVELOPING.md |

Tips that match how the app actually looks: capture the overlay with the desktop
visible behind it (not a maximised window), and on a multi-monitor setup prefer a
crop that still shows the seam between screens. `trails/` holds motion captures used
for the gravity write-ups.
