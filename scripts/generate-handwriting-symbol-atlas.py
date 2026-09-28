#!/usr/bin/env python3
"""Generate the standalone LaTeX atlas from a pinned Detexify checkout.

Usage: python3 scripts/generate-handwriting-symbol-atlas.py /path/to/checkout
"""
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REVISION = "ba0742b03b01a7a958110ced23d509a72d85744e"
WIDTH_MM, HEIGHT_MM = 18.0, 7.0
MAX_SAMPLE_POINTS = 32


def simplify_strokes(strokes):
    """Keep an easy-to-recognize trace while bounding TeX's path complexity."""
    points = [point for stroke in strokes for point in stroke]
    if sum(map(len, strokes)) <= MAX_SAMPLE_POINTS:
        return strokes

    x_values = [point["x"] for point in points]
    y_values = [point["y"] for point in points]
    epsilon = max(max(x_values) - min(x_values), max(y_values) - min(y_values)) * 0.008

    def simplify(path):
        if len(path) <= 2:
            return path
        first, last = path[0], path[-1]
        dx, dy = last["x"] - first["x"], last["y"] - first["y"]
        denom = dx * dx + dy * dy
        farthest, distance = 0, -1.0
        for index, point in enumerate(path[1:-1], 1):
            if denom == 0:
                dist = (point["x"] - first["x"]) ** 2 + (point["y"] - first["y"]) ** 2
            else:
                t = max(0.0, min(1.0, ((point["x"] - first["x"]) * dx + (point["y"] - first["y"]) * dy) / denom))
                proj_x, proj_y = first["x"] + t * dx, first["y"] + t * dy
                dist = (point["x"] - proj_x) ** 2 + (point["y"] - proj_y) ** 2
            if dist > distance:
                farthest, distance = index, dist
        if distance <= epsilon * epsilon:
            return [first, last]
        left = simplify(path[: farthest + 1])
        right = simplify(path[farthest:])
        return left[:-1] + right

    simplified = [simplify(stroke) for stroke in strokes]
    total = sum(map(len, simplified))
    if total <= MAX_SAMPLE_POINTS:
        return simplified

    # Preserve each pen lift and stroke endpoint, then spend the remaining
    # point budget proportionally on the simplified stroke interiors.
    minimum = sum(min(2, len(stroke)) for stroke in simplified)
    if minimum >= MAX_SAMPLE_POINTS:
        return [stroke[:1] for stroke in simplified[:MAX_SAMPLE_POINTS]]
    extra_budget = MAX_SAMPLE_POINTS - minimum
    extras = [max(0, len(stroke) - 2) for stroke in simplified]
    extra_total = sum(extras)
    allocations = [int(extra_budget * extra / extra_total) if extra_total else 0 for extra in extras]
    for index in sorted(range(len(extras)), key=lambda i: extras[i], reverse=True)[: extra_budget - sum(allocations)]:
        allocations[index] += 1
    result = []
    for stroke, extra in zip(simplified, allocations):
        budget = min(len(stroke), min(2, len(stroke)) + extra)
        if budget >= len(stroke):
            result.append(stroke)
        elif budget <= 1:
            result.append(stroke[:1])
        else:
            indices = sorted({round(i * (len(stroke) - 1) / (budget - 1)) for i in range(budget)})
            result.append([stroke[i] for i in indices])
    return result


def charcode(text):
    return "".join(f'\\char"{ord(char):X}\\relax{{}}\\allowbreak ' for char in text)


def font_coverage(families):
    coverage = {}
    try:
        for family, macro in families:
            font_file = subprocess.check_output(
                ["fc-match", "--format=%{file}", family], text=True
            ).strip()
            ranges = subprocess.check_output(
                ["fc-query", "--format=%{charset}", font_file], text=True
            ).split()
            codepoints = set()
            for item in ranges:
                if "-" in item:
                    first, last = (int(part, 16) for part in item.split("-", 1))
                    codepoints.update(range(first, last + 1))
                else:
                    codepoints.add(int(item, 16))
            coverage[family] = (macro, codepoints)
    except (FileNotFoundError, subprocess.CalledProcessError):
        # If fontconfig is unavailable, still generate a usable atlas using
        # its main math font; unsupported cells are called out by codepoint.
        return {families[0][0]: (families[0][1], set())}
    return coverage


def typst_output(text, fonts):
    rendered = []
    for char in text:
        codepoint = ord(char)
        family = next(
            (name for name, (_, points) in fonts.items() if codepoint in points),
            None,
        )
        if family is None:
            rendered.append(r"\typstmissing{" + f"{codepoint:04X}" + "}")
        else:
            macro, _ = fonts[family]
            rendered.append("{" + macro + r"\small " + charcode(char) + "}")
    return "".join(rendered)


def strokes_picture(strokes):
    strokes = simplify_strokes(strokes)
    points = [point for stroke in strokes for point in stroke]
    if not points:
        return r"\begin{tikzpicture}\end{tikzpicture}"
    x0, x1 = min(p["x"] for p in points), max(p["x"] for p in points)
    y0, y1 = min(p["y"] for p in points), max(p["y"] for p in points)
    width, height = x1 - x0, y1 - y0
    padding = 0.6
    scale = min((WIDTH_MM - 2 * padding) / max(width, 1e-9), (HEIGHT_MM - 2 * padding) / max(height, 1e-9))
    left = (WIDTH_MM - width * scale) / 2
    bottom = (HEIGHT_MM - height * scale) / 2
    paths = []
    for stroke in strokes:
        coords = []
        for point in stroke:
            x = left + (point["x"] - x0) * scale
            y = bottom + (y1 - point["y"]) * scale
            coords.append(f"({x:.2f}mm,{y:.2f}mm)")
        if len(coords) == 1:
            paths.append(r"\fill[black!62] " + coords[0] + r" circle[radius=.55pt];")
        else:
            paths.append(r"\draw[black!62,line width=.65pt,line cap=round,line join=round] " + " -- ".join(coords) + ";")
    return (
        r"\begin{tikzpicture}[baseline=-.4em]"
        + r"\path[use as bounding box] (0,0) rectangle (18mm,7mm);"
        + "".join(paths)
        + r"\end{tikzpicture}"
    )


def output(command, mode, package):
    if command == r"\\":
        # Show its effect in context: this command starts a new math line.
        return r"\ensuremath{\begin{gathered}a\\b\end{gathered}}"
    if command == r"\----":
        return r"---"
    if package == "tipa":
        return f"\\ensuremath{{{command}}}" if mode == "math" else command
    # TIPA redefines the math spacing and norm commands. Restore the standard
    # meanings around non-TIPA rows so the shared package environment does not
    # silently change unrelated TeX output.
    command = r"\begingroup\let\!\atlasMathBang\let\|\atlasMathVert " + command + r"\endgroup"
    return f"\\ensuremath{{{command}}}" if mode == "math" else command


def main():
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    checkout = Path(sys.argv[1])
    revision = subprocess.check_output(["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True).strip()
    if revision != REVISION:
        raise SystemExit(f"expected Detexify revision {REVISION}, got {revision}")
    source = checkout / "packages/data/source"
    upstream = json.loads((source / "symbols.json").read_text())["symbols"]
    rejected = set(json.loads((source / "reviews/rejected-samples.json").read_text())["rejected"])
    labels = json.loads((ROOT / "assets/handwriting/detexify-symbols.json").read_text())
    if len(upstream) != len(labels):
        raise SystemExit("Detexify definitions do not match the bundled symbol table")

    fonts = font_coverage([
        ("STIX Two Math", r"\atlasStixMath"),
        ("Arial Unicode MS", r"\atlasArialUnicode"),
        ("Apple Symbols", r"\atlasAppleSymbols"),
    ])
    mapped = sum(bool(label.get("typst")) for label in labels)
    private_use = sum(
        1
        for label in labels
        for char in (label.get("typst") or "")
        if 0xE000 <= ord(char) <= 0xF8FF
    )
    starts_with_backslash = sum(
        1 for label in labels
        if isinstance(label.get("typst"), str) and label["typst"].startswith("\\")
    )

    rows = []
    for symbol, label in zip(upstream, labels):
        accepted = [
            json.loads(line)
            for line in (source / symbol["samples"]["path"]).read_text().splitlines()
            if json.loads(line)["id"] not in rejected
        ]
        if not accepted:
            raise SystemExit(f"no accepted handwriting sample: {symbol['id']}")
        tex = symbol["command"]
        typst = label.get("typst")
        rows.append((
            strokes_picture(accepted[0]["strokes"]),
            charcode(tex),
            typst_output(typst, fonts) if typst is not None else r"\textcolor{gray}{—}",
            output(tex, symbol.get("mode", "math"), symbol.get("package")),
            symbol.get("package") or "core",
        ))

    preamble = r"""\documentclass[8pt,landscape]{extarticle}
\usepackage[margin=7mm]{geometry}
\usepackage{fontspec}
\setmainfont{STIX Two Text}
\newfontfamily\atlasStixMath{STIX Two Math}
\newfontfamily\atlasArialUnicode{Arial Unicode MS}
\newfontfamily\atlasAppleSymbols{Apple Symbols}
\setmonofont{Menlo}
\usepackage{tikz,longtable,array,booktabs}
\usepackage{amsmath,amssymb,amsfonts,mathtools}
\usepackage{mathrsfs,dsfont,bbold,upgreek,esint,stmaryrd,wasysym}
\let\atlasMathBang\!
\let\atlasMathVert\|
\usepackage{marvosym,tipa,textcomp,gensymb,cmll,latexsym,mathdots,skull}
\setlength{\tabcolsep}{1.2mm}
\setlength{\LTpre}{2pt}
\setlength{\LTpost}{2pt}
\setlength{\extrarowheight}{2pt}
\newcolumntype{S}{>{\raggedright\arraybackslash}p{18mm}}
\newcolumntype{C}{>{\raggedright\arraybackslash}p{35mm}}
\newcolumntype{T}{>{\raggedright\arraybackslash}p{44mm}}
\newcolumntype{O}{>{\centering\arraybackslash}p{20mm}}
\newcommand{\code}[1]{{\ttfamily\footnotesize #1}}
\newcommand{\typstmissing}[1]{{\normalfont\ttfamily\scriptsize\color{gray}[U+#1]}}
\newcommand{\pkg}[1]{{\tiny\color{gray}#1}}
\newcommand{\sample}[1]{#1}
\begin{document}
\pagestyle{plain}
\begin{center}
{\Large Handwriting symbol atlas}\quad{\small 1,123 Detexify definitions; one accepted sample per symbol}
\end{center}
\noindent Each drawing is the first accepted Detexify stroke sample, simplified for print while retaining pen lifts. “Typst insertion” is the current app mapping; — means no verified mapping. @SUMMARY@ TeX output is rendered with the listed package in this document’s shared TeX environment, so package/font variants may differ from another installation. Commands are shown literally in monospace. If a local font cannot show a mapped character, its cell gives the Unicode codepoint.\par\smallskip
\small
\begin{longtable}{@{}S C T O @{\hspace{2mm}} S C T O@{}}
\toprule
Sample & TeX command / package & Typst insertion & TeX output & Sample & TeX command / package & Typst insertion & TeX output\\
\midrule
\endfirsthead
\toprule
Sample & TeX command / package & Typst insertion & TeX output & Sample & TeX command / package & Typst insertion & TeX output\\
\midrule
\endhead
\bottomrule
\endfoot
"""
    body = []
    for index in range(0, len(rows), 2):
        cells = []
        for sample, tex, typst, tex_output, package in rows[index : index + 2]:
            cells.extend((
                sample,
                r"\code{" + tex + r"}\par\pkg{" + package + "}",
                typst,
                tex_output,
            ))
        if len(cells) == 4:
            cells.extend(("", "", "", ""))
        body.append(" & ".join(cells) + r" \\")
    destination = ROOT / "docs/handwriting-symbol-atlas.tex"
    summary = (
        f"{mapped:,} of {len(labels):,} entries have a mapping; "
        f"{len(labels) - mapped} are unmapped. {starts_with_backslash} Typst insertions begin with a backslash; "
        f"{private_use} uses a private-use Unicode codepoint."
    )
    destination.write_text(
        preamble.replace("@SUMMARY@", summary)
        + "\n".join(body)
        + "\n\\end{longtable}\n\\end{document}\n"
    )
    print(f"Wrote {destination} with {len(rows):,} entries")


if __name__ == "__main__":
    main()
