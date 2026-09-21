"""Move one marked section of lib.rs into its own module.

lib.rs is 9710 lines and already carries 20 `// ---------- name ----------` markers. This
turns a section into `src-tauri/src/<name>.rs` and leaves a `mod`/`use` pair behind, so the
split is one command per section and the build is green (or reverted) at every step.

What it does, in order:
  1. find the section's line range, from its marker to the next marker (or EOF);
  2. move those lines out, keeping them byte-for-byte;
  3. give every *top-level* item in the moved text `pub(crate)` visibility, because the rest
     of the crate calls into it across the module boundary now;
  4. prepend `use crate::*;` so the moved code still sees everything it used to;
  5. replace the section in lib.rs with `mod <name>;` and `use <name>::*;`.

Refuses to touch a section that does not exist, and prints the before/after line counts so
the result is checkable.

Usage: python extract_section.py <name>            # e.g. diagnostics
       python extract_section.py --list
       python extract_section.py --revert <name>   # put it back (undoes step 5 and deletes)
"""

import io
import os
import re
import sys

ROOT = r"C:\Users\erict\OneDrive\Desktop\floaty\src-tauri\src"
LIB = os.path.join(ROOT, "lib.rs")
MARKER = re.compile(r"^// ---------- (?P<name>.+?) ----------\s*$")

# Top-level items get crate visibility. `impl` blocks are left alone: they carry their own
# visibility from the type they are for.
ITEM = re.compile(
    r"^(?P<indent>(?:pub(?:\([^)]*\))?\s+)?)(?P<rest>(?:async\s+)?fn |struct |enum |union |const |static |type |trait |mod )"
)


def sections(lines):
    """Every marker's name and its (start, end) line numbers, 1-based, end exclusive."""
    found = []
    for index, line in enumerate(lines):
        match = MARKER.match(line)
        if match:
            found.append((match.group("name"), index))
    out = []
    for position, (name, start) in enumerate(found):
        end = found[position + 1][1] if position + 1 < len(found) else len(lines)
        out.append((name, start, end))
    return out


def make_crate_visible(body):
    """`pub(crate)` on every top-level item that is not already pub."""
    out = []
    for line in body:
        match = ITEM.match(line)
        if match and not line.startswith("pub") and not line.startswith("#"):
            line = "pub(crate) " + line
        out.append(line)
    return out


def module_name(name):
    return re.sub(r"[^a-z0-9]+", "_", name.lower()).strip("_")


def main():
    lines = io.open(LIB, encoding="utf-8", newline="").read().splitlines(keepends=True)
    found = sections(lines)

    if sys.argv[1:2] == ["--list"]:
        for name, start, end in found:
            print(f"{module_name(name):<28} lines {start + 1:>5}-{end:<5} ({end - start} lines)")
        return

    if sys.argv[1:2] == ["--revert"]:
        name = module_name(sys.argv[2])
        path = os.path.join(ROOT, f"{name}.rs")
        if not os.path.exists(path):
            print(f"no {path}")
            return
        body = io.open(path, encoding="utf-8", newline="").read().splitlines(keepends=True)
        # strip the header this tool added (comment block + use crate::*)
        while body and (body[0].startswith("//") or body[0].strip() == "" or body[0].startswith("use crate::*;")):
            body.pop(0)
        text = "".join(body)
        out = "".join(lines).replace(f"mod {name};\nuse {name}::*;\n", text, 1)
        io.open(LIB, "w", encoding="utf-8", newline="").write(out)
        os.remove(path)
        print(f"reverted {name}")
        return

    name = module_name(sys.argv[1])
    wanted = [entry for entry in found if module_name(entry[0]) == name]
    if not wanted:
        print(f"no section called {name}; try --list")
        sys.exit(2)
    _, start, end = wanted[0]

    body = lines[start:end]
    # drop trailing blank lines so the module file is tidy, but keep the section's own text
    while body and body[-1].strip() == "":
        body.pop()
        end -= 1

    # The moved code refers to `AppHandle`, `Value`, `PathBuf` … by their short names, which
    # come from lib.rs's own top-level `use` lines. They have to come along, or every module
    # this tool makes fails to compile on names it cannot see.
    outer = []
    for line in lines[:start]:
        if line.startswith("use ") or (outer and line.startswith("    ") and outer[-1].startswith("use ")):
            outer.append(line)
        elif outer and line.strip() == "":
            continue
        elif outer:
            break

    header = [
        f"//! `{sys.argv[1]}` — moved out of lib.rs verbatim.\n",
        "//!\n",
        "//! Visibility is `pub(crate)` because the rest of the crate calls in across the module\n",
        "//! boundary now; nothing here changed shape on the way out.\n",
        "\n",
        "#![allow(unused_imports)]\n",
        "use crate::*;\n",
        *outer,
        "\n",
    ]
    io.open(os.path.join(ROOT, f"{name}.rs"), "w", encoding="utf-8", newline="").write(
        "".join(header + make_crate_visible(body))
    )

    replacement = [f"mod {name};\n", f"use {name}::*;\n"]
    lines[start:end] = replacement
    io.open(LIB, "w", encoding="utf-8", newline="").write("".join(lines))
    print(f"{name}: {end - start} lines moved, lib.rs {len(lines) - len(replacement) + (end - start)} lines")


if __name__ == "__main__":
    main()
