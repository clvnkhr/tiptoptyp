#!/usr/bin/env python3
"""Export verified symbol names from the bundled Typst CLI (no runtime probing)."""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
compiler = sys.argv[1]

deprecated = set()
def evaluate(expression):
    result = subprocess.run([compiler, 'eval', expression], text=True, capture_output=True, check=True)
    deprecated.update(re.findall(r'warning: `([^`]+)` is deprecated', result.stderr))
    return json.loads(result.stdout)

def unquote(literal):
    literal = re.sub(r'\\u\{([0-9a-fA-F]+)\}',
                     lambda m: json.dumps(chr(int(m[1], 16)), ensure_ascii=False)[1:-1], literal)
    return json.loads(literal)

def normalized(glyph):
    return glyph.replace('\ufe0e', '').replace('\ufe0f', '')

aliases = {}
for name, representation in evaluate('dictionary(sym).pairs().map(((name, value)) => (name, repr(value)))'):
    tokens = re.findall(r'"(?:\\.|[^"\\])*"', representation)
    values = list(map(unquote, tokens))
    if len(values) % 2:
        aliases[name] = values.pop(0)
    for modifier, glyph in zip(values[::2], values[1::2]):
        aliases[name + ('.' + modifier if modifier else '')] = glyph
# Verify every exported name against the compiler, not just its printed form.
actual = evaluate('(' + ','.join('str(sym.' + name + ')' for name in aliases) + ',)')
assert actual == list(aliases.values())
aliases = {name: glyph for name, glyph in aliases.items() if name not in deprecated}
canonical = {}
for name in sorted(aliases, key=lambda name: (len(name), name)):
    canonical.setdefault(normalized(aliases[name]), name)
# Preserve the model's familiar canonical names when the current compiler verifies them.
for symbol in json.loads((ROOT / 'assets/handwriting/symbols.json').read_text()):
    for name in symbol['names']:
        if name in aliases and normalized(aliases[name]) == normalized(symbol['char']):
            canonical[normalized(symbol['char'])] = name
            break
path = ROOT / 'assets/handwriting/typst-symbol-names.json'
path.write_text(json.dumps(canonical, ensure_ascii=False, sort_keys=True, separators=(',', ':')) + '\n')
print(f'{len(canonical)} verified glyph names; {path.stat().st_size} bytes')
