#!/usr/bin/env python3
"""Generate deterministic TeX/Typst workloads for the LTeX JVM comparison.

Controlled mathematical prose, not quotations. Edit cohorts change either one
paragraph or all paragraphs, separating normal cache reuse from bulk rechecking.
"""
import argparse
import json
from pathlib import Path

SENTENCES = (
    "The estimate follows from the compactness of the embedding and the uniform bound.",
    "We now consider a sequence of smooth solutions on the bounded domain.",
    "The weak limit satisfies the energy inequality for almost every positive time.",
    "By interpolation, the resulting operator extends to the whole function space.",
    "This argument also applies to the corresponding nonautonomous evolution problem.",
    "The semigroup property gives the desired conclusion after integration in time.",
    "There exists a unique fixed point in the closed ball whenever the parameter is small.",
    "The constants depend only on the dimension and the regularity of the initial data.",
)


def source(language, paragraphs, stage, local_edit=False):
    body = ["This are a result. These estimates is uniform."]
    for index in range(paragraphs):
        if language == "typst" and index == paragraphs // 2:
            body.append("This are a result.")
        sentence = SENTENCES[index % len(SENTENCES)]
        paragraph_stage = stage if not local_edit or index == paragraphs // 2 else 0
        lead = f"At stage {paragraph_stage * 1000 + index + 1}, {sentence[0].lower() + sentence[1:]}"
        if language == "latex":
            formatted = r"The \emph{uniform bound} controls $\lVert u_n\rVert$ for every index."
            equation = (r"\[ \int_0^T \lVert u_n(t)\rVert^2\,dt \leq C, \]"
                        if index % 4 == 0 else "")
        else:
            formatted = "The _uniform bound_ controls $norm(u_n)$ for every index."
            equation = "$ integral_0^T norm(u_n(t))^2 dif t <= C, $" if index % 4 == 0 else ""
        body.append(lead + " " + formatted + "\n" + equation)
    if language == "typst":
        body.append("This are a result.")
    text = "\n\n".join(body) + "\n"
    if language == "latex":
        return ("\\documentclass{article}\n\\usepackage{amsmath}\n"
                "\\begin{document}\n" + text + "\\end{document}\n")
    return '#set text(lang: "en", region: "gb")\n\n' + text


def generate(directory, edits):
    directory.mkdir(parents=True, exist_ok=True)
    fixtures = []
    for language, extension in (("latex", "tex"), ("typst", "typ")):
        for size, paragraphs, local_edit in (("short", 2, False), ("long", 384, False),
                                             ("long-local", 384, True)):
            name = size + "-" + extension
            paths = []
            for stage in range(edits + 1):
                path = directory / f"{name}-{stage}.{extension}"
                path.write_text(source(language, paragraphs, stage, local_edit), encoding="utf-8")
                paths.append(path.name)
            fixtures.append(dict(name=name, language=language, source=paths[0], edits=paths[1:],
                                 min_diagnostics=1,
                                 required_rules=["THIS_NNS", "PLURAL_VERB_AFTER_THIS"]))
            if language == "typst":
                fixtures[-1]["required_diagnostic_lines"] = [
                    index for index, line in enumerate(
                        source(language, paragraphs, 0, local_edit).splitlines())
                    if line.startswith("This are a result.")
                ]
    manifest = directory / "fixtures.json"
    manifest.write_text(json.dumps(dict(version=1, fixtures=fixtures), indent=2) + "\n")
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--edits", type=int, default=10)
    arguments = parser.parse_args()
    if not 1 <= arguments.edits <= 100:
        parser.error("--edits must be between 1 and 100")
    print(generate(arguments.output, arguments.edits))
