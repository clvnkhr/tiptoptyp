"""Opt-in engine-only probe; no application or viewer latency claims.

Run with isolated pypdfium2/PyMuPDF binary wheels:
  python scripts/benchmark-pdf-candidates.py ENGINE A.pdf B.pdf > results.json
ENGINE is pdfium or mupdf. Alternate actual compiled revisions of equal length.
Read bytes before timing; reopen each iteration; render the middle page at 1800px
wide and extract its plain text. Five warmups, then 40 measured iterations.
No GUI, upload, output images, compilation time or full-document search included.
"""

import hashlib
import importlib.metadata
import json
import math
import platform
import statistics
import sys
import time
from pathlib import Path


def main():
    engine, *paths = sys.argv[1:]
    assert engine in ("mupdf", "pdfium") and len(paths) == 2
    buffers = [Path(p).read_bytes() for p in paths]
    assert buffers[0] != buffers[1], "Use two genuinely changed PDF revisions"
    if engine == "mupdf":
        import pymupdf as api
        version = api.VersionBind
        native_version = api.VersionFitz
    else:
        import pypdfium2 as api
        version = importlib.metadata.version("pypdfium2")
        native_version = str(api.PDFIUM_INFO)

    samples = []
    first = None
    dimensions = None
    for iteration in range(45):
        # Alternate revisions; process-global engine caches stay warm, documents do not.
        data = buffers[iteration % 2]
        t0 = time.perf_counter_ns()
        doc = api.open(stream=data, filetype="pdf") if engine == "mupdf" else api.PdfDocument(data)
        count = len(doc)
        page = doc[count // 2]
        t1 = time.perf_counter_ns()
        width = page.rect.width if engine == "mupdf" else page.get_width()
        scale = 1800 / width
        if engine == "mupdf":
            bitmap = page.get_pixmap(matrix=api.Matrix(scale, scale), alpha=False)
            dimensions = [bitmap.width, bitmap.height, bitmap.n]
        else:
            bitmap = page.render(scale=scale)
            dimensions = [bitmap.width, bitmap.height, bitmap.n_channels]
        t2 = time.perf_counter_ns()
        if engine == "mupdf":
            text = page.get_text("text")
        else:
            textpage = page.get_textpage()
            text = textpage.get_text_range()
            textpage.close()
        t3 = time.perf_counter_ns()
        assert ("Updated freely" if iteration % 2 else "Scroll freely") in text
        row = dict(open_page_ms=(t1-t0)/1e6, render_ms=(t2-t1)/1e6,
                   text_ms=(t3-t2)/1e6, total_ms=(t3-t0)/1e6)
        if first is None:
            first = row
        if iteration >= 5:
            samples.append(row)
        if engine == "pdfium":
            bitmap.close()
            page.close()
        del bitmap, page
        doc.close()
    summary = {}
    for key in samples[0]:
        values = sorted(s[key] for s in samples)
        summary[key] = {"p50": statistics.median(values), "p95": values[math.ceil(.95*len(values))-1]}
    print(json.dumps(dict(engine=engine, binding_version=version, native_version=native_version,
        platform=platform.platform(), python=sys.version, architecture=platform.machine(),
        pages=count, page_index=count//2, pixel_dimensions=dimensions, warmups=5,
        measured_iterations=40, source_paths=paths,
        sha256=[hashlib.sha256(b).hexdigest() for b in buffers],
        bytes=[len(b) for b in buffers], first_iteration=first,
        summary=summary, samples=samples,
        limitations="Engine-only binary-wheel probe; one simple Typst fixture family. Excludes compilation, disk reads, teardown, presentation, UI/text-layer construction, full search and memory measurement. First iteration excludes imports. Not comparable to browser end-to-end results. No visual fidelity verification."), indent=2))


if __name__ == "__main__":
    main()
