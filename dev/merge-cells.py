#!/usr/bin/env python3
"""Merges VHS's per-cell background rectangles into one per run.

VHS draws each colored cell as its own rectangle, which svgo turns into an
`M x y h w v h ... z` square in a path, so a filled row costs a square per
column. Squares that touch along a row, with the same height, become one wider
rectangle. A path holding anything else is left alone.

    merge-cells.py <file.svg>   rewrites the file in place
"""

import re
import sys

NUMBER = r"(-?[\d.]+)"
CELL = re.compile(rf"M{NUMBER}[ ,]{NUMBER}h{NUMBER}v{NUMBER}(?:H{NUMBER}|h-{NUMBER})z")
PATH_DATA = re.compile(r'(<path\b[^>]*?\bd=")([^"]*)(")')


def merge(data: str) -> str:
    cells = []
    end = 0
    for match in CELL.finditer(data):
        if match.start() != end:
            return data
        end = match.end()
        x, y, w, h = (float(match.group(i)) for i in range(1, 5))
        cells.append((x, y, w, h))
    if end != len(data) or not cells:
        return data

    runs = []
    for x, y, w, h in cells:
        if runs:
            rx, ry, rw, rh = runs[-1]
            if ry == y and rh == h and abs(rx + rw - x) < 1e-6:
                runs[-1] = (rx, ry, rw + w, rh)
                continue
        runs.append((x, y, w, h))

    def num(value: float) -> str:
        return f"{value:g}"

    return "".join(
        f"M{num(x)} {num(y)}h{num(w)}v{num(h)}h-{num(w)}z" for x, y, w, h in runs
    )


def main() -> None:
    path = sys.argv[1]
    with open(path, encoding="utf-8") as file:
        svg = file.read()
    svg = PATH_DATA.sub(lambda m: m.group(1) + merge(m.group(2)) + m.group(3), svg)
    with open(path, "w", encoding="utf-8") as file:
        file.write(svg)


if __name__ == "__main__":
    main()
