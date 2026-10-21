"""Fast adapter/export plumbing checks; no Rust build or ML packages required."""
import copy
import importlib.util
import io
import json
from contextlib import redirect_stderr
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0,str(Path(__file__).resolve().parent))
from grammar_compare.harper_comparison import aligned_reference, finish, make_request, prepare, propose_harper, validate_export
from grammar_compare.comparison import render_report


def block(text,source=None,start=0):
    source=text if source is None else source
    return dict(source_start=start,source_end=start+len(source),source=source,
                opaque=dict(text=text,source_map=list(range(start,start+len(text))),protected_scalars=[]),harper_lints=[])


class HarperComparisonTests(unittest.TestCase):
    def test_prepare_uses_exported_projection_and_expected_source_not_handwritten_text(self):
        with tempfile.TemporaryDirectory() as temp:
            p=Path(temp)
            fixture=dict(id='case',kind='tex',source='These is wrong.',expected_source='These are wrong.',
                         category='agreement',cohort='test',notes='Generated source fixture.')
            docs=[dict(id='case',source=fixture['source'],blocks=[block('These is wrong.')],harper_spell_only_lints=[],timings={}),
                  dict(id='case--expected',source=fixture['expected_source'],blocks=[block('These are wrong.')],harper_spell_only_lints=[],timings={})]
            (p/'fixtures.json').write_text(json.dumps(dict(documents=[fixture])))
            (p/'probe.json').write_text(json.dumps(dict(documents=docs,projection_contract='Actual exported tokens.',timing_contract='Not a timing test.')))
            counts=prepare(p/'fixtures.json',p/'probe.json',p/'corpus.json',p/'sidecar.json')
            case=json.loads((p/'corpus.json').read_text())['cases'][0]
            self.assertEqual(case['text'],'These is wrong.')
            self.assertEqual(case['expected'],'These are wrong.')
            self.assertEqual(counts['scored_errors'],1)

    def test_no_admitted_prose_is_a_coverage_exclusion_not_a_successful_control(self):
        with tempfile.TemporaryDirectory() as temp:
            p=Path(temp)
            fixtures=[dict(id='opaque',kind='typst',source='$ text("tyop") $',expected_source='$ text("tyop") $',category='opaque',cohort='test',notes='Math text is opaque.'),
                      dict(id='valid',kind='tex',source='Valid.',expected_source='Valid.',category='unchanged',cohort='test',notes='Valid.')]
            hidden=block('x',fixtures[0]['source']);hidden['opaque']['source_map']=[None];hidden['opaque']['protected_scalars']=[0]
            docs=[dict(id='opaque',source=fixtures[0]['source'],blocks=[hidden]),dict(id='valid',source=fixtures[1]['source'],blocks=[block('Valid.')],harper_spell_only_lints=[],timings={})]
            (p/'fixtures.json').write_text(json.dumps(dict(documents=fixtures)))
            (p/'probe.json').write_text(json.dumps(dict(documents=docs,projection_contract='Opaque.',timing_contract='Not timed.')))
            counts=prepare(p/'fixtures.json',p/'probe.json',p/'corpus.json',p/'sidecar.json')
            self.assertEqual((counts['paragraphs'],counts['excluded_documents']),(1,1))

    def test_first_suggestions_are_only_from_shown_lints_and_cannot_cross_markup(self):
        source='The \\emph{tyop} remains.'
        projection=dict(text='The tyop remains.',source_map=[0,1,2,None,10,11,12,13,None,16,17,18,19,20,21,22,23],protected_scalars=[])
        # Use a correct explicit map for the admitted text, with syntax omitted.
        projection['source_map']=[0,1,2,None,*range(source.index('tyop'),source.index('tyop')+4),None,*range(source.index('remains'),len(source))]
        start=source.index('tyop')
        lints=[dict(start=start,end=start+4,priority=10,shown_in_app=True,suggestions=[dict(kind='replace',text='typo')]),
               dict(start=0,end=len(source),priority=20,shown_in_app=True,suggestions=[dict(kind='replace',text='Discard the macro.')]),
               dict(start=start,end=start+4,priority=1,shown_in_app=False,suggestions=[dict(kind='replace',text='wrong')])]
        result=propose_harper(source,projection,lints)
        self.assertEqual(result['output'],'The typo remains.')
        self.assertEqual(len(result['unapplied_suggestions']),1)


    def test_report_exposes_guard_rejections_and_escapes_model_diagnostics(self):
        attack='<script>bad()</script>'
        case=dict(id='safe',category='unchanged',format='tex',text='Valid.',expected='Valid.')
        row=dict(id='safe',output='Valid.',source_mapping=dict(all_safe=False,edits=[],rejected=[dict(reason=attack)]),shown_lints=[dict(message=attack)])
        document=dict(version=1,corpus=dict(cases=[case]),models=[dict(name='fixture',results=[row])],harper_adapter=dict(projection_contract='Actual token export.'))
        rendered=render_report(document)
        self.assertIn('Source edit rejected',rendered)
        self.assertIn('1 Harper diagnostics',rendered)
        self.assertIn('Harper receives actual production paragraph',rendered)
        self.assertNotIn(attack,rendered)
        self.assertIn('&lt;script&gt;',rendered)


    def test_harper_report_distinguishes_timing_scopes_and_source_guard(self):
        case=dict(id='safe',category='unchanged',format='tex',text='Valid.',expected='Valid.')
        row=dict(id='safe',output='Valid.',latency_ms=0.025)
        model=dict(name='harper-spelling',results=[row],metadata=dict(source_mapping=dict(
            rejected_proposals=2,changed_outputs=3)))
        document=dict(version=1,corpus=dict(cases=[case]),models=[model],harper_adapter=dict(
            projection_contract='Actual tokens.'))
        rendered=render_report(document)
        self.assertIn('0.025 ms',rendered)
        self.assertIn('median timed check',rendered)
        self.assertIn('Source guard rejected 2/3',rendered)
        self.assertIn('These scopes are not equivalent.',rendered)
        self.assertNotIn('<td>projected math</td>',rendered)
        self.assertNotIn('Latency covers uncached model inference on all',rendered)

    def test_export_validation_rejects_stale_source_and_invalid_map(self):
        source='Proof.'
        with self.assertRaisesRegex(ValueError,'differs'):
            validate_export(dict(source='Other.',blocks=[]),source,'stale')
        for offsets in ([0,0,2,3,4,5],[0,1,2,3,4,99]):
            damaged=block(source);damaged['opaque']['source_map']=offsets
            with self.subTest(offsets=offsets),self.assertRaisesRegex(ValueError,'invalid projection'):
                validate_export(dict(source=source,blocks=[damaged]),source,'invalid')
        damaged=block('Other.')
        with self.assertRaisesRegex(ValueError,'invalid source slice'):
            validate_export(dict(source=source,blocks=[damaged]),source,'slice')

    def test_global_scalar_maps_cannot_point_outside_the_exported_block(self):
        source='one two'
        wrong=block('one',start=4);wrong['source']='two'
        wrong['opaque']['source_map']=[0,1,2]
        with self.assertRaisesRegex(ValueError,'outside block'):
            validate_export(dict(source=source,blocks=[wrong]),source,'outside')

    def test_reference_alignment_accepts_local_edit_and_physical_edge_insertions(self):
        for source,target in [('This are valid.','This is valid.'),('proof','A proof.'),
                              ('😀 proof are valid.','😀 proof is valid.')]:
            with self.subTest(source=source):
                self.assertTrue(aligned_reference(source,target,block(source),block(target)))
        source='First.\n\nThis are valid.';target=source.replace(' are ',' is ')
        self.assertTrue(aligned_reference(source,target,block('This are valid.',start=8),
                                          block('This is valid.',start=8)))

    def test_reference_boundary_inside_replaced_source_is_ambiguous(self):
        self.assertFalse(aligned_reference('abcdef','XY',block('cdef',start=2),block('XY')))

    def test_same_count_split_and_merge_are_not_paired_by_paragraph_index(self):
        with tempfile.TemporaryDirectory() as temp:
            p=Path(temp)
            source='A sentence.\n\nAnother sentence.'
            target='A.\n\nsentence. Another sentence.'
            fixture=dict(id='topology',kind='tex',source=source,expected_source=target,
                         category='boundary',cohort='test',notes='Split and merge keeps paragraph count.')
            docs=[dict(id='topology',source=source,
                       blocks=[block('A sentence.'),block('Another sentence.',start=13)],
                       harper_spell_only_lints=[],timings={}),
                  dict(id='topology--expected',source=target,
                       blocks=[block('A.'),block('sentence. Another sentence.',start=4)],
                       harper_spell_only_lints=[],timings={})]
            (p/'fixtures.json').write_text(json.dumps(dict(documents=[fixture])))
            (p/'probe.json').write_text(json.dumps(dict(documents=docs,projection_contract='Actual.',timing_contract='None.')))
            counts=prepare(p/'fixtures.json',p/'probe.json',p/'corpus.json',p/'sidecar.json')
            self.assertEqual(counts['unscored'],2)
            self.assertTrue(all('boundaries do not align' in row['notes']
                                for row in json.loads((p/'corpus.json').read_text())['cases']))

    def test_accepted_exports_are_checked_against_their_own_source(self):
        with tempfile.TemporaryDirectory() as temp:
            p=Path(temp)
            fixture=dict(id='case',kind='tex',source='These is wrong.',expected_source='These are wrong.',
                         accepted_source=['This is wrong.'],category='agreement',cohort='test',notes='Generated.')
            docs=[dict(id='case',source=fixture['source'],blocks=[block(fixture['source'])]),
                  dict(id='case--expected',source=fixture['expected_source'],blocks=[block(fixture['expected_source'])]),
                  dict(id='case--accepted-0',source='Stale source.',blocks=[])]
            (p/'fixtures.json').write_text(json.dumps(dict(documents=[fixture])))
            (p/'probe.json').write_text(json.dumps(dict(documents=docs,projection_contract='Actual.',timing_contract='None.')))
            with self.assertRaisesRegex(ValueError,'accepted-0.*differs'):
                prepare(p/'fixtures.json',p/'probe.json',p/'corpus.json',p/'sidecar.json')


    def test_request_expands_references_and_preserves_all_probe_options(self):
        with tempfile.TemporaryDirectory() as temp:
            p=Path(temp)
            fixtures=[dict(id='edited',kind='tex',source='Bad.',expected_source='Good.',
                           accepted_source=['Better.','Bad.'],language='american',
                           dictionary='Miao\nXu',ignore_capitalized_names=True),
                      dict(id='control',kind='typst',source='Valid.',expected_source='Valid.',
                           ignore_capitalized_names=False)]
            (p/'fixtures.json').write_text(json.dumps(dict(documents=fixtures)))
            counts=make_request(p/'fixtures.json',p/'request.json',samples=20)
            request=json.loads((p/'request.json').read_text())
            self.assertEqual(request['samples'],20)
            self.assertEqual(counts['documents'],5)
            self.assertEqual([d['id'] for d in request['documents']],
                             ['edited','edited--expected','edited--accepted-0','edited--accepted-1','control'])
            for row in request['documents'][:4]:
                self.assertEqual((row['language'],row['dictionary'],row['ignore_capitalized_names']),
                                 ('american','Miao\nXu',True))
            self.assertEqual([d['source'] for d in request['documents']],['Bad.','Good.','Better.','Bad.','Valid.'])
            self.assertFalse(request['documents'][-1]['ignore_capitalized_names'])
            for invalid in (0,21,-1,True,1.5):
                with self.subTest(invalid=invalid),self.assertRaisesRegex(ValueError,'between 1 and 20'):
                    make_request(p/'fixtures.json',p/'request.json',invalid)

    def test_request_rejects_generated_id_collisions(self):
        with tempfile.TemporaryDirectory() as temp:
            p=Path(temp)
            fixtures=[dict(id='case',kind='tex',source='Bad.',expected_source='Good.'),
                      dict(id='case--expected',kind='tex',source='Other.',expected_source='Other.')]
            (p/'fixtures.json').write_text(json.dumps(dict(documents=fixtures)))
            with self.assertRaisesRegex(ValueError,'Duplicate generated probe id'):
                make_request(p/'fixtures.json',p/'request.json')

    def test_request_cli_has_checked_in_fixture_default_and_bounded_samples(self):
        script=Path(__file__).parent/'compare-harper-grammar.py'
        spec=importlib.util.spec_from_file_location('harper_cli',script)
        module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
        parsed=module.arguments(['request','--output','request.json','--samples','1'])
        self.assertEqual(parsed.samples,1)
        self.assertEqual(parsed.fixtures,Path(__file__).resolve().parent/'grammar_compare/harper-fixtures.json')
        for value in ('0','21','oops'):
            with self.subTest(value=value),redirect_stderr(io.StringIO()),self.assertRaises(SystemExit):
                module.arguments(['request','--output','request.json','--samples',value])

    def test_finish_binds_results_to_prepared_corpus_and_projection(self):
        with tempfile.TemporaryDirectory() as temp:
            p=Path(temp)
            source='Valid.'
            fixture=dict(id='case',kind='tex',source=source,expected_source=source,
                         category='unchanged',cohort='test',notes='Generated.')
            probe=dict(documents=[dict(id='case',source=source,blocks=[block(source)],
                                      harper_spell_only_lints=[],timings=dict(
                                          production_cold_cache_ms=[1],spell_only_warm_suggestion_cache_ms=[.1]))],
                       projection_contract='Actual.',timing_contract='Test timing.')
            (p/'fixtures.json').write_text(json.dumps(dict(documents=[fixture])))
            (p/'probe.json').write_text(json.dumps(probe))
            prepare(p/'fixtures.json',p/'probe.json',p/'corpus.json',p/'sidecar.json')
            sidecar=json.loads((p/'sidecar.json').read_text())
            corpus=json.loads((p/'corpus.json').read_text())
            result=dict(version=1,corpus=corpus,corpus_sha256=sidecar['corpus_sha256'],
                        models=[dict(name=name,metadata={},results=[dict(id='case-p1',output=source)])
                                for name in ('liquid','gector')])
            (p/'results.json').write_text(json.dumps(result))
            self.assertEqual(set(finish(p/'results.json',p/'sidecar.json',p/'report')),
                             {'liquid','gector','harper','harper-spelling'})
            stale=copy.deepcopy(sidecar);stale['corpus_sha256']='stale'
            (p/'bad-sidecar.json').write_text(json.dumps(stale))
            with self.assertRaisesRegex(ValueError,'corpus hash'):
                finish(p/'results.json',p/'bad-sidecar.json',p/'rejected')
            changed=copy.deepcopy(result);changed['corpus']['cases'][0]['expected']='Altered.'
            (p/'altered-results.json').write_text(json.dumps(changed))
            with self.assertRaisesRegex(ValueError,'content hash'):
                finish(p/'altered-results.json',p/'sidecar.json',p/'rejected')
            stale=copy.deepcopy(sidecar);stale['details']['case-p1']['projection']['text']='Wrong.'
            (p/'bad-sidecar.json').write_text(json.dumps(stale))
            with self.assertRaisesRegex(ValueError,'input differs'):
                finish(p/'results.json',p/'bad-sidecar.json',p/'rejected')


if __name__=='__main__':unittest.main()
