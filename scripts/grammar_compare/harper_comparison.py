"""Developer comparison of ML text adapters over actual cached Harper inputs."""
from __future__ import annotations

from difflib import SequenceMatcher
import hashlib
import json
from pathlib import Path
import re
import statistics

from .comparison import canonical, load_corpus, save_report
from .harper_mapping import map_correction


def content_digest(value):
    """Stable content identity independent of JSON indentation or key ordering."""
    encoded = json.dumps(value, ensure_ascii=False, sort_keys=True,
                         separators=(',', ':')).encode('utf-8')
    return hashlib.sha256(encoded).hexdigest()


def make_request(fixtures_path: Path, output_path: Path, samples: int = 3):
    """Write the Rust probe's complete Request from the checked-in source fixtures."""
    if type(samples) is not int or not 1 <= samples <= 20:
        raise ValueError('Samples must be an integer between 1 and 20')
    fixtures = json.loads(fixtures_path.read_text())
    originals = fixtures.get('documents') if isinstance(fixtures, dict) else None
    if not isinstance(originals, list) or not originals:
        raise ValueError('Fixtures must contain a nonempty documents list')
    documents = []
    seen = set()
    for fixture in originals:
        if (not isinstance(fixture, dict) or not isinstance(fixture.get('id'), str)
                or not fixture['id'] or fixture.get('kind') not in ('tex', 'typst')):
            raise ValueError('Each fixture requires a nonempty id and tex/typst kind')
        source, expected = fixture.get('source'), fixture.get('expected_source')
        accepted = fixture.get('accepted_source', [])
        if (not isinstance(source, str) or not isinstance(expected, str)
                or not isinstance(accepted, list) or any(not isinstance(s, str) for s in accepted)):
            raise ValueError(f"{fixture['id']}: source, expected_source and accepted_source must contain strings")
        options = {key: fixture[key] for key in
                   ('language', 'dictionary', 'ignore_capitalized_names') if key in fixture}
        if ('language' in options and options['language'] not in ('auto', 'british', 'american')
                or 'dictionary' in options and not isinstance(options['dictionary'], str)
                or 'ignore_capitalized_names' in options
                and type(options['ignore_capitalized_names']) is not bool):
            raise ValueError(f"{fixture['id']}: invalid probe language/dictionary/name option")
        variants = [(fixture['id'], source)]
        if expected != source:
            variants.append((fixture['id'] + '--expected', expected))
        variants.extend((fixture['id'] + f'--accepted-{index}', value)
                        for index, value in enumerate(accepted))
        for identifier, value in variants:
            if identifier in seen:
                raise ValueError(f'Duplicate generated probe id: {identifier}')
            seen.add(identifier)
            documents.append(dict(id=identifier, kind=fixture['kind'], source=value, **options))
    if len(documents) > 4096:
        raise ValueError('Expanded probe request exceeds 4096 documents')
    request = dict(samples=samples, documents=documents)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(request, ensure_ascii=False, indent=2) + '\n')
    return dict(fixtures=len(originals), documents=len(documents), samples=samples,
                request=str(output_path.resolve()))


def eligible(block):
    projection=block['opaque']
    return any(offset is not None and ch.isalpha()
               for ch,offset in zip(projection['text'],projection['source_map']))


def propose_harper(source, projection, lints):
    """Illustrate compatible first suggestions; the app does not batch-apply these."""
    selected=[]; skipped=[]
    for lint in sorted(lints,key=lambda r:(r['priority'],r['start'],r['end'])):
        if not lint['shown_in_app'] or not lint['suggestions']:continue
        suggestion=lint['suggestions'][0]
        start,end=lint['start'],lint['end']
        indices=[i for i,m in enumerate(projection['source_map']) if m is not None and start<=m<end]
        if not indices:
            skipped.append(dict(lint=lint,reason='No editable projection span.'));continue
        a,b=min(indices),max(indices)+1
        mapped={m for m in projection['source_map'][a:b] if m is not None}
        if any(not source[p].isspace() and p not in mapped for p in range(start,end)):
            skipped.append(dict(lint=lint,reason='Suggestion spans hidden source syntax.'));continue
        if any(i in projection['protected_scalars'] for i in range(a,b)):
            skipped.append(dict(lint=lint,reason='Suggestion spans an opaque object.'));continue
        if suggestion['kind']=='insert_after':a=b
        if any(not (b<=x['start'] or a>=x['end']) for x in selected):
            skipped.append(dict(lint=lint,reason='Overlaps an earlier-priority suggestion.'));continue
        text=suggestion['text'] if suggestion['kind']!='remove' else ''
        selected.append(dict(start=a,end=b,text=text,lint=lint))
    output=projection['text']
    for edit in sorted(selected,key=lambda r:r['start'],reverse=True):
        output=output[:edit['start']]+edit['text']+output[edit['end']:]
    return dict(output=output,chosen_suggestions=selected,unapplied_suggestions=skipped,
                shown_lints=[r for r in lints if r['shown_in_app']])


def validate_export(document, source, label):
    """Refuse stale sources or malformed scalar geometry before ML evaluation."""
    if document.get('source') != source:
        raise ValueError(f'{label}: exported source differs from the fixture source')
    for index, block in enumerate(document['blocks']):
        start, end = block['source_start'], block['source_end']
        if (type(start) is not int or type(end) is not int
                or not 0 <= start <= end <= len(source)
                or block['source'] != source[start:end]):
            raise ValueError(f'{label} block {index}: invalid source slice')
        for name in ('opaque', 'prose'):
            if name not in block:
                if name == 'opaque':
                    raise ValueError(f'{label} block {index}: missing opaque projection')
                continue
            projection = block[name]
            result = map_correction(source, projection, projection.get('text', ''))
            if not result['all_safe']:
                raise ValueError(f'{label} block {index} {name}: invalid projection: '
                                 + '; '.join(r['reason'] for r in result['rejected']))
            if any(offset is not None and not start <= offset < end
                   for offset in projection['source_map']):
                raise ValueError(f'{label} block {index} {name}: mapped scalar outside block')


def aligned_reference(source, target, before, after):
    """Require a target block's boundaries to follow the same source region."""
    opcodes = SequenceMatcher(None, source, target, autojunk=False).get_opcodes()

    def boundary(position, is_start):
        candidates = set()
        for operation, a, b, c, d in opcodes:
            if operation == 'equal' and a <= position <= b:
                candidates.add(c + position - a)
            elif operation != 'equal':
                if a < position < b:
                    return None
                if a == b == position:
                    # A physical boundary insertion belongs to the block whose
                    # target boundary actually includes it. Exact range checking
                    # below still rejects a paragraph split/merge.
                    return c if is_start else d
                if position == a:
                    candidates.add(c)
                if position == b:
                    candidates.add(d)
        return next(iter(candidates)) if len(candidates) == 1 else None

    start = boundary(before['source_start'], True)
    end = boundary(before['source_end'], False)
    return (start is not None and end is not None
            and (start, end) == (after['source_start'], after['source_end']))


def prepare(fixtures_path:Path, probe_path:Path, corpus_path:Path, sidecar_path:Path):
    fixtures=json.loads(fixtures_path.read_text())
    probe=json.loads(probe_path.read_text())
    exported={d['id']:d for d in probe['documents']}
    if len(exported) != len(probe['documents']):
        raise ValueError('Probe contains duplicate document ids')
    for fixture in fixtures['documents']:
        source_pairs = [(fixture['id'], fixture['source'])]
        if fixture['expected_source'] != fixture['source']:
            source_pairs.append((fixture['id'] + '--expected', fixture['expected_source']))
        source_pairs.extend((fixture['id'] + f'--accepted-{index}', source)
                            for index, source in enumerate(fixture.get('accepted_source', [])))
        for identifier, source in source_pairs:
            if identifier not in exported:
                raise ValueError(f'Missing exported document: {identifier}')
            validate_export(exported[identifier], source, identifier)
    cases=[]; details={}; excluded=[]
    for fixture in fixtures['documents']:
        document=exported[fixture['id']]
        before=[b for b in document['blocks'] if eligible(b)]
        target_id=fixture['id']+'--expected' if fixture['expected_source']!=fixture['source'] else fixture['id']
        target=exported[target_id]
        after=[b for b in target['blocks'] if eligible(b)]
        alternatives=[]
        for index,_ in enumerate(fixture.get('accepted_source',[])):
            alternatives.append([b for b in exported[fixture['id']+f'--accepted-{index}']['blocks'] if eligible(b)])
        if not before:
            excluded.append(dict(id=fixture['id'],reason='No admitted prose characters in the production Harper tokens.',
                                 source=fixture['source'],notes=fixture['notes']))
        changed=[(a,b) for op,a,b,_,_ in SequenceMatcher(None,fixture['source'],fixture['expected_source'],autojunk=False).get_opcodes() if op!='equal']
        for index,block in enumerate(before):
            identifier=re.sub('[^a-z0-9_-]','-',fixture['id'].lower())+f'-p{index+1}'
            projection=block['opaque']
            notes=fixture['notes']+' Actual production Harper paragraph tokens were exported; opaque math/code objects are serialized as protected x for ML only. The string is not Harper\'s original input representation.'
            case=dict(id=identifier,category=fixture['category'],cohort=fixture['cohort'],kind='paragraph',
                      format=fixture['kind'],text=projection['text'],source=block['source'],notes=notes)
            if projection['protected_scalars']:case['protected']=['x']
            if len(before)==len(after):
                expected=after[index]['opaque']['text']
                alignment = aligned_reference(fixture['source'], fixture['expected_source'], block, after[index])
                intersects=any(a<=block['source_end'] and b>=block['source_start'] for a,b in changed)
                if not alignment:
                    case['notes']+=' Unscored: source/target paragraph boundaries do not align through the source repair.'
                elif expected==case['text'] and intersects and fixture['expected_source']!=fixture['source']:
                    case['notes']+=' Unscored: intended source correction is invisible to this parser/projection.'
                else:
                    case['expected']=expected
                    accepted=list(dict.fromkeys(a[index]['opaque']['text']
                        for source, a in zip(fixture.get('accepted_source', []), alternatives)
                        if len(a)==len(before) and aligned_reference(fixture['source'], source, block, a[index])))
                    accepted=[a for a in accepted if a not in (expected,case['text'])]
                    if accepted:case['accepted']=accepted
            else:
                case['notes']+=' Unscored: source repair changes admitted paragraph topology.'
            if 'citation' in fixture:
                citation=fixture['citation']
                case['citation']=dict(title='arXiv '+citation['paper'],url=citation['url'],
                    location=f"{citation['source_file']}:{citation['line_start']}")
            spelling=[r for r in document['harper_spell_only_lints']
                      if block['source_start']<=r['start'] and r['end']<=block['source_end']]
            details[identifier]=dict(document_id=fixture['id'],document_source=fixture['source'],
                block=block,projection=projection,single_paragraph=len(before)==1,timings=document['timings'],
                harper=propose_harper(fixture['source'],projection,block['harper_lints']),
                spelling=propose_harper(fixture['source'],projection,spelling))
            cases.append(case)
    corpus=dict(version=1,description='Actual production Harper cached paragraph/token exports adapted to ML text. Source-backed cases and generated fixtures are labelled separately. Opaque objects become protected x, not their original formulas. This is an integration challenge set, not a general benchmark.',
                fixtures_sha256=hashlib.sha256(fixtures_path.read_bytes()).hexdigest(),
                probe_sha256=hashlib.sha256(probe_path.read_bytes()).hexdigest(),cases=cases)
    corpus_path.parent.mkdir(parents=True,exist_ok=True)
    corpus_path.write_text(json.dumps(corpus,ensure_ascii=False,indent=2)+'\n')
    corpus_sha256 = hashlib.sha256(corpus_path.read_bytes()).hexdigest()
    sidecar_path.write_text(json.dumps(dict(version=1,corpus_sha256=corpus_sha256,
        corpus_content_sha256=content_digest(corpus),projection_contract=probe['projection_contract'],timing_contract=probe['timing_contract'],
        excluded_documents=excluded,details=details),ensure_ascii=False,indent=2)+'\n')
    load_corpus(corpus_path)
    return dict(documents=len(fixtures['documents']),paragraphs=len(cases),unique_inputs=len({c['text'] for c in cases}),
                scored_errors=sum(c.get('expected',c['text'])!=c['text'] for c in cases),
                correct_controls=sum(c.get('expected')==c['text'] for c in cases),
                unscored=sum('expected' not in c for c in cases),excluded_documents=len(excluded))


def finish(results_path:Path,sidecar_path:Path,output:Path):
    document=json.loads(results_path.read_text());sidecar=json.loads(sidecar_path.read_text())
    expected_hash = sidecar.get('corpus_sha256')
    if not expected_hash or document.get('corpus_sha256') != expected_hash:
        raise ValueError('Results corpus hash does not match the prepared sidecar')
    expected_content = sidecar.get('corpus_content_sha256')
    if not expected_content or content_digest(document.get('corpus')) != expected_content:
        raise ValueError('Results corpus content hash differs from the prepared full corpus')
    lookup=sidecar['details']
    for case in document['corpus']['cases']:
        details = lookup.get(case['id'])
        if details is None or case['text'] != details['projection']['text']:
            raise ValueError(f"{case['id']}: result input differs from the sidecar projection")
    assert {m['name'] for m in document['models']}=={'liquid','gector'}
    for name,label,field,timing in [
        ('harper','Harper curated · first compatible suggestions','harper','production_cold_cache_ms'),
        ('harper-spelling','Harper spelling-only · first compatible suggestions','spelling','spell_only_warm_suggestion_cache_ms'),
    ]:
        rows=[]
        for case in document['corpus']['cases']:
            details=lookup[case['id']];proposal=details[field]
            row=dict(id=case['id'],**proposal)
            if details['single_paragraph']:row['latency_ms']=statistics.median(details['timings'][timing])
            rows.append(row)
        document['models'].append(dict(name=name,label=label,results=rows,metadata=dict(
            harper_version='2.11.0',proposal_policy='Illustration only: first suggestion of each shown lint, sorted by priority then position, no overlap. The app currently exposes diagnostics; this is not an automatic app correction policy.',
            timing_contract=sidecar['timing_contract'],latency_scope='Only single-admitted-paragraph source documents contribute a timing. Curated uses full production cold-cache check including parsing/filtering; spelling-only excludes Document construction/parsing, so its timing is an isolated linter lower bound, not end-to-end.'
        )))
    for model in document['models']:
        safe=changed=0;reasons={}
        for row in model['results']:
            if row.get('error'):continue
            details=lookup[row['id']]
            mapping=map_correction(details['document_source'],details['projection'],row['output'])
            row['source_mapping']=mapping
            if row['output']!=details['projection']['text']:
                changed+=1;safe+=bool(mapping['all_safe'])
                for reason in mapping['rejected']:
                    label=reason.get('reason',str(reason)) if isinstance(reason,dict) else str(reason)
                    reasons[label]=reasons.get(label,0)+1
        model['metadata']['source_mapping']=dict(changed_outputs=changed,locally_mappable= safe,rejected_proposals=changed-safe,reasons=reasons,
             scope='Conservative scalar/source-geometry guard, not semantic or syntax correctness approval. Unsafe proposals leave document source unchanged. No source file is edited.')
    document['harper_adapter']=dict(projection_contract=sidecar['projection_contract'],timing_contract=sidecar['timing_contract'],
        excluded_documents=sidecar['excluded_documents'],fixture_scope='Existing source-backed cases/documents, generated boundary controls, single-word/context pairs, and4 exact arXiv TeX excerpts. Not a reconstruction of the94 normalized paper sentences.')
    save_report(document,output)
    return {m['name']:dict(summary=m['summary'],source_mapping=m['metadata']['source_mapping']) for m in document['models']}
