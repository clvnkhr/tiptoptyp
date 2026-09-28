//! Bounded point-cloud matching for the Detexify classes, including those absent from Detypify.
use eframe::egui::Pos2;
use std::sync::OnceLock;

type Cloud = [[f32; 2]; 32];
fn normalize(strokes: &[Vec<Pos2>]) -> Option<Cloud> {
    let mut bounds = eframe::egui::Rect::NOTHING;
    for point in strokes.iter().flatten() {
        bounds.extend_with(*point);
    }
    let scale = bounds.width().max(bounds.height());
    if !scale.is_finite() {
        return None;
    }
    if scale <= 0.0 {
        return Some([[0.0; 2]; 32]);
    }
    let segments: Vec<_> = strokes
        .iter()
        .flat_map(|s| s.windows(2))
        .filter_map(|p| {
            let length = p[0].distance(p[1]);
            (length > 0.0).then_some((p[0], p[1], length))
        })
        .collect();
    let total: f32 = segments.iter().map(|s| s.2).sum();
    if total <= 0.0 {
        let points: Vec<_> = strokes.iter().flatten().collect();
        return Some(std::array::from_fn(|i| {
            let p = (*points[i % points.len()] - bounds.center()) / scale;
            [p.x, p.y]
        }));
    }
    let mut cloud = [[0.0; 2]; 32];
    for (index, point) in cloud.iter_mut().enumerate() {
        let mut target = total * (index as f32 + 0.5) / 32.0;
        for &(a, b, length) in &segments {
            if target <= length {
                let p = (a + (b - a) * (target / length) - bounds.center()) / scale;
                *point = [p.x, p.y];
                break;
            }
            target -= length;
        }
    }
    Some(cloud)
}
fn distance(a: &Cloud, b: &Cloud) -> f32 {
    let one_way = |a: &Cloud, b: &Cloud| {
        a.iter()
            .map(|p| {
                b.iter()
                    .map(|q| (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2))
                    .fold(f32::INFINITY, f32::min)
            })
            .sum::<f32>()
    };
    one_way(a, b) + one_way(b, a)
}
struct Sample {
    class: usize,
    cloud: Cloud,
    feature: [f32; 64],
}
fn feature(cloud: &Cloud) -> [f32; 64] {
    let mut result = [0.0; 64];
    for p in cloud {
        let x = ((p[0] + 0.5) * 6.0).clamp(0.0, 6.0);
        let y = ((p[1] + 0.5) * 6.0).clamp(0.0, 6.0);
        let (ix, iy) = (x as usize, y as usize);
        let (fx, fy) = (x - ix as f32, y - iy as f32);
        for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
            for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
                result[(iy + dy) * 8 + ix + dx] += wx * wy / 32.0;
            }
        }
    }
    result
}
fn decode(record: &[u8]) -> (usize, Cloud) {
    let class = u16::from_le_bytes([record[0], record[1]]) as usize;
    let mut cloud = [[0.0; 2]; 32];
    for (target, bytes) in cloud
        .iter_mut()
        .flatten()
        .zip(record[2..].as_chunks::<2>().0)
    {
        *target = u16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 65535.0 - 0.5;
    }
    (class, cloud)
}
fn load_samples(excluded: &std::collections::HashSet<&[u8]>) -> Vec<Sample> {
    include_bytes!("../assets/handwriting/detexify-samples.bin")
        .chunks_exact(130)
        .filter(|record| !excluded.contains(record))
        .map(|record| {
            let (class, cloud) = decode(record);
            Sample {
                class,
                feature: feature(&cloud),
                cloud,
            }
        })
        .collect()
}
fn labels() -> &'static [super::handwriting::Symbol] {
    static LABELS: OnceLock<Vec<super::handwriting::Symbol>> = OnceLock::new();
    LABELS.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/handwriting/detexify-symbols.json"))
            .expect("bundled Detexify labels")
    })
}
pub(crate) fn fixture_predictions() -> Vec<super::handwriting::Symbol> {
    let commands = [
        "\\mathcal{A}",
        "\\mathbb{A}",
        "\\mathfrak{A}",
        "\\Delta",
        "\\triangle",
        "\\forall",
        "\\Lambda",
        "\\bigwedge",
    ];
    commands
        .iter()
        .filter_map(|command| {
            labels()
                .iter()
                .find(|s| s.tex.as_deref() == Some(*command))
                .cloned()
        })
        .collect()
}
fn rank(cloud: &Cloud, samples: &[Sample]) -> Vec<usize> {
    let query = feature(cloud);
    let mut shortlist = vec![[(f32::INFINITY, 0usize); 3]; labels().len()];
    for (index, sample) in samples.iter().enumerate() {
        let score = sample
            .feature
            .iter()
            .zip(query)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f32>();
        let best = &mut shortlist[sample.class];
        if score < best[2].0 {
            best[2] = (score, index);
            best.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
    }
    let mut classes: Vec<_> = (0..labels().len())
        .filter(|&i| shortlist[i][0].0.is_finite())
        .collect();
    classes.sort_by(|&a, &b| shortlist[a][0].0.total_cmp(&shortlist[b][0].0));
    let mut scores: Vec<_> = classes
        .into_iter()
        .take(64)
        .map(|class| {
            let score = shortlist[class]
                .iter()
                .filter(|s| s.0.is_finite())
                .map(|s| distance(cloud, &samples[s.1].cloud))
                .fold(f32::INFINITY, f32::min);
            (class, score)
        })
        .collect();
    scores.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut commands = std::collections::HashSet::new();
    scores
        .into_iter()
        .filter(|(class, _)| commands.insert(labels()[*class].tex.as_deref()))
        .take(8)
        .map(|s| s.0)
        .collect()
}
pub(crate) fn recognize(strokes: &[Vec<Pos2>]) -> Vec<super::handwriting::Symbol> {
    static SAMPLES: OnceLock<Vec<Sample>> = OnceLock::new();
    normalize(strokes)
        .map(|cloud| {
            let samples = SAMPLES.get_or_init(|| load_samples(&Default::default()));
            rank(&cloud, samples)
                .into_iter()
                .map(|class| labels()[class].clone())
                .collect()
        })
        .unwrap_or_default()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "full-dataset optimized recognition evaluation"]
    fn held_out_symbols() {
        let records: Vec<_> = include_bytes!("../assets/handwriting/detexify-holdout.bin")
            .chunks_exact(130)
            .collect();
        let excluded = records.iter().copied().collect();
        let samples = load_samples(&excluded);
        let (mut correct, mut total, mut cal_correct, mut cal_total) = (0, 0, 0, 0);
        for record in records.iter().step_by(10) {
            let (class, cloud) = decode(record);
            let candidates = rank(&cloud, &samples);
            let hit = candidates
                .iter()
                .any(|&i| labels()[i].tex == labels()[class].tex);
            total += 1;
            correct += usize::from(hit);
            if labels()[class]
                .tex
                .as_ref()
                .unwrap()
                .starts_with("\\mathcal{")
            {
                cal_total += 1;
                cal_correct += usize::from(hit);
            }
        }
        eprintln!(
            "Detexify held-out top-eight: {correct}/{total}; mixed-ranking calligraphic subset {cal_correct}/{cal_total}"
        );
        assert!(correct * 100 / total >= 65);
    }
    #[test]
    fn typst_mappings_cover_script_greek_ipa_and_escaped_punctuation() {
        for (tex, typst) in [
            ("\\mathscr{A}", "scr(A)"),
            ("\\mathds{h}", "bb(h)"),
            ("\\Updelta", "upright(Delta)"),
            ("\\textsca", "\"ᴀ\""),
            ("\\textturnv", "\"ʌ\""),
            ("\\textceltpal", "\"ʲ\""),
            ("\\textdollar", "\"$\""),
            ("\\landdownint", "∫"),
            ("\\sqiint", "∯"),
            ("\\llceil", "\"⌈⌈\""),
            ("\\nnearrow", "↗"),
            ("\\textthreequartersemdash", "\"‒\""),
            ("\\textbraceleft", "\"{\""),
            ("\\ngeqq", "≧\u{338}"),
            ("\\Aquarius", "♒"),
        ] {
            let symbol = labels()
                .iter()
                .find(|s| s.tex.as_deref() == Some(tex))
                .unwrap();
            assert_eq!(symbol.typst.as_deref(), Some(typst), "{tex}");
            assert!(!symbol.tex_only, "{tex}");
        }
        assert!(labels().iter().filter(|s| s.typst.is_some()).count() >= 1120);
        for symbol in labels() {
            assert_eq!(symbol.tex_only, symbol.typst.is_none());
        }
    }
    #[test]
    fn every_typst_mapping_parses_as_math() {
        for symbol in labels() {
            if let Some(expression) = &symbol.typst {
                let source = format!("$ {expression} $");
                assert!(
                    typst_syntax::parse(&source)
                        .errors_and_warnings()
                        .0
                        .is_empty(),
                    "{}: {source}",
                    symbol.tex.as_deref().unwrap()
                );
            }
        }
    }
    #[test]
    #[ignore = "requires the Typst CLI; validates every mapped expression against its math library"]
    fn every_typst_mapping_compiles() {
        let directory = tempfile::tempdir().unwrap();
        let source = labels()
            .iter()
            .filter_map(|s| {
                s.typst
                    .as_ref()
                    .map(|value| format!("$ {value} $ // {}\n", s.tex.as_deref().unwrap()))
            })
            .collect::<String>();
        let input = directory.path().join("symbols.typ");
        std::fs::write(&input, source).unwrap();
        let result = std::process::Command::new("typst")
            .arg("compile")
            .arg(&input)
            .arg(directory.path().join("symbols.pdf"))
            .output()
            .expect("Typst CLI must be installed for this test");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    #[test]
    fn every_sample_has_a_label_and_bounded_coordinates() {
        let bytes = include_bytes!("../assets/handwriting/detexify-samples.bin");
        assert_eq!(bytes.len() % 130, 0);
        for record in bytes.as_chunks::<130>().0 {
            let (class, cloud) = decode(record);
            assert!(class < labels().len());
            assert!(cloud.iter().flatten().all(|p| p.abs() <= 0.5));
        }
        let a = labels()
            .iter()
            .find(|s| s.tex.as_deref() == Some("\\mathcal{A}"))
            .unwrap();
        assert_eq!(a.typst.as_deref(), Some("cal(A)"));
        assert!(labels().len() > 1000);
    }
    #[test]
    fn normalization_is_translation_and_scale_invariant() {
        let strokes = vec![vec![
            Pos2::new(0.0, 0.0),
            Pos2::new(10.0, 20.0),
            Pos2::new(20.0, 0.0),
        ]];
        let moved: Vec<_> = strokes
            .iter()
            .map(|s| {
                s.iter()
                    .map(|p| Pos2::new(p.x * 3.0 + 40.0, p.y * 3.0 - 20.0))
                    .collect()
            })
            .collect();
        let a = normalize(&strokes).unwrap();
        let b = normalize(&moved).unwrap();
        assert!(distance(&a, &b) < 0.00001);
        assert!(normalize(&[]).is_none());
    }
}
