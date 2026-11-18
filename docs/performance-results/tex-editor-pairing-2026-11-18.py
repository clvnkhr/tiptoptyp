#!/usr/bin/env python3
"""Bounded PairingBuffer component probe; requires existing Cargo dev dependencies.

Run: python3 docs/performance-results/tex-editor-pairing-2026-11-18.py
Results are written to a fresh temporary directory. This does not launch a GUI,
run Cargo, change application settings, or modify production source files.
"""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parents[2]
SOURCES = ('auto_pairs.rs', 'auto_pairs/tex.rs', 'auto_pairs/indentation.rs',
           'delimiters.rs', 'delimiters/tex.rs', 'editor_math_guides.rs')
RUST_TEST = r'''
#[cfg(test)] mod component_measurement {
    use super::*;
    #[test] #[ignore] fn tex_pairing_cache_hit_and_miss() {
        let row = "\\newcommand{\\f}[1]{{#1}}\n\\begin{align}\n α+(β)&=γ\n\\end{align}\n";
        let fixture = row.repeat(1500);
        for (phase, enabled, changed) in [("baseline-disabled", false, true), ("enabled-miss", true, true), ("enabled-hit", true, false)] {
            let mut syntax = PairSyntax::default();
            let mut timings = Vec::new();
            for iteration in 0..65 {
                let mut source = format!("% {}\n{}\n", if changed { iteration } else { 0 }, fixture);
                let mut caret = CCursor::new(source.chars().count());
                let start = std::time::Instant::now();
                PairingBuffer::new(&mut source, &mut syntax, enabled, &[])
                    .with_document_kind(crate::document::DocumentKind::Tex)
                    .insert_text_at(&mut caret, "{", usize::MAX);
                let micros = start.elapsed().as_secs_f64() * 1e6;
                assert!(source.ends_with(if enabled { "{}" } else { "{" }));
                if iteration >= 5 { timings.push(micros); }
            }
            timings.sort_by(f64::total_cmp);
            println!("{{\"phase\":\"{}\",\"fixture_bytes\":{},\"samples\":{},\"median_us\":{},\"p95_us\":{},\"index_builds\":{}}}",
                phase, fixture.len(), timings.len(), timings[30], timings[57], syntax.1.as_ref().map_or(0, |s| s.builds));
        }
    }
}
'''
ADAPTER = '''#![allow(dead_code)]
mod document { pub use tiptoptyp_core::document::*; }
mod rainbow {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum BracketFamily { Round, Square, Curly, Mixed }
}
mod editor_math_guides;
mod delimiters;
mod auto_pairs;
'''


def run():
    output = Path(tempfile.mkdtemp(prefix='ttt-tex-pairing-'))
    dependencies = ROOT / 'target/debug/deps'
    manifest = tomllib.loads((ROOT / 'Cargo.toml').read_text())
    compiler = os.environ.get('RUSTC') or shutil.which('rustc')
    if Path('/opt/homebrew/bin/rustc').is_file() and 'RUSTC' not in os.environ:
        compiler = '/opt/homebrew/bin/rustc'
    if not compiler:
        raise RuntimeError('Rust compiler is unavailable')
    hashes = {}
    for name in SOURCES:
        source = ROOT / 'src' / name
        destination = output / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        hashes[name] = hashlib.sha256(source.read_bytes()).hexdigest()
    with (output / 'auto_pairs.rs').open('a') as destination:
        destination.write(RUST_TEST)
    (output / 'main.rs').write_text(ADAPTER)
    command = [compiler, '--edition=2024', '-C', 'opt-level=3', '--test', str(output / 'main.rs'),
               '-L', 'dependency=' + str(dependencies), '-o', str(output / 'probe')]
    artifacts = {}
    for name in ('eframe', 'typst_syntax', 'tiptoptyp_core'):
        candidates = list(dependencies.glob('lib' + name + '-*.rlib'))
        if name == 'typst_syntax':
            version = manifest['dependencies']['typst-syntax']
            if not isinstance(version, str):
                version = version['version']
            version = version.lstrip('=^~')
            candidates = [path for path in candidates
                          if ('/typst-syntax-' + version + '/') in
                          path.with_name(path.name[3:-5] + '.d').read_text()]
        if not candidates:
            raise RuntimeError('Build the Cargo development dependencies before probing ' + name)
        artifact = max(candidates, key=lambda path: path.stat().st_mtime)
        artifacts[name] = str(artifact)
        command.extend(['--extern', name + '=' + str(artifact)])
    compiled = subprocess.run(command, capture_output=True, text=True)
    (output / 'compile.log').write_text(compiled.stdout + compiled.stderr)
    compiled.check_returncode()
    measured = subprocess.run([str(output / 'probe'), 'auto_pairs::component_measurement::tex_pairing_cache_hit_and_miss',
                               '--exact', '--ignored', '--nocapture', '--test-threads=1'],
                              capture_output=True, text=True)
    (output / 'performance.log').write_text(measured.stdout + measured.stderr)
    measured.check_returncode()
    rows = [json.loads(line[line.index('{"phase"'):]) for line in measured.stdout.splitlines()
            if '{"phase"' in line]
    assert len(rows) == 3
    report = dict(scope='One PairingBuffer insertion only; excludes the highlighter, EditorDerivedData and GUI layout/rendering',
                  host=platform.platform(), compiler=subprocess.check_output([compiler, '--version'], text=True).strip(),
                  flags='--edition=2024 -C opt-level=3 --test', viewport_theme='not applicable: component operation',
                  fixture='1500 generated TeX rows: 96000 UTF-8 bytes plus iteration comment header and final newline',
                  warmup_per_phase=5, samples_per_phase=60,
                  workload='Insert one opening curly at EOF; changed-header iterations measure misses; resetting unchanged source measures hits',
                  baseline='Previous native TeX pairing gate disabled: literal insertion',
                  after='Native TeX pairing enabled: opening curly inserts its matching closer',
                  measurements=rows, source_hashes=hashes, adapter_sha256=hashlib.sha256(ADAPTER.encode()).hexdigest(),
                  dependency_artifacts=artifacts, compile_command=command,
                  limitations='Local component feature overhead, not a whole-editor or cross-platform speed guarantee; identical cached development dependency rlibs are reused for every phase')
    (output / 'performance.json').write_text(json.dumps(report, indent=2) + '\n')
    print(output)
    print(json.dumps(rows, indent=2))


if __name__ == '__main__':
    run()
