"""Print one section of a Markdown document by name.

Usage: python -I entry.py <file> "<name>"

A section is either a heading whose text starts with the name (printed up to
the next heading of the same or a higher level) or a top-level list item
whose text starts with the name (printed with its nested lines up to the next
top-level item, heading, or unindented paragraph). Matching ignores case,
heading marks, the "- " marker, and leading bold markers. With no match, the
headings and entry names containing the name are listed instead.
"""

import re
import sys

HEADING = re.compile(r"^(#{1,6})\s+(.*)$")
ITEM = re.compile(r"^[-*+]\s+(.*)$")


def key(text):
    return text.lstrip("*_ ").strip().lower()


def main():
    if len(sys.argv) != 3:
        sys.exit('usage: python -I entry.py <file> "<name>"')
    sys.stdout.reconfigure(encoding="utf-8")
    path, name = sys.argv[1], sys.argv[2].strip().lower()
    with open(path, encoding="utf-8") as f:
        lines = f.read().splitlines()

    # Each candidate is (start index, kind, level, text); kind is "h" or "i".
    candidates, fenced = [], False
    for i, line in enumerate(lines):
        if line.lstrip().startswith(("```", "~~~")):
            fenced = not fenced
        if fenced:
            continue
        if m := HEADING.match(line):
            candidates.append((i, "h", len(m.group(1)), m.group(2)))
        elif m := ITEM.match(line):
            candidates.append((i, "i", 0, m.group(1)))

    matches = [c for c in candidates if key(c[3]).startswith(name)]
    if not matches:
        near = [c for c in candidates if name in c[3].lower()]
        print(f"No entry in {path} starts with {sys.argv[2]!r}.")
        if near:
            print("Headings and entries containing it:")
        for i, kind, _, text in near:
            label = "heading" if kind == "h" else "entry"
            print(f"  line {i + 1}, {label}: {text[:100]}")
        sys.exit(1)

    for start, kind, level, _ in matches:
        end, fenced = len(lines), False
        for j in range(start + 1, len(lines)):
            line = lines[j]
            if line.lstrip().startswith(("```", "~~~")):
                fenced = not fenced
            if fenced:
                continue
            h = HEADING.match(line)
            if kind == "h" and h and len(h.group(1)) <= level:
                end = j
                break
            if kind == "i" and (h or (line and not line[0].isspace())):
                end = j
                break
        while end > start + 1 and not lines[end - 1].strip():
            end -= 1
        print(f"{path}:{start + 1}")
        print("\n".join(lines[start:end]))
        print()


if __name__ == "__main__":
    main()
