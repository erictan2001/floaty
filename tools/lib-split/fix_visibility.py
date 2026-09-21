"""Make a just-split crate compile, using the compiler's own errors as the work list.

Splitting one file into modules turns every cross-module use of a private item into an error:
`field '0' of struct 'store::AppState' is private`, `cannot find type 'X' in the crate root`.
Rust's rule is that privacy is per module, so each of those items needs `pub(crate)` — and the
compiler names every one of them, which makes this a loop rather than a judgement call.

Round: run `cargo check --lib`, read the errors, patch the named items, repeat. Stops when
there are no more of the two shapes it understands, or after `--rounds`.

Usage: python fix_visibility.py [--rounds 8]
"""

import io
import os
import re
import subprocess
import sys

SRC = r"C:\Users\erict\OneDrive\Desktop\floaty\src-tauri\src"
CARGO = r"C:\Users\erict\OneDrive\Desktop\floaty\src-tauri"

FIELD = re.compile(r"field `(?P<field>[^`]+)` of struct `(?P<module>[A-Za-z0-9_]+)::(?P<struct>[A-Za-z0-9_]+)` is private")
NOT_IN_ROOT = re.compile(r"cannot find (?:type|value|function) `(?P<item>[A-Za-z0-9_]+)` in the crate root")


def read(path):
    return io.open(path, encoding="utf-8", newline="").read()


def write(path, text):
    io.open(path, "w", encoding="utf-8", newline="").write(text)


def check():
    proc = subprocess.run(
        ["cargo", "check", "--lib", "--message-format=short"],
        cwd=CARGO,
        capture_output=True,
        text=True,
        timeout=900,
    )
    return proc.stdout + proc.stderr


def add_field_visibility(text, struct, field):
    """`pub(crate)` on one field of a struct or enum, by walking its braces."""
    pattern = re.compile(rf"\b(struct|enum)\s+{re.escape(struct)}\b")
    for match in pattern.finditer(text):
        brace = text.find("{", match.end())
        if brace < 0:
            continue  # a tuple struct: handled by the caller
        depth = 0
        for index in range(brace, len(text)):
            if text[index] == "{":
                depth += 1
            elif text[index] == "}":
                depth -= 1
                if depth == 0:
                    break
        body = text[brace:index]
        field_line = re.compile(rf"^(\s*)({re.escape(field)}\s*:)", re.M)
        new_body, count = field_line.subn(r"\1pub(crate) \2", body, count=1)
        if count:
            return text[:brace] + new_body + text[index:], True
    # tuple struct: `struct AppState(pub(crate) Mutex<..>);`
    tuple_pattern = re.compile(rf"(struct\s+{re.escape(struct)}\s*\()", re.M)
    new_text, count = tuple_pattern.subn(r"\1pub(crate) ", text, count=1)
    return new_text, bool(count)


def add_item_visibility(text, item):
    """`pub(crate)` on a top-level definition, indented or not."""
    pattern = re.compile(
        rf"^(\s*)((?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:static|const|type|struct|enum|fn|union)\s+{re.escape(item)}\b)",
        re.M,
    )
    new_text, count = pattern.subn(r"\1pub(crate) \2", text, count=1)
    return new_text, bool(count)


def main():
    rounds = 8
    if "--rounds" in sys.argv:
        rounds = int(sys.argv[sys.argv.index("--rounds") + 1])

    for round_number in range(1, rounds + 1):
        output = check()
        fields = {}
        for match in FIELD.finditer(output):
            fields.setdefault(match.group("module"), set()).add((match.group("struct"), match.group("field")))
        items = {match.group("item") for match in NOT_IN_ROOT.finditer(output)}

        if not fields and not items:
            errors = len(re.findall(r"^error", output, re.M))
            print(f"round {round_number}: nothing left of the two shapes this knows (errors: {errors})")
            print("\n".join(output.splitlines()[:40]))
            return

        print(f"round {round_number}: {sum(len(v) for v in fields.values())} field(s), {len(items)} root item(s)")
        for module, wanted in fields.items():
            path = os.path.join(SRC, module + ".rs")
            if not os.path.exists(path):
                continue
            text = read(path)
            for struct, field in sorted(wanted):
                text, changed = add_field_visibility(text, struct, field)
                print(f"   {module}::{struct}.{field} {'ok' if changed else 'NOT FOUND'}")
            write(path, text)

        if items:
            # the item lives in exactly one of the extracted modules
            for name in sorted(os.listdir(SRC)):
                if not name.endswith(".rs") or name in ("lib.rs", "main.rs"):
                    continue
                path = os.path.join(SRC, name)
                text = read(path)
                touched = False
                for item in list(items):
                    new_text, changed = add_item_visibility(text, item)
                    if changed:
                        text = new_text
                        items.discard(item)
                        touched = True
                        print(f"   {name}: pub(crate) {item}")
                if touched:
                    write(path, text)
            for leftover in items:
                print(f"   {leftover}: NOT FOUND in any module")


if __name__ == "__main__":
    main()
