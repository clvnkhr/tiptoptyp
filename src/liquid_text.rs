//! Native text preparation and edit decoding for the pinned Liquid tagger.
//! The English rules mirror sacremoses 0.2.0; see assets/liquid/SACREMOSES-LICENSE.
use regex::Regex;
use serde::Deserialize;
use std::{collections::HashSet, ops::Range, path::Path, sync::LazyLock};
use tokenizers::Tokenizer;

const VOCABULARY: i64 = 64_400;
const TOKEN_LIMIT: usize = 128;
const MAX_PASSES: usize = 3;
pub(crate) const MAX_INPUT_BYTES: usize = 1024 * 1024;
const CHUNK_WINDOW_BYTES: usize = 1024;

pub(crate) struct ChunkProgress<'a> {
    pub(crate) byte_range: Range<usize>,
    pub(crate) corrected: &'a str,
    pub(crate) token_count: usize,
    pub(crate) completed: usize,
    pub(crate) total: usize,
}

struct Prepared {
    text: String,
    ids: Vec<i64>,
    offsets: Vec<(usize, usize)>,
}
struct ModelEncoding {
    ids: Vec<i64>,
    offsets: Vec<(usize, usize)>,
}

struct Boundaries {
    words: Vec<usize>,
    sentences: Vec<usize>,
}
impl Boundaries {
    fn new(original: &str) -> Self {
        static WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\S+").unwrap());
        let mut words = Vec::new();
        let mut sentences = Vec::new();
        let mut spans = WORDS.find_iter(original).peekable();
        while let Some(word) = spans.next() {
            words.push(word.end());
            let next = spans.peek();
            let paragraph = next.is_some_and(|next| {
                original[word.end()..next.start()]
                    .chars()
                    .filter(|&c| c == '\n')
                    .take(2)
                    .count()
                    == 2
            });
            if next.is_none()
                || paragraph
                || sentence_terminal(word.as_str(), next.map(|word| word.as_str()))
            {
                sentences.push(word.end());
            }
        }
        Self { words, sentences }
    }
    fn between(ends: &[usize], start: usize, end: usize) -> &[usize] {
        &ends[ends.partition_point(|&value| value <= start)
            ..ends.partition_point(|&value| value <= end)]
    }
}

fn sentence_terminal(word: &str, next: Option<&str>) -> bool {
    let word = word.trim_end_matches(['\'', '"', '’', '”', ')', ']', '}', '»']);
    if word.ends_with(['!', '?']) || word.ends_with("..") {
        return true;
    }
    let Some(prefix) = word.strip_suffix('.') else {
        return false;
    };
    if prefix.is_empty() {
        return true;
    }
    !RULES.prefixes.contains(prefix)
        && !(prefix.chars().count() == 1 && prefix.chars().all(char::is_alphabetic))
        && !(prefix.contains('.') && prefix.chars().any(char::is_alphabetic))
        && !(RULES.numeric_prefixes.contains(prefix)
            && next.is_some_and(|next| next.starts_with(|c: char| c.is_ascii_digit())))
}

pub(crate) struct Prediction {
    pub(crate) tags: Vec<i64>,
    pub(crate) error_probabilities: Vec<f32>,
}

pub(crate) struct TextProcessor {
    tokenizer: Tokenizer,
    bos: u32,
}
impl TextProcessor {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let mut tokenizer = Tokenizer::from_file(path)
            .map_err(|e| format!("Cannot load Liquid's tokenizer: {e}"))?;
        tokenizer.with_truncation(None).map_err(|e| e.to_string())?;
        tokenizer.with_padding(None);
        if tokenizer.get_vocab_size(false) != VOCABULARY as usize {
            return Err("Liquid tokenizer vocabulary does not match its model.".into());
        }
        let bos = tokenizer
            .token_to_id("<|startoftext|>")
            .ok_or("Liquid tokenizer is missing its sentence anchor.")?;
        if bos != 1 {
            return Err("Liquid tokenizer has an invalid sentence anchor.".into());
        }
        Ok(Self { tokenizer, bos })
    }
    fn encode_unbounded(&self, text: &str) -> Result<ModelEncoding, String> {
        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|e| e.to_string())?;
        let ids = std::iter::once(i64::from(self.bos))
            .chain(encoding.get_ids().iter().map(|id| i64::from(*id)))
            .collect();
        Ok(ModelEncoding {
            ids,
            offsets: encoding.get_offsets().to_vec(),
        })
    }
    #[cfg(test)]
    fn encode(&self, text: &str) -> Result<Vec<i64>, String> {
        let ids = self.encode_unbounded(text)?.ids;
        if ids.len() > TOKEN_LIMIT {
            return Err(format!("Liquid input exceeds {TOKEN_LIMIT} model tokens."));
        }
        Ok(ids)
    }
    fn prepare(&self, original: &str) -> Result<Prepared, String> {
        let text = RULES.tokenize(&normalize_apostrophes(original)).join(" ");
        let ModelEncoding { ids, offsets } = self.encode_unbounded(&text)?;
        Ok(Prepared { text, ids, offsets })
    }
    fn largest_fitting_boundary(
        &self,
        original: &str,
        start: usize,
        boundaries: &[usize],
    ) -> Result<Option<(usize, Prepared)>, String> {
        let (mut lower, mut upper) = (0, boundaries.len());
        let mut best = None;
        while lower < upper {
            let middle = lower + (upper - lower) / 2;
            let end = boundaries[middle];
            let prepared = self.prepare(&original[start..end])?;
            if prepared.ids.len() <= TOKEN_LIMIT {
                best = Some((end, prepared));
                lower = middle + 1;
            } else {
                upper = middle;
            }
        }
        Ok(best)
    }
    fn token_prefix(
        &self,
        original: &str,
        start: usize,
        window_end: usize,
        window: &Prepared,
    ) -> Result<(usize, Prepared), String> {
        // Moses inserts whitespace, but keeps non-whitespace characters in
        // order. Map tokenizer offsets back through those insertions and the
        // apostrophe normalization instead of slicing UTF-8 or model pieces.
        let source = &original[start..window_end];
        let normalized = normalize_apostrophes(source);
        let mut source_chars = source
            .char_indices()
            .zip(normalized.chars())
            .filter(|(_, c)| *c > '\u{1f}' && !c.is_whitespace());
        let mut mapping = Vec::new();
        let mut mapped = true;
        for (offset, c) in window
            .text
            .char_indices()
            .filter(|(_, c)| !c.is_whitespace())
        {
            let Some(((source_offset, source_char), normalized_char)) = source_chars.next() else {
                mapped = false;
                break;
            };
            if c != normalized_char {
                mapped = false;
                break;
            }
            mapping.push((
                offset + c.len_utf8(),
                start + source_offset + source_char.len_utf8(),
            ));
        }
        mapped &= source_chars.next().is_none();
        if mapped {
            let mut boundaries = Vec::new();
            for &(_, end) in window.offsets.iter().take(TOKEN_LIMIT - 1) {
                let index = mapping.partition_point(|&(prepared_end, _)| prepared_end <= end);
                if index > 0 {
                    boundaries.push(mapping[index - 1].1);
                }
            }
            boundaries.sort_unstable();
            boundaries.dedup();
            // A terminal punctuation rule can change at the new boundary, so
            // validate the real prepared prefix before sending it to the model.
            for end in boundaries.into_iter().rev() {
                let prepared = self.prepare(&original[start..end])?;
                if prepared.ids.len() <= TOKEN_LIMIT {
                    return Ok((end, prepared));
                }
            }
        }
        // A literal Moses sentinel can alter characters, and some tokenizer
        // offsets can be empty. Keep a Unicode-safe checked fallback; no input
        // character is ever silently truncated or discarded.
        let boundaries: Vec<_> = source
            .char_indices()
            .map(|(i, c)| start + i + c.len_utf8())
            .collect();
        self.largest_fitting_boundary(original, start, &boundaries)?
            .ok_or_else(|| "Liquid cannot fit one character in its model token budget.".into())
    }
    fn chunks(&self, original: &str) -> Result<Vec<Range<usize>>, String> {
        let boundaries = Boundaries::new(original);
        let end = original.trim_end().len();
        let mut start = original.len() - original.trim_start().len();
        let mut oversized_sentence = None;
        let mut chunks = Vec::new();
        while start < end {
            let sentence_index = boundaries
                .sentences
                .partition_point(|&value| value <= start);
            let sentence_end = boundaries.sentences[sentence_index];
            let sentence_fits = if oversized_sentence == Some(sentence_end) {
                false
            } else {
                // Each oversized sentence is measured just once. Its remaining
                // suffixes use small windows rather than repeatedly tokenizing
                // the entire rest of a large paper.
                let fits = self.prepare(&original[start..sentence_end])?.ids.len() <= TOKEN_LIMIT;
                oversized_sentence = (!fits).then_some(sentence_end);
                fits
            };
            let search_end = if sentence_fits { end } else { sentence_end };
            let mut window_bytes = CHUNK_WINDOW_BYTES.max(if sentence_fits {
                sentence_end - start
            } else {
                0
            });
            let (chunk_end, _) = loop {
                let mut window_end = start.saturating_add(window_bytes).min(search_end);
                while !original.is_char_boundary(window_end) {
                    window_end -= 1;
                }
                let window = self.prepare(&original[start..window_end])?;
                if window.ids.len() <= TOKEN_LIMIT {
                    if window_end == search_end {
                        break (window_end, window);
                    }
                    window_bytes = window_bytes.saturating_mul(2);
                    continue;
                }
                let sentence_ends = Boundaries::between(&boundaries.sentences, start, window_end);
                if let Some(prefix) =
                    self.largest_fitting_boundary(original, start, sentence_ends)?
                {
                    break prefix;
                }
                let word_ends = Boundaries::between(&boundaries.words, start, window_end);
                if let Some(prefix) = self.largest_fitting_boundary(original, start, word_ends)? {
                    break prefix;
                }
                break self.token_prefix(original, start, window_end, &window)?;
            };
            chunks.push(start..chunk_end);
            start = chunk_end + original[chunk_end..end].len()
                - original[chunk_end..end].trim_start().len();
        }
        if chunks.len() == 1 {
            // Preserve the existing single-model-input behavior, including all
            // pinned refinement inputs and original-whitespace identity checks.
            chunks[0] = 0..original.len();
        }
        Ok(chunks)
    }
    #[cfg(test)]
    pub(crate) fn correct(
        &self,
        original: &str,
        infer: impl FnMut(&[i64]) -> Result<Prediction, String>,
    ) -> Result<String, String> {
        self.correct_stream(original, infer, |_| Ok(()))
    }
    pub(crate) fn correct_stream(
        &self,
        original: &str,
        mut infer: impl FnMut(&[i64]) -> Result<Prediction, String>,
        mut progress: impl FnMut(ChunkProgress<'_>) -> Result<(), String>,
    ) -> Result<String, String> {
        if original.len() > MAX_INPUT_BYTES {
            return Err("The selected prose exceeds Liquid's 1 MiB limit.".into());
        }
        if original.trim().is_empty() {
            return Ok(original.into());
        }
        let chunks = self.chunks(original)?;
        let total = chunks.len();
        let mut corrected = String::with_capacity(original.len());
        let mut previous_end = 0;
        for (index, byte_range) in chunks.into_iter().enumerate() {
            let prepared = self.prepare(&original[byte_range.clone()])?;
            let token_count = prepared.ids.len();
            let output = self.correct_chunk(&original[byte_range.clone()], prepared, &mut infer)?;
            corrected.push_str(&original[previous_end..byte_range.start]);
            corrected.push_str(&output);
            previous_end = byte_range.end;
            progress(ChunkProgress {
                byte_range,
                corrected: &output,
                token_count,
                completed: index + 1,
                total,
            })?;
        }
        corrected.push_str(&original[previous_end..]);
        Ok(corrected)
    }
    fn correct_chunk(
        &self,
        original: &str,
        prepared: Prepared,
        infer: &mut impl FnMut(&[i64]) -> Result<Prediction, String>,
    ) -> Result<String, String> {
        let initial = prepared.text;
        let mut current = initial.clone();
        let mut ids = prepared.ids;
        let mut seen = HashSet::from([current.clone()]);
        for pass in 0..MAX_PASSES {
            if ids.len() > TOKEN_LIMIT {
                return Err("Liquid chunk exceeds its model token budget.".into());
            }
            let output = apply_tags(&ids, &infer(&ids)?)?;
            let next = self
                .tokenizer
                .decode(&output, true)
                .map_err(|e| e.to_string())?
                .trim()
                .to_owned();
            if next == current || !seen.insert(next.clone()) {
                current = next;
                break;
            }
            current = next;
            if pass + 1 < MAX_PASSES {
                let next_ids = self.encode_unbounded(&current)?.ids;
                if next_ids.len() > TOKEN_LIMIT {
                    // The tagger may legally append enough tokens to exceed
                    // its next-input budget. Keep that correction and finish
                    // this chunk; never fail or truncate a valid earlier pass.
                    break;
                }
                ids = next_ids;
            }
        }
        if current.split_whitespace().eq(initial.split_whitespace()) {
            return Ok(original.into());
        }
        let words: Vec<_> = current.split_whitespace().collect();
        Ok(restore_apostrophes(RULES.detokenize(&words), original))
    }
}

fn apply_tags(ids: &[i64], prediction: &Prediction) -> Result<Vec<u32>, String> {
    if prediction.tags.len() != ids.len() || prediction.error_probabilities.len() != ids.len() {
        return Err("Liquid model returned an invalid prediction length.".into());
    }
    let mut out = Vec::with_capacity(ids.len() * 2);
    for (index, (&piece, (&label, &probability))) in ids
        .iter()
        .zip(prediction.tags.iter().zip(&prediction.error_probabilities))
        .enumerate()
    {
        if !(0..2 + 2 * VOCABULARY).contains(&label)
            || !probability.is_finite()
            || !(0.0..=1.0).contains(&probability)
            || !(0..VOCABULARY + 2).contains(&piece)
        {
            return Err("Liquid model returned an invalid prediction.".into());
        }
        let label = if probability >= 0.5 { label } else { 0 };
        match label {
            0 if index > 0 => out.push(piece as u32),
            0 | 1 => {}
            label if label < 2 + VOCABULARY => out.push((label - 2) as u32),
            label => {
                if index > 0 {
                    out.push(piece as u32);
                }
                out.push((label - 2 - VOCABULARY) as u32);
            }
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleData {
    version: u8,
    source: String,
    alnum: String,
    alpha: String,
    numbers: String,
    lower: String,
    currency: String,
    prefixes: Vec<String>,
    numeric_prefixes: Vec<String>,
    cjk_ranges: Vec<(u32, u32)>,
}
struct EnglishRules {
    alnum: HashSet<char>,
    alpha: HashSet<char>,
    lower: HashSet<char>,
    currency: HashSet<char>,
    prefixes: HashSet<String>,
    numeric_prefixes: HashSet<String>,
    cjk: Vec<(u32, u32)>,
    apostrophes: Vec<(Regex, &'static str)>,
    commas: Vec<(Regex, &'static str)>,
    multidots: Regex,
    dot_marker: Regex,
}
static RULES: LazyLock<EnglishRules> = LazyLock::new(EnglishRules::new);
impl EnglishRules {
    fn new() -> Self {
        let data: RuleData = serde_json::from_str(include_str!("../assets/liquid/moses-en.json"))
            .expect("bundled English tokenization rules");
        assert_eq!(data.version, 1);
        assert_eq!(data.source, "sacremoses 0.2.0 English; MIT");
        let alpha = &data.alpha;
        let number = &data.numbers;
        let compile = |pattern: String, replacement| {
            (
                Regex::new(&pattern).expect("bundled English regex"),
                replacement,
            )
        };
        Self {
            apostrophes: vec![
                compile(format!("([^{alpha}])'([^{alpha}])"), "${1} ' ${2}"),
                compile(format!("([^{alpha}{number}])'([{alpha}])"), "${1} ' ${2}"),
                compile(format!("([{alpha}])'([^{alpha}])"), "${1} ' ${2}"),
                compile(format!("([{alpha}])'([{alpha}])"), "${1} '${2}"),
                compile(format!("([{number}])'([s])"), "${1} '${2}"),
            ],
            commas: vec![
                compile(format!("([^{number}]),"), "${1} , "),
                compile(format!(",([^{number}])"), " , ${1}"),
                compile(format!("([{number}]),$"), "${1} , "),
            ],
            alnum: data.alnum.chars().collect(),
            alpha: data.alpha.chars().collect(),
            lower: data.lower.chars().collect(),
            currency: data.currency.chars().collect(),
            prefixes: data.prefixes.into_iter().collect(),
            numeric_prefixes: data.numeric_prefixes.into_iter().collect(),
            cjk: data.cjk_ranges,
            multidots: Regex::new(r"\.{2,}").unwrap(),
            dot_marker: Regex::new(r"(?:DOT)+MULTI").unwrap(),
        }
    }
    fn tokenize(&self, original: &str) -> Vec<String> {
        let clean = original.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut text = String::with_capacity(clean.len() * 2);
        for c in clean.chars().filter(|c| *c > '\u{1f}') {
            if !self.alnum.contains(&c) && !c.is_whitespace() && !".'`,-".contains(c) {
                text.push(' ');
                text.push(c);
                text.push(' ');
            } else {
                text.push(c);
            }
        }
        let length = text.len();
        text = self
            .multidots
            .replace_all(&text, |captures: &regex::Captures<'_>| {
                let run = captures.get(0).unwrap();
                format!(
                    " {}MULTI{}",
                    "DOT".repeat(run.len()),
                    if run.end() < length { " " } else { "" }
                )
            })
            .into_owned();
        for (rule, replacement) in self.commas.iter().chain(&self.apostrophes) {
            text = rule.replace_all(&text, *replacement).into_owned();
        }
        let words: Vec<_> = text.split_whitespace().collect();
        let mut expanded = Vec::with_capacity(words.len());
        for (i, &word) in words.iter().enumerate() {
            if let Some(prefix) = word.strip_suffix('.').filter(|s| !s.is_empty()) {
                let next = words.get(i + 1).and_then(|s| s.chars().next());
                let numeric = self.numeric_prefixes.contains(prefix);
                let keep = (prefix.contains('.')
                    && prefix.chars().any(|c| self.alpha.contains(&c)))
                    || (self.prefixes.contains(prefix) && !numeric)
                    || next.is_some_and(|c| self.lower.contains(&c))
                    || (numeric && next.is_some_and(|c| c.is_ascii_digit()));
                if !keep {
                    expanded.push(prefix);
                    expanded.push(".");
                    continue;
                }
            }
            expanded.push(word);
        }
        text = expanded.join(" ");
        if text.ends_with(".'") {
            text.truncate(text.len() - 2);
            text.push_str(" . ' ");
        }
        self.dot_marker
            .replace_all(&text, |captures: &regex::Captures<'_>| {
                ".".repeat((captures[0].len() - 5) / 3)
            })
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }
    fn is_cjk(&self, c: char) -> bool {
        let value = c as u32;
        self.cjk
            .iter()
            .any(|&(start, end)| (start..=end).contains(&value))
    }
    fn detokenize(&self, words: &[&str]) -> String {
        // Moses accepts whitespace-tokenized text and merges its @-@ convention.
        let merged = format!(" {} ", words.join(" ")).replace(" @-@ ", "-");
        let words: Vec<_> = merged.split_whitespace().collect();
        let mut out = String::new();
        let mut space = " ";
        let mut quotes = std::collections::HashMap::<&str, usize>::new();
        for (i, &word) in words.iter().enumerate() {
            let first = word.chars().next().unwrap();
            if self.is_cjk(first) {
                if i == 0
                    || !words[i - 1]
                        .chars()
                        .next_back()
                        .is_some_and(|c| self.is_cjk(c))
                {
                    out.push_str(space);
                }
                out.push_str(word);
                space = " ";
            } else if word
                .chars()
                .all(|c| self.currency.contains(&c) || "([{¿¡".contains(c))
            {
                out.push_str(space);
                out.push_str(word);
                space = "";
            } else if word.chars().all(|c| ",.?!:;\\%}])".contains(c))
                || (i > 0
                    && word
                        .strip_prefix('\'')
                        .and_then(|s| s.chars().next())
                        .is_some_and(|c| self.alpha.contains(&c)))
            {
                out.push_str(word);
                space = " ";
            } else if word.chars().all(|c| "'\"„“`".contains(c)) {
                let normalized = if word.chars().all(|c| "„“”".contains(c)) {
                    "\""
                } else {
                    word
                };
                let count = quotes.entry(normalized).or_default();
                if (*count).is_multiple_of(2) {
                    if word == "'" && i > 0 && words[i - 1].ends_with('s') {
                        out.push_str(word);
                        space = " ";
                    } else {
                        out.push_str(space);
                        out.push_str(word);
                        space = "";
                        *count += 1;
                    }
                } else {
                    out.push_str(word);
                    space = " ";
                    *count += 1;
                }
            } else {
                out.push_str(space);
                out.push_str(word);
                space = " ";
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn normalize_apostrophes(text: &str) -> String {
    let chars: Vec<_> = text.chars().collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            if matches!(c, '’' | 'ʼ')
                && i > 0
                && i + 1 < chars.len()
                && word_char(chars[i - 1])
                && word_char(chars[i + 1])
            {
                '\''
            } else {
                c
            }
        })
        .collect()
}
fn restore_apostrophes(mut corrected: String, original: &str) -> String {
    if !original.chars().any(|c| matches!(c, '’' | 'ʼ')) {
        return corrected;
    }
    static WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+(?:['’ʼ]\w+)*").unwrap());
    let replacements = {
        // Align complete lexical occurrences, including ordinary context words.
        // A global replacement conflates repeated names with different glyphs;
        // matching the context also handles an earlier occurrence being deleted.
        let source: Vec<_> = WORDS
            .find_iter(original)
            .map(|word| (word, normalize_apostrophes(word.as_str())))
            .collect();
        let output: Vec<_> = WORDS
            .find_iter(&corrected)
            .map(|word| (word, normalize_apostrophes(word.as_str())))
            .collect();
        let width = output.len() + 1;
        let Some(cells) = (source.len() + 1)
            .checked_mul(width)
            .filter(|cells| *cells <= 1_048_576)
        else {
            // Successful model input is limited to 128 pieces, so normal checks
            // are far below this bound. Never guess glyph ancestry for an
            // oversized/unmapped result or allocate an unbounded diff table.
            return corrected;
        };
        // Preserve the maximum number of matching words first. Neighboring
        // context only breaks ties between repeated names, so removing an
        // earlier clause does not steal the remaining name's glyph style.
        let word_score = (2 * source.len().min(output.len()) + 1) as u32;
        let context_score = |i: usize, j: usize| {
            u32::from(i > 0 && j > 0 && source[i - 1].1 == output[j - 1].1)
                + u32::from(
                    i + 1 < source.len()
                        && j + 1 < output.len()
                        && source[i + 1].1 == output[j + 1].1,
                )
        };
        let mut table = vec![0u32; cells];
        for i in (0..source.len()).rev() {
            for j in (0..output.len()).rev() {
                let skipped = table[(i + 1) * width + j].max(table[i * width + j + 1]);
                table[i * width + j] = if source[i].1 == output[j].1 {
                    skipped.max(word_score + context_score(i, j) + table[(i + 1) * width + j + 1])
                } else {
                    skipped
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        let mut replacements = Vec::new();
        while i < source.len() && j < output.len() {
            if source[i].1 == output[j].1
                && table[i * width + j]
                    == word_score + context_score(i, j) + table[(i + 1) * width + j + 1]
            {
                if source[i].0.as_str() != output[j].0.as_str() {
                    replacements.push((output[j].0.range(), source[i].0.as_str()));
                }
                i += 1;
                j += 1;
            } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
        replacements
    };
    for (range, word) in replacements.into_iter().rev() {
        corrected.replace_range(range, word);
    }
    corrected
}

#[cfg(test)]
mod tests {
    use super::*;
    fn character_processor() -> TextProcessor {
        use tokenizers::{
            decoders::fuse::Fuse,
            models::bpe::{BPE, Vocab},
        };
        let mut vocab =
            Vocab::from_iter([("<unk>".to_owned(), 0), ("<|startoftext|>".to_owned(), 1)]);
        for c in (32u8..=126).map(char::from).chain("é中’sʼ".chars()) {
            let next = vocab.len() as u32;
            vocab.entry(c.to_string()).or_insert(next);
        }
        let mut tokenizer = Tokenizer::new(
            BPE::builder()
                .vocab_and_merges(vocab, Vec::new())
                .unk_token("<unk>".into())
                .build()
                .unwrap(),
        );
        tokenizer.with_decoder(Some(Fuse::new()));
        TextProcessor { tokenizer, bos: 1 }
    }
    fn unchanged_prediction(ids: &[i64]) -> Result<Prediction, String> {
        assert!(ids.len() <= TOKEN_LIMIT, "oversized input reached model");
        Ok(Prediction {
            tags: vec![0; ids.len()],
            error_probabilities: vec![0.0; ids.len()],
        })
    }
    #[test]
    fn oversized_selection_keeps_complete_sentences_and_original_separators() {
        let processor = character_processor();
        let sentences = [
            "The first estimate is valid for every parameter in this open interval.",
            "The second estimate is valid for every parameter in this open interval.",
            "The final estimate is valid for every parameter in this open interval.",
        ];
        let original = format!(
            " \t{}  \n\n{}\t {} \n",
            sentences[0], sentences[1], sentences[2]
        );
        let mut reports = Vec::new();
        let corrected = processor
            .correct_stream(&original, unchanged_prediction, |chunk| {
                reports.push((
                    chunk.byte_range,
                    chunk.corrected.to_owned(),
                    chunk.token_count,
                    chunk.completed,
                    chunk.total,
                ));
                Ok(())
            })
            .unwrap();
        assert_eq!(corrected, original);
        assert_eq!(reports.len(), sentences.len());
        for (index, (range, text, tokens, completed, total)) in reports.iter().enumerate() {
            assert_eq!(&original[range.clone()], sentences[index]);
            assert_eq!(text, sentences[index]);
            assert!(*tokens <= TOKEN_LIMIT);
            assert_eq!(*completed, index + 1);
            assert_eq!(*total, sentences.len());
        }
    }
    #[test]
    fn chunking_counts_normalized_model_tokens_and_splits_long_unicode_words_losslessly() {
        let processor = character_processor();
        let original = "é".repeat(350);
        let mut reports = Vec::new();
        let corrected = processor
            .correct_stream(&original, unchanged_prediction, |chunk| {
                assert!(original.is_char_boundary(chunk.byte_range.start));
                assert!(original.is_char_boundary(chunk.byte_range.end));
                reports.push((chunk.byte_range, chunk.token_count));
                Ok(())
            })
            .unwrap();
        assert_eq!(corrected, original);
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0], (0..127 * 2, TOKEN_LIMIT));
        assert_eq!(reports[1], (127 * 2..254 * 2, TOKEN_LIMIT));
        assert_eq!(reports[2], (254 * 2..350 * 2, 97));
        // Moses expands each CJK character to its own whitespace-separated
        // token. The budget must include those added spaces, not source words
        // or source scalar counts.
        let mut reports = Vec::new();
        let original = "中".repeat(150);
        assert_eq!(
            processor
                .correct_stream(&original, unchanged_prediction, |chunk| {
                    reports.push((chunk.byte_range, chunk.token_count));
                    Ok(())
                })
                .unwrap(),
            original
        );
        assert_eq!(reports[0], (0..64 * 3, TOKEN_LIMIT));
        assert_eq!(reports[1], (64 * 3..128 * 3, TOKEN_LIMIT));
        assert_eq!(reports[2], (128 * 3..150 * 3, 44));
    }
    #[test]
    fn chunk_edits_preserve_joining_and_outer_whitespace() {
        let processor = character_processor();
        let sentence = format!("{}.", "a".repeat(80));
        let original = format!(" \t{sentence}  \n\n{sentence} \t");
        let a = i64::from(processor.tokenizer.token_to_id("a").unwrap());
        let b = i64::from(processor.tokenizer.token_to_id("b").unwrap());
        let corrected = processor
            .correct_stream(
                &original,
                |ids| {
                    assert!(ids.len() <= TOKEN_LIMIT);
                    Ok(Prediction {
                        tags: ids
                            .iter()
                            .map(|&id| if id == a { 2 + b } else { 0 })
                            .collect(),
                        error_probabilities: vec![1.0; ids.len()],
                    })
                },
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(corrected, original.replace('a', "b"));
    }
    #[test]
    fn streaming_cancellation_stops_before_the_next_chunk() {
        let processor = character_processor();
        let mut calls = 0;
        let error = processor
            .correct_stream(
                &"a ".repeat(200),
                |ids| {
                    calls += 1;
                    unchanged_prediction(ids)
                },
                |_| Err("cancelled".into()),
            )
            .unwrap_err();
        assert_eq!(error, "cancelled");
        assert_eq!(calls, 1);
    }
    #[test]
    fn a_valid_pass_may_expand_beyond_the_next_pass_budget_without_losing_edits() {
        let processor = character_processor();
        let a = i64::from(processor.tokenizer.token_to_id("a").unwrap());
        let mut calls = 0;
        let corrected = processor
            .correct(&"a".repeat(100), |ids| {
                calls += 1;
                assert!(ids.len() <= TOKEN_LIMIT);
                Ok(Prediction {
                    tags: ids
                        .iter()
                        .enumerate()
                        .map(|(i, _)| if i == 0 { 0 } else { 2 + VOCABULARY + a })
                        .collect(),
                    error_probabilities: vec![1.0; ids.len()],
                })
            })
            .unwrap();
        assert_eq!(corrected, "a".repeat(200));
        assert_eq!(calls, 1);
    }
    #[test]
    fn selection_limit_is_bounded_without_the_old_paragraph_limit() {
        let processor = character_processor();
        let original = "a ".repeat(10_000);
        assert_eq!(
            processor.correct(&original, unchanged_prediction).unwrap(),
            original
        );
        assert!(
            processor
                .correct(&"a".repeat(MAX_INPUT_BYTES + 1), |_| panic!(
                    "oversized selection reached model"
                ))
                .is_err()
        );
    }
    #[derive(Deserialize)]
    struct Case {
        text: String,
        prepared: String,
        ids: Vec<i64>,
        detokenized: String,
    }
    fn cases() -> Vec<Case> {
        serde_json::from_str(include_str!("../assets/liquid/text-cases.json")).unwrap()
    }
    #[test]
    fn english_preparation_and_detokenization_match_pinned_python_corpus() {
        let fixtures = cases();
        assert!(fixtures.len() >= 250);
        for case in fixtures {
            let tokens = RULES.tokenize(&normalize_apostrophes(&case.text));
            assert_eq!(tokens.join(" "), case.prepared, "{}", case.text);
            let words: Vec<_> = tokens.iter().map(String::as_str).collect();
            assert_eq!(RULES.detokenize(&words), case.detokenized, "{}", case.text);
        }
    }
    #[derive(Deserialize)]
    struct ReferenceCorpus {
        cases: Vec<ReferenceCase>,
    }
    #[derive(Deserialize)]
    struct ReferenceCase {
        text: String,
        reference_corrected: String,
        moses_tokens: Vec<String>,
        reference_passes: Vec<ReferencePass>,
    }
    #[derive(Deserialize)]
    struct ReferencePass {
        input_ids: Vec<i64>,
        best_tags: Vec<i64>,
        error_probs: Vec<f32>,
        output_ids: Vec<u32>,
    }
    fn reference_cases() -> Vec<ReferenceCase> {
        serde_json::from_str::<ReferenceCorpus>(include_str!(
            "../assets/liquid/correction-cases.json"
        ))
        .unwrap()
        .cases
    }
    #[test]
    fn all_pinned_reference_edit_decisions_decode_identically() {
        let cases = reference_cases();
        assert_eq!(cases.len(), 268);
        let mut passes = 0;
        for case in cases {
            assert_eq!(
                RULES.tokenize(&normalize_apostrophes(&case.text)),
                case.moses_tokens
            );
            for step in case.reference_passes {
                let prediction = Prediction {
                    tags: step.best_tags,
                    error_probabilities: step.error_probs,
                };
                assert_eq!(
                    apply_tags(&step.input_ids, &prediction).unwrap(),
                    step.output_ids,
                    "{}",
                    case.text
                );
                passes += 1;
            }
        }
        assert_eq!(passes, 427);
    }
    #[test]
    #[ignore = "needs the pinned optional Liquid tokenizer"]
    fn full_native_text_pipeline_matches_all_reference_corrections() {
        let path = std::env::var_os("TIPTOPTYP_LIQUID_TOKENIZER").expect("tokenizer path");
        let processor = TextProcessor::load(Path::new(&path)).unwrap();
        for case in reference_cases() {
            let mut steps = case.reference_passes.into_iter();
            let corrected = processor
                .correct(&case.text, |ids| {
                    let step = steps.next().expect("unexpected extra refinement");
                    assert_eq!(ids, step.input_ids, "{}", case.text);
                    Ok(Prediction {
                        tags: step.best_tags,
                        error_probabilities: step.error_probs,
                    })
                })
                .unwrap();
            assert_eq!(corrected, case.reference_corrected, "{}", case.text);
            assert!(steps.next().is_none(), "omitted refinement: {}", case.text);
        }
    }
    #[test]
    fn decoder_validates_tags_and_gates_edits() {
        let prediction = Prediction {
            tags: vec![2 + VOCABULARY + 5, 2 + 6, 1, 0],
            error_probabilities: vec![0.8, 0.5, 0.9, 0.3],
        };
        assert_eq!(
            apply_tags(&[1, 10, 11, 12], &prediction).unwrap(),
            [5, 6, 12]
        );
        assert!(apply_tags(&[1], &prediction).is_err());
        assert!(
            apply_tags(
                &[1],
                &Prediction {
                    tags: vec![-1],
                    error_probabilities: vec![1.0]
                }
            )
            .is_err()
        );
        assert!(
            apply_tags(
                &[1],
                &Prediction {
                    tags: vec![0],
                    error_probabilities: vec![f32::NAN]
                }
            )
            .is_err()
        );
    }
    #[test]
    fn preserves_word_apostrophes_and_unchanged_whitespace() {
        assert_eq!(
            normalize_apostrophes("Tao’s and Barbatoʼs 'quotes'"),
            "Tao's and Barbato's 'quotes'"
        );
        assert_eq!(
            restore_apostrophes(
                "Tao's and Barbato's corrected result".into(),
                "Tao’s and Barbatoʼs result"
            ),
            "Tao’s and Barbatoʼs corrected result"
        );
        assert_eq!(
            restore_apostrophes("XTao's Tao'sa Tao's".into(), "Tao’s"),
            "XTao's Tao'sa Tao’s"
        );
    }
    #[test]
    fn restores_apostrophe_glyphs_per_lexical_occurrence() {
        for (corrected, original, expected) in [
            (
                "Tao's and Tao's estimates hold.",
                "Tao’s and Taoʼs estimates holds.",
                "Tao’s and Taoʼs estimates hold.",
            ),
            (
                "Tao's, Tao's and Tao's estimates hold.",
                "Tao's, Tao’s and Taoʼs estimates holds.",
                "Tao's, Tao’s and Taoʼs estimates hold.",
            ),
            (
                "rock'n'roll is enjoyable.",
                "rock’nʼroll are enjoyable.",
                "rock’nʼroll is enjoyable.",
            ),
            (
                "Tao's bounds hold.",
                "Tao’s estimates fail; Taoʼs bounds holds.",
                "Taoʼs bounds hold.",
            ),
            (
                "but Tao's bounds hold.",
                "Tao’s estimates fail but Taoʼs bounds holds.",
                "but Taoʼs bounds hold.",
            ),
            (
                "Tao's estimates and also Tao's bounds hold.",
                "Tao’s estimates and Taoʼs bounds holds.",
                "Tao’s estimates and also Taoʼs bounds hold.",
            ),
        ] {
            assert_eq!(
                restore_apostrophes(corrected.into(), original),
                expected,
                "{original}"
            );
        }
    }
    #[test]
    fn apostrophe_restoration_keeps_new_words_and_bounds_alignment_work() {
        let corrected = "Barbato's and Tao's result holds.";
        assert_eq!(
            restore_apostrophes(corrected.into(), "Taoʼs result hold."),
            "Barbato's and Taoʼs result holds."
        );
        let unchanged = "ASCII punctuation needs no restoration.".to_owned();
        let address = unchanged.as_ptr();
        let restored = restore_apostrophes(unchanged, "ASCII punctuation need no restoration.");
        assert_eq!(
            restored.as_ptr(),
            address,
            "the ordinary path must reuse the original string allocation"
        );
        let original = format!("{}Tao’s", "word ".repeat(1100));
        let corrected = format!("{}Tao's", "word ".repeat(1100));
        assert_eq!(restore_apostrophes(corrected.clone(), &original), corrected);
    }
    #[test]
    #[ignore = "needs the pinned optional Liquid tokenizer"]
    fn tokenizer_ids_match_pinned_python_and_checks_never_truncate() {
        let path = std::env::var_os("TIPTOPTYP_LIQUID_TOKENIZER").expect("tokenizer path");
        let processor = TextProcessor::load(Path::new(&path)).unwrap();
        for case in cases() {
            if case.ids.len() <= TOKEN_LIMIT {
                assert_eq!(
                    processor.encode(&case.prepared).unwrap(),
                    case.ids,
                    "{}",
                    case.text
                );
                let unchanged = processor
                    .correct(&case.text, |ids| {
                        Ok(Prediction {
                            tags: vec![0; ids.len()],
                            error_probabilities: vec![0.0; ids.len()],
                        })
                    })
                    .unwrap();
                assert_eq!(unchanged, case.text);
            } else {
                assert!(processor.encode(&case.prepared).is_err());
            }
        }
        let too_long = "word ".repeat(200);
        assert_eq!(
            processor.correct(&too_long, unchanged_prediction).unwrap(),
            too_long
        );
        let paper = "The estimates are uniform for all positive parameters.  \n".repeat(2_000);
        let prepared = RULES.tokenize(&paper).join(" ");
        assert!(processor.encode_unbounded(&prepared).unwrap().ids.len() > 16_384);
        let mut completed = 0;
        assert_eq!(
            processor
                .correct_stream(&paper, unchanged_prediction, |chunk| {
                    completed += 1;
                    assert_eq!(chunk.completed, completed);
                    assert!(chunk.token_count <= TOKEN_LIMIT);
                    assert_eq!(chunk.corrected, &paper[chunk.byte_range]);
                    Ok(())
                })
                .unwrap(),
            paper
        );
        assert!(completed > 100);
        assert!(
            processor
                .correct(&"x".repeat(MAX_INPUT_BYTES + 1), |_| panic!(
                    "oversized paper reached model"
                ))
                .is_err()
        );
    }
}
