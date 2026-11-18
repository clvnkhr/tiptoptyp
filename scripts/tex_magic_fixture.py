"""Owned fake TeX engines for root/compiler routing, not TeX rendering accuracy."""
import json
import os
from pathlib import Path
import sys

MAGIC_MAIN = ('% !TeX program = xelatex\n\\documentclass{article}\n'
              '\\begin{document}\nRoot A.\n\\input{chapters/section}\n\\end{document}\n')
MAGIC_MAIN_EDITED = MAGIC_MAIN.replace('program = xelatex', 'program = pdflatex')
MAGIC_ALTERNATE = ('% !TeX TS-program = xelatex\n\\documentclass{article}\n'
                   '\\begin{document}\nRoot B.\n\\input{chapters/section}\n\\end{document}\n')
MAGIC_CHILD = '% !TeX root = ../main.tex\n% !TeX program = pdflatex\nChild body.\n'
MAGIC_CHILD_ALTERNATE = MAGIC_CHILD.replace('../main.tex', '../alternate.tex')

ENGINE = r'''#!PYTHON
import json
from pathlib import Path
import sys

root = Path(FIXTURE_ROOT)
engine = Path(sys.argv[0]).name
arguments = sys.argv[1:]
if '--version' in arguments or '-version' in arguments:
    print('Synthetic TeX magic engine ' + engine)
    raise SystemExit(0)
if engine not in ('pdflatex', 'xelatex'):
    raise SystemExit('Unexpected synthetic engine')
output = next((arg.split('=', 1)[1] for arg in arguments if arg.startswith('-output-directory=')), None)
source = next((Path(arg) for arg in reversed(arguments) if arg.endswith('.tex') and not arg.startswith('-')), None)
if output is None or source is None:
    raise SystemExit('Expected a direct engine invocation with managed output')
output, source = Path(output).resolve(), source.resolve()
if not output.is_relative_to(root) or not source.is_relative_to(root):
    raise SystemExit('Synthetic TeX request escaped its owned fixture')
data = source.read_bytes()
fingerprint = 0xcbf29ce484222325
for byte in data:
    fingerprint = ((fingerprint ^ byte) * 0x100000001b3) & 0xffffffffffffffff
digest = f'{fingerprint:016x}'

# A valid, bounded one-page PDF lets the real artifact/renderer pipeline run.
text = ('Synthetic TeX magic ' + engine + ' ' + digest).encode('ascii')
stream = b'BT /F1 12 Tf 20 150 Td (' + text + b') Tj ET\n'
objects = [
    b'<< /Type /Catalog /Pages 2 0 R >>',
    b'<< /Type /Pages /Count 1 /Kids [3 0 R] >>',
    b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 420 200] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>',
    b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
    b'<< /Length ' + str(len(stream)).encode() + b' >>\nstream\n' + stream + b'endstream',
    b'<< /Producer (SyntheticTeXMagic-' + engine.encode() + b') >>',
]
pdf, offsets = bytearray(b'%PDF-1.4\n'), [0]
for index, value in enumerate(objects, 1):
    offsets.append(len(pdf))
    pdf.extend(str(index).encode() + b' 0 obj\n' + value + b'\nendobj\n')
xref = len(pdf)
pdf.extend(b'xref\n0 7\n0000000000 65535 f \n')
for offset in offsets[1:]:
    pdf.extend(f'{offset:010d} 00000 n \n'.encode())
pdf.extend(b'trailer\n<< /Size 7 /Root 1 0 R /Info 6 0 R >>\nstartxref\n'
           + str(xref).encode() + b'\n%%EOF\n')
output.mkdir(exist_ok=True)
destination = output / source.with_suffix('.pdf').name
destination.write_bytes(pdf)
record = dict(engine=engine, input_name=source.name, source_fingerprint=digest,
              source_bytes=len(data), output=str(destination), arguments=arguments)
with (root / 'tex-magic-invocations.jsonl').open('a', encoding='utf-8') as log:
    log.write(json.dumps(record) + '\n')
print('Synthetic TeX magic artifact ready')
'''


def tex_magic_paths(directory):
    directory = Path(directory).resolve()
    project = directory / 'magic-project'
    return dict(project=project, main=project / 'main.tex',
                alternate=project / 'alternate.tex', child=project / 'chapters/section.tex',
                snippets=project / 'snippet.tex', typst=project / 'snippet.typ',
                engines=directory / 'tex-magic-engines',
                log=directory / 'tex-magic-invocations.jsonl')


def prepare_tex_magic_fixture(directory):
    directory = Path(directory).resolve()
    paths = tex_magic_paths(directory)
    paths['child'].parent.mkdir(parents=True)
    paths['main'].write_text(MAGIC_MAIN, encoding='utf-8')
    paths['alternate'].write_text(MAGIC_ALTERNATE, encoding='utf-8')
    paths['child'].write_text(MAGIC_CHILD, encoding='utf-8')
    paths['snippets'].write_text('', encoding='utf-8')
    paths['typst'].write_text('texroot', encoding='utf-8')
    paths['engines'].mkdir(mode=0o700)
    script = ENGINE.replace('#!PYTHON', '#!' + sys.executable, 1).replace('FIXTURE_ROOT', repr(str(directory)))
    for name in ('pdflatex', 'xelatex'):
        program = paths['engines'] / name
        program.write_text(script, encoding='utf-8')
        program.chmod(0o700)
    return paths


def tex_magic_environment(directory, environment=None):
    environment = dict(os.environ if environment is None else environment)
    environment['PATH'] = str(tex_magic_paths(directory)['engines']) + os.pathsep + environment.get('PATH', '')
    return environment


def tex_magic_invocations(directory):
    path = tex_magic_paths(directory)['log']
    if not path.exists():
        return []
    content = path.read_bytes()
    if len(content) > 256 * 1024:
        raise AssertionError('Synthetic TeX invocation log exceeded its bounded workload')
    # The single compiler worker may be completing its last atomic line write.
    records = []
    for line in content.splitlines(keepends=True):
        if not line.endswith(b'\n'):
            continue
        records.append(json.loads(line))
    return records
