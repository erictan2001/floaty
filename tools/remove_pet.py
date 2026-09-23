"""Remove the pet widget from floaty.

Every edit is anchored on an exact string and asserted, so a mismatch fails loudly
instead of silently doing nothing. Line endings are detected and preserved.
Run from the repo root:  python tools/remove_pet.py
"""
import pathlib
import sys

root = pathlib.Path('.')
report = []


def load(rel):
    p = root / rel
    raw = p.read_bytes().decode('utf-8')
    crlf = '\r\n' in raw
    return p, raw.replace('\r\n', '\n'), crlf


def save(p, text, crlf):
    out = text.replace('\n', '\r\n') if crlf else text
    p.write_bytes(out.encode('utf-8'))


def edit(rel, pairs, note=''):
    p, text, crlf = load(rel)
    for old, new in pairs:
        assert old in text, f'{rel}: anchor not found:\n{old[:200]}'
        text = text.replace(old, new, 1)
    save(p, text, crlf)
    report.append(f'  {rel}: {len(pairs)} edit(s) {note}')


# ---------- Rust: the built-in descriptor ----------
edit('src-tauri/src/plugins.rs', [
    ('''    PluginDef {
        id: "pet",
        name: "Pet",
        description: "A wandering blob. Hover to calm it, double-click to freeze it.",
        default_size: (170.0, 170.0),
        resizable: false,
        default_data: || serde_json::json!({ "name": "bloop" }),
        custom_size: None,
        desktop_item: None,
        add_label: Some("+ pet"),
        layout_priority: 4,
    },
''', ''),
    ('const KINDS: [&str; 9] = [\n        "note",\n        "clock",\n        "pet",\n',
     'const KINDS: [&str; 8] = [\n        "note",\n        "clock",\n'),
    ('let disabled = vec!["live2d".to_string(), "pet".to_string()];',
     'let disabled = vec!["live2d".to_string()];'),
], 'built-in descriptor + tests')

# ---------- Rust: the pet_speed setting ----------
edit('src-tauri/src/global_floating_settings.rs', [
    ('    #[serde(default = "default_pet_speed")]\n    pub(crate) pet_speed: f64,\n', ''),
    ('pub(crate) fn default_pet_speed() -> f64 {\n    1.0\n}\n', ''),
    ('            pet_speed: default_pet_speed(),\n', ''),
    ('        pet_speed: settings.pet_speed.clamp(0.0, 3.0),\n', ''),
], 'pet_speed removed')

# ---------- Rust: demo mode + comments ----------
edit('src-tauri/src/lib.rs', [
    ('                // also a clock + pet for the visual check\n',
     '                // also a clock for the visual check\n'),
    ('                create_record(&handle, "pet").ok();\n', ''),
])
edit('src-tauri/src/launcher_palette.rs', [
    ('// A note, a clock, a pet: it is already where it belongs. Saying so',
     '// A note, a clock, a widget: it is already where it belongs. Saying so'),
])
edit('src-tauri/src/sleep_resume.rs', [
    ('// poll that costs a powershell, a pet that animates — all have a',
     '// poll that costs a powershell, a widget that animates — all have a'),
])

# ---------- TypeScript: registration, types, settings ----------
edit('src/widgets/plugin.ts', [
    ('import { petPlugin } from "./pet";\n', ''),
    ('  petPlugin,\n', ''),
    ("`gravity`, `pet_speed`, one of the", "`gravity`, `bounce`, one of the"),
])
edit('src/widgets/lib.ts', [
    ('  pet_speed: number;\n', ''),
    ('  pet_speed: 1,\n', ''),
])
edit('src/settings/params.ts', [
    ('  { key: "pet_speed", label: "pet speed", min: 0, max: 2, step: 0.1 },\n', ''),
])
edit('src/settings/panes/plugins.ts', [
    ("(a pet's speed, a visualizer's gain)", "(a visualizer's gain, the folder it scans)"),
])
edit('src/widgets/appicon.ts', [
    ('// Manual drag (same reason as the pet): an OS-level startDragging on every',
     '// Manual drag: an OS-level startDragging on every'),
    ('// capture lazily on first real movement (see pet.ts: eager capture eats taps)',
     '// capture lazily on first real movement (eager capture eats taps)'),
])
edit('src/widgets/folder.ts', [
    ('// capture lazily on first real movement (see pet.ts: eager capture eats taps)',
     '// capture lazily on first real movement (eager capture eats taps)'),
])

# ---------- CSS: the whole pet block + its kind chip ----------
p, css, crlf = load('src/style.css')
start = css.index('/* ---------- pet ---------- */')
end = css.index('/* ---------- gravity app launchers ---------- */')
assert start < end, 'style.css: pet block markers out of order'
removed = css[start:end]
assert '.pet-tag' in removed and '.eye' in removed, 'style.css: block looks wrong'
css = css[:start] + css[end:]
chip = '.k-pet { background: rgba(110, 231, 183, 0.15); color: #6ee7b7; }\n'
assert chip in css, 'style.css: .k-pet chip not found'
css = css.replace(chip, '', 1)
save(p, css, crlf)
report.append(f'  src/style.css: pet block removed ({removed.count(chr(10))} lines) + .k-pet')

# ---------- delete the widget module ----------
pet = root / 'src/widgets/pet.ts'
lines = len(pet.read_text(encoding='utf-8').splitlines())
pet.unlink()
report.append(f'  src/widgets/pet.ts: deleted ({lines} lines)')

# ---------- verify scripts ----------
edit('verify/panel-drag.mjs', [
    ('const kinds = ["sysmon", "clock", "visualizer", "pet", "note"];',
     'const kinds = ["sysmon", "clock", "visualizer", "note"];'),
    ('(sysmon, clock, note, visualizer or pet)', '(sysmon, clock, note or visualizer)'),
])
edit('verify/stranded.mjs', [
    ('const kinds = ["sysmon", "clock", "note", "visualizer", "pet"];',
     'const kinds = ["sysmon", "clock", "note", "visualizer"];'),
    ('(sysmon, clock, note, visualizer or pet)', '(sysmon, clock, note or visualizer)'),
    ('const kinds = ["sysmon", "clock", "note", "countdown", "pet"];',
     'const kinds = ["sysmon", "clock", "note", "countdown"];'),
])
edit('verify/panes.mjs', [
    ('pet_speed: 1, gravity: 2600', 'gravity: 2600'),
    ('plugin("note", "Note"), plugin("clock", "Clock"), plugin("pet", "Pet"),',
     'plugin("note", "Note"), plugin("clock", "Clock"),'),
    ('rejected_plugins: ["broken-pet: plugin.json did not parse"]',
     'rejected_plugins: ["broken-note: plugin.json did not parse"]'),
])

# ---------- docs ----------
edit('README.md', [
    ('Sticky notes, a pomodoro clock, a wandering pet, a Live2D\n',
     'Sticky notes, a pomodoro clock, a Live2D\n'),
])
edit('docs/WIDGETS.md', [
    ('| `pet` | a wandering blob; hover to calm it, double-click to freeze it |\n', ''),
    ('notes, clock, pet, Live2D, visualizer, system monitor', 'notes, clock, Live2D, visualizer, system monitor'),
])
edit('docs/DEVELOPING.md', [
    ('# float a few sample apps, a clock and the pet', '# float a few sample apps and a clock'),
    ('note.ts clock.ts pet.ts live2d.ts', 'note.ts clock.ts live2d.ts'),
])
edit('docs/PLUGINS.md', [
    ('(`note`, `clock`, `pet`, `app`, `file`, `folder`, `live2d`, `visualizer`, `sysmon`)',
     '(`note`, `clock`, `app`, `file`, `folder`, `live2d`, `visualizer`, `sysmon`)'),
    ('live2d 1, note 2, clock 3, pet 4, visualizer/sysmon 5',
     'live2d 1, note 2, clock 3, visualizer/sysmon 5'),
    ('ctx.getSharedRow("pet_speed")', 'ctx.getSharedRow("gravity")'),
    ("(a pet's speed, a visualizer's gain, the folder it scans)",
     "(a visualizer's gain, the folder it scans)"),
    ('names (`gravity`, `pet_speed`, …)', 'names (`gravity`, `bounce`, …)'),
    ('(`gravity`, `bounce`, `pet_speed`, …)', '(`gravity`, `bounce`, …)'),
])
edit('screenshots/README.md', [
    ('note, clock, pet, Live2D, visualizer, sysmon', 'note, clock, Live2D, visualizer, sysmon'),
])

print('\n'.join(report))
print(f'\n{len(report)} files touched')
