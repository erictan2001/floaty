"""Split README.md into beginner-friendly docs.

The existing prose is moved VERBATIM (no rewriting) into docs/ pages; the new
README is written by hand afterwards. Run from the repo root.
"""
import pathlib

root = pathlib.Path('.')
lines = (root / 'README.md').read_text(encoding='utf-8').splitlines(keepends=True)


def block(a, b):
    """1-indexed inclusive line range, verbatim."""
    return ''.join(lines[a - 1:b]).strip('\n') + '\n'


BACK = '\n---\n\n[<- Back to the README](../README.md)\n'

docs = {
    'docs/CONCEPTS.md': (
        '# How Floaty thinks\n\n'
        'The ideas the rest of the app is built on. Worth ten minutes if you want to\n'
        'change how Floaty behaves, or if something it does looks surprising.\n\n'
        + block(284, 391)
        + '\n![Two screens, one drag: a floatie handed from one overlay to the other]'
          '(../screenshots/two-screens.png)\n'
        + BACK
    ),
    'docs/WIDGETS.md': (
        '# Widgets, plugins and settings\n\n'
        'What you can put on the desktop besides your files, how to write one of your\n'
        'own, and where every setting and file lives.\n\n'
        + block(392, 498)
        + '\n![The widget tray: notes, clock, pet, Live2D, visualizer, system monitor]'
          '(../screenshots/widgets.png)\n'
        + BACK
    ),
    'docs/DEVELOPING.md': (
        '# Working on Floaty\n\n'
        'How to build it, run it while you change it, and check your work before it\n'
        'goes anywhere.\n\n'
        + block(63, 95)
        + '\n' + block(173, 198)
        + '\n![The Diagnostics pane: log tail, monitors, window positions, heartbeats]'
          '(../screenshots/diagnostics-pane.png)\n'
        + '\n' + block(499, 623)
        + BACK
    ),
    'docs/RELEASING.md': (
        '# Shipping a release\n\n'
        'Cutting a version, signing it, and how an installed copy updates itself.\n'
        'You only need this page when you are publishing.\n\n'
        + block(96, 172)
        + '\n' + block(199, 283)
        + BACK
    ),
}

for path, text in docs.items():
    p = root / path
    p.parent.mkdir(parents=True, exist_ok=True)
    # normalise to CRLF, matching the rest of the repo
    p.write_bytes(text.replace('\n', '\r\n').encode('utf-8'))
    print(f'{path}: {len(text.splitlines())} lines')
