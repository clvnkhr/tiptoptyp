#![allow(dead_code)]
use std::{hint::black_box, sync::Arc, time::Instant};

mod editor_math_guides {
    include!(__INLINE__);
}
mod before {
    include!(__BEFORE_HELPERS__);
    #[path=__BEFORE_TABLE__]
    pub(crate) mod tex_table;
}
#[cfg(indexed_probe)]
mod after {
    include!(__AFTER_HELPERS__);
    #[path=__AFTER_TABLE__]
    pub(crate) mod tex_table;
}
#[cfg(not(indexed_probe))]
type Table = before::tex_table::TexTable;
#[cfg(indexed_probe)]
type Table = after::tex_table::TexTable;

#[derive(Clone, Copy)]
struct Caret {
    character: usize,
    byte: usize,
}
struct Case {
    name: &'static str,
    source: Arc<str>,
    starts: Vec<usize>,
    carets: Vec<Caret>,
}
impl Case {
    fn new(name: &'static str, source: String, byte_carets: Vec<usize>) -> Self {
        let mut starts: Vec<usize> = source.char_indices().map(|(at, _)| at).collect();
        starts.push(source.len());
        let carets = byte_carets
            .into_iter()
            .map(|byte| {
                assert!(source.is_char_boundary(byte));
                Caret {
                    character: starts.partition_point(|at| *at < byte),
                    byte,
                }
            })
            .collect();
        Self {
            name,
            source: source.into(),
            starts,
            carets,
        }
    }
}
struct Runner {
    source: Arc<str>,
    previous: Option<usize>,
    table: Option<Table>,
    #[cfg(indexed_probe)]
    index: after::tex_table::TexTableIndex,
}
impl Runner {
    fn new(case: &Case) -> Self {
        Self {
            source: Arc::clone(&case.source),
            previous: None,
            table: None,
            #[cfg(indexed_probe)]
            index: after::tex_table::TexTableIndex::new(Arc::clone(&case.source), &case.starts),
        }
    }
    fn query(&mut self, caret: Caret) -> (usize, usize, usize) {
        // The existing last-caret cache belongs to both routes. Changed-caret
        // sweeps expose the cost beyond that fast path; stable-caret is control.
        if self.previous != Some(caret.character) {
            #[cfg(not(indexed_probe))]
            {
                self.table = before::tex_table::tex_table_at(&self.source, caret.character);
            }
            #[cfg(indexed_probe)]
            {
                if let after::tex_table::TableSelectionUpdate::Changed(id) =
                    self.index.update(caret.byte)
                {
                    self.table = id.and_then(|id| self.index.parse(id));
                }
            }
            self.previous = Some(caret.character);
        }
        self.table.as_ref().map_or((usize::MAX, 0, 0), |table| {
            (
                table.source_range.start,
                table.source_range.end,
                table.source.len(),
            )
        })
    }
    fn check(&mut self, case: &Case) {
        for &caret in &case.carets {
            self.query(caret);
            let expected = before::tex_table::tex_table_at(&case.source, caret.character);
            assert_eq!(
                format!("{:?}", self.table),
                format!("{expected:?}"),
                "{} at {}",
                case.name,
                caret.character
            );
        }
    }
}
fn fixtures() -> Vec<Case> {
    let prose = "% α Unicode fixture\nPlain prose, punctuation and text.\n";
    let plain = prose.repeat(1_048_576 / prose.len());
    let starts: Vec<_> = plain.char_indices().map(|(at, _)| at).collect();
    let plain_carets = (0..32)
        .map(|i| starts[(i + 1) * (starts.len() - 1) / 33])
        .collect();
    let table = "\\begin{tabular}{cc}\n α & β \\\\\n c & d \\\\\n\\end{tabular}";
    let prefix = format!("\\usepackage{{array}}\n{}", plain);
    let one = format!("{prefix}{table}\n");
    let table_starts: Vec<_> = table
        .char_indices()
        .map(|(at, _)| at + prefix.len())
        .collect();
    let inside = (0..32)
        .map(|i| table_starts[1 + i % (table_starts.len() - 2)])
        .collect::<Vec<_>>();
    let sibling = format!("{table}\n");
    let siblings = format!(
        "\\RequirePackage{{array,siunitx}}\n{}",
        sibling.repeat(5000)
    );
    let header = siblings.len() - sibling.len() * 5000;
    let cell_offset = sibling.find('α').unwrap();
    let sibling_carets = (0..32)
        .map(|i| header + ((i * 157) % 5000) * sibling.len() + cell_offset)
        .collect();
    let nested = format!(
        "{}\\begin{{tabular}}{{c}}\\begin{{tabular}}{{\\dynamic}}unsupported payload\\end{{tabular}}\\end{{tabular}}",
        prose.repeat(4096)
    );
    let nested_start = nested.find("unsupported payload").unwrap();
    let nested_carets = (0..32).map(|i| nested_start + i % 19).collect();
    let stable = vec![table_starts[5]; 32];
    vec![
        Case::new("no-table-1MiB", plain, plain_carets),
        Case::new("inside-one-table-1MiB", one.clone(), inside),
        Case::new("many-sibling-tables", siblings, sibling_carets),
        Case::new("unsupported-inner-table", nested, nested_carets),
        Case::new("stable-caret-control", one, stable),
    ]
}
fn percentile(samples: &[f64]) -> (f64, f64) {
    let mut samples = samples.to_vec();
    samples.sort_by(f64::total_cmp);
    (samples[samples.len() / 2], samples[samples.len() * 9 / 10])
}
fn write_fresh(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .expect("benchmark artifacts must be new files");
    file.write_all(bytes).unwrap();
}
fn main() {
    let strategy = if cfg!(indexed_probe) {
        "indexed"
    } else {
        "uncached"
    };
    let output = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("usage: table-lookup-probe NEW_ARTIFACT_DIRECTORY"),
    );
    std::fs::create_dir(&output).expect("benchmark artifact directory must not exist");
    for case in fixtures() {
        write_fresh(
            &output.join(format!("{}.tex", case.name)),
            case.source.as_bytes(),
        );
        let carets = case
            .carets
            .iter()
            .map(|caret| format!("[{},{}]", caret.character, caret.byte))
            .collect::<Vec<_>>()
            .join(",");
        write_fresh(
            &output.join(format!("{}.carets.json", case.name)),
            format!("[{carets}]\n").as_bytes(),
        );
        let mut runner = Runner::new(&case);
        runner.check(&case);
        for _ in 0..3 {
            for &caret in &case.carets {
                black_box(runner.query(black_box(caret)));
            }
        }
        let mut query_samples = Vec::new();
        for _ in 0..9 {
            let start = Instant::now();
            for &caret in &case.carets {
                black_box(runner.query(black_box(caret)));
            }
            query_samples.push(start.elapsed().as_secs_f64() * 1e9 / case.carets.len() as f64);
        }
        let mut cold_samples = Vec::new();
        for iteration in 0..12 {
            let start = Instant::now();
            let mut fresh = Runner::new(black_box(&case));
            black_box(fresh.query(black_box(case.carets[0])));
            let ns = start.elapsed().as_secs_f64() * 1e9;
            if iteration >= 3 {
                cold_samples.push(ns);
            }
        }
        let (warm_median_ns, warm_p90_ns) = percentile(&query_samples);
        let (cold_median_ns, cold_p90_ns) = percentile(&cold_samples);
        println!(
            "{{\"strategy\":\"{strategy}\",\"case\":\"{}\",\"source_bytes\":{},\"source_chars\":{},\"carets_per_batch\":{},\"warm_median_ns\":{warm_median_ns},\"warm_p90_ns\":{warm_p90_ns},\"cold_index_and_first_target_median_ns\":{cold_median_ns},\"cold_p90_ns\":{cold_p90_ns},\"golden_checks\":{},\"warm_samples_ns_per_query\":{:?},\"cold_samples_ns\":{:?}}}",
            case.name,
            case.source.len(),
            case.starts.len() - 1,
            case.carets.len(),
            case.carets.len(),
            query_samples,
            cold_samples
        );
    }
}
