#!/usr/bin/env python3
"""Extract a pinned Detexify complete sample set; pass the upstream checkout path."""
import json
import math
from pathlib import Path
import subprocess
import sys
import struct
import re
from unicodeit.data import REPLACEMENTS

revision = "ba0742b03b01a7a958110ced23d509a72d85744e"
checkout = Path(sys.argv[1])
assert subprocess.check_output(["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True).strip() == revision
source = checkout / "packages/data/source"
out = Path(__file__).resolve().parents[1] / "assets/handwriting"
rejected = json.loads((source / "reviews/rejected-samples.json").read_text())["rejected"]

def normalize(strokes):
    segments = [(a, b, math.hypot(b['x']-a['x'], b['y']-a['y'])) for stroke in strokes for a, b in zip(stroke, stroke[1:])]
    segments = [(a,b,d) for a,b,d in segments if d > 0]
    total = sum(d for a,b,d in segments)
    points = [p for stroke in strokes for p in stroke]
    if not points: return None
    x0,x1 = min(p['x'] for p in points), max(p['x'] for p in points)
    y0,y1 = min(p['y'] for p in points), max(p['y'] for p in points)
    scale = max(x1-x0,y1-y0)
    if scale == 0: return [[0.,0.]]*32
    if total == 0:
        return [[round((points[i%len(points)]['x']-(x0+x1)/2)/scale,5),round((points[i%len(points)]['y']-(y0+y1)/2)/scale,5)] for i in range(32)]
    result=[]
    for i in range(32):
        target = total * (i + .5) / 32
        for a,b,d in segments:
            if target <= d:
                t=target/d
                result.append([round((a['x']+(b['x']-a['x'])*t-(x0+x1)/2)/scale,5), round((a['y']+(b['y']-a['y'])*t-(y0+y1)/2)/scale,5)])
                break
            target -= d
    assert len(result)==32
    return result

train,holdout=bytearray(),bytearray()
metadata=[]
unicode_map=dict(REPLACEMENTS)
unicode_map.update(json.loads((out/'typst-unicode-mappings.json').read_text()))
expressions = {
    r'\colonapprox': 'colon approx', r'\Colonapprox': 'colon.double approx',
    r'\colonsim': 'colon tilde.op', r'\Colonsim': 'colon.double tilde.op',
    r'\Eqcolon': 'minus colon.double', r'\Eqqcolon': 'eq colon.double',
    r'\idotsint': 'integral dots.h integral',
    r'\dotsint': 'integral dots.h integral',
}
for symbol in json.loads((source/'symbols.json').read_text())['symbols']:
    command=symbol['command']
    rows=[json.loads(line) for line in (source/symbol['samples']['path']).read_text().splitlines()]
    rows=[r for r in rows if r['id'] not in rejected]
    index=len(metadata)
    char=unicode_map.get(command, '')
    # ASCII punctuation can be math syntax (notably $, #, _, and brackets).
    # Text/IPA symbols must also stay upright rather than become math variables.
    typst=(json.dumps(char, ensure_ascii=False) if command.startswith('\\text')
           or any(ord(c) < 128 for c in char) or char == '√' else char) if char else None
    typst=expressions.get(command, typst)
    styled=re.fullmatch(r"\\math(cal|scr|frak|bb|ds|bf|it|sf|tt)\{([A-Za-z])\}",command)
    if styled:
        style,letter=styled.groups()
        function={'cal':'cal','scr':'scr','frak':'frak','bb':'bb','ds':'bb','bf':'bold','it':'italic','sf':'sans','tt':'mono'}[style]
        typst=f'{function}({letter})'
        if style == 'cal' and letter.isupper():
            char = dict(zip('BEFHILMR', 'ℬℰℱℋℐℒℳℛ')).get(letter, chr(0x1d49c + ord(letter) - ord('A')))
    if re.fullmatch(r'\\Up(delta|gamma|lambda|omega|phi|pi|psi|sigma|theta|upsilon|xi)', command):
        greek=command[3:].capitalize()
        typst=f'upright({greek})'
    name = command if typst and typst.startswith('"') else typst or command
    metadata.append({'char':char,'names':[name],'tex':command,'typst':typst,'detexify':True,'tex_only':not bool(typst), 'package':symbol.get('package')})
    for i,row in enumerate(rows):
        points=normalize(row['strokes'])
        if points:
            record=struct.pack('<H',index)+struct.pack('<64H',*[round(max(0,min(1,v+.5))*65535) for point in points for v in point])
            # Keep all accepted samples in production; evaluation omits every
            # tenth sample through a separate reference set.
            train.extend(record)
            if i%10==0: holdout.extend(record)
(out/'detexify-samples.bin').write_bytes(train)
(out/'detexify-holdout.bin').write_bytes(holdout)
(out/'detexify-symbols.json').write_text(json.dumps(metadata,ensure_ascii=False,separators=(',',':'))+'\n')
(out/'DETEXIFY-LICENSE').write_text((checkout/'LICENSE').read_text())
print(len(metadata),len(train)//130,len(holdout)//130, sum(not s['tex_only'] for s in metadata))
