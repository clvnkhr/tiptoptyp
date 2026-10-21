use super::{Citation, Identifier, MAX_ENTRY_BYTES, bibtex, check_cancel};
use std::{
    collections::BTreeMap,
    io::Read,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

const RESPONSE_LIMIT: usize = 512 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Call only from a worker after the user explicitly requests a lookup.
pub(crate) fn lookup(query: &str, cancel: &AtomicBool) -> Result<Citation, String> {
    let identifier = Identifier::parse(query)?;
    check_cancel(cancel)?;
    let (url, accept) = request(&identifier);
    let body = fetch(url, accept, cancel)?;
    check_cancel(cancel)?;
    let bibtex = match &identifier {
        Identifier::Doi(doi) => {
            let entry = bibtex::parse_entry(body.trim())?;
            if entry.doi().as_deref() != Some(doi) {
                return Err("The DOI service returned a different or missing DOI. No citation was imported.".into());
            }
            entry.bibtex
        }
        Identifier::Arxiv(id) => arxiv_bibtex(&body, id)?,
    };
    let entry = bibtex::parse_entry(&bibtex)?;
    Ok(Citation {
        identifier,
        key: entry.key,
        bibtex: entry.bibtex,
    })
}

fn request(identifier: &Identifier) -> (url::Url, &'static str) {
    match identifier {
        Identifier::Doi(doi) => {
            let mut url = url::Url::parse("https://doi.org/").unwrap();
            url.path_segments_mut().unwrap().extend(doi.split('/'));
            (url, "application/x-bibtex")
        }
        Identifier::Arxiv(id) => {
            let mut url = url::Url::parse("https://export.arxiv.org/api/query").unwrap();
            url.query_pairs_mut()
                .append_pair("id_list", id)
                .append_pair("max_results", "1");
            (url, "application/atom+xml")
        }
    }
}

fn allowed_url(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.fragment().is_none()
        && matches!(
            url.host_str(),
            Some(
                "doi.org"
                    | "dx.doi.org"
                    | "data.crossref.org"
                    | "api.crossref.org"
                    | "data.datacite.org"
                    | "api.datacite.org"
                    | "data.crosscite.org"
                    | "citation.crosscite.org"
                    | "export.arxiv.org"
            )
        )
}

fn fetch(mut url: url::Url, accept: &str, cancel: &AtomicBool) -> Result<String, String> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    for _ in 0..=4 {
        check_cancel(cancel)?;
        if !allowed_url(&url) {
            return Err(
                "The citation service redirected outside the supported metadata services.".into(),
            );
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("Citation lookup timed out.")?;
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(remaining))
            .timeout_connect(Some(remaining.min(Duration::from_secs(5))))
            .timeout_recv_body(Some(remaining.min(Duration::from_secs(3))))
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let response = agent.get(url.as_str()).header("Accept", accept)
            .header("User-Agent", "tiptoptyp citation import (https://github.com/calvinkhor/mytypst)")
            .call().map_err(|_| "The citation service could not be reached. Check your connection and try again.")?;
        check_cancel(cancel)?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or("The citation service returned an invalid redirect.")?;
            url = url
                .join(location)
                .map_err(|_| "The citation service returned an invalid redirect.")?;
            continue;
        }
        if status == 404 {
            return Err("No citation was found for that identifier.".into());
        }
        if status == 429 {
            return Err("The citation service is busy. Wait a moment and try again.".into());
        }
        if status != 200 {
            return Err(format!(
                "The citation service returned HTTP {status}. Try again later."
            ));
        }
        let mut reader = response
            .into_body()
            .into_with_config()
            .limit((RESPONSE_LIMIT + 1) as u64)
            .reader();
        let mut bytes = Vec::new();
        let mut chunk = [0; 8192];
        loop {
            check_cancel(cancel)?;
            if Instant::now() >= deadline {
                return Err("Citation lookup timed out.".into());
            }
            let count = reader
                .read(&mut chunk)
                .map_err(|_| "The citation response could not be read.")?;
            if count == 0 {
                break;
            }
            if bytes.len() + count > RESPONSE_LIMIT {
                return Err("The citation response exceeds the 512 KiB limit.".into());
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        return String::from_utf8(bytes)
            .map_err(|_| "The citation service returned text that is not UTF-8.".into());
    }
    Err("The citation service returned too many redirects.".into())
}

fn arxiv_bibtex(source: &str, requested: &str) -> Result<String, String> {
    let fields = atom_fields(source)?;
    let get = |key: &str| {
        fields
            .get(key)
            .filter(|values| values.len() == 1)
            .and_then(|values| values.first())
    };
    let actual_identifier = get("id").and_then(|id| Identifier::parse(id).ok());
    let actual = get("id").and_then(|id| super::identifier::arxiv_identity(id));
    let exact_version = requested.rsplit_once('v').is_some_and(|(_, version)| {
        !version.is_empty() && version.bytes().all(|character| character.is_ascii_digit())
    });
    if actual != super::identifier::arxiv_identity(requested)
        || exact_version && actual_identifier != Some(Identifier::Arxiv(requested.to_owned()))
    {
        return Err("arXiv returned no matching article for that identifier.".into());
    }
    let title = get("title")
        .filter(|title| !title.is_empty())
        .ok_or("arXiv returned no article title.")?;
    let authors = fields
        .get("name")
        .filter(|names| !names.is_empty())
        .ok_or("arXiv returned no authors.")?;
    let published = get("published").ok_or("arXiv returned no publication date.")?;
    let year = published
        .get(..4)
        .filter(|year| year.bytes().all(|c| c.is_ascii_digit()))
        .ok_or("arXiv returned an invalid publication date.")?;
    let key = format!("arxiv{}", requested.replace('/', ":"));
    let mut output = format!(
        "@misc{{{key},\n  title = {{{{{}}}}},\n  author = {{{}}},\n  year = {{{year}}},\n  eprint = {{{requested}}},\n  archivePrefix = {{arXiv}},\n  url = {{https://arxiv.org/abs/{requested}}}",
        tex_value(title),
        authors
            .iter()
            .map(|name| tex_value(name))
            .collect::<Vec<_>>()
            .join(" and ")
    );
    if let Some(doi) = get("doi") {
        let doi = super::identifier::doi(doi)?;
        output.push_str(&format!(",\n  doi = {{{doi}}}"));
    }
    output.push_str("\n}");
    if output.len() > MAX_ENTRY_BYTES {
        return Err("The arXiv citation is too large.".into());
    }
    bibtex::parse_entry(&output)?;
    Ok(output)
}

fn tex_value(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            output.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            output.push(character);
            escaped = true;
            continue;
        }
        if matches!(character, '%' | '&' | '#') {
            output.push('\\');
        }
        output.push(character);
    }
    output
}

/// Deliberately small Atom reader: no DTD/entities, bounded depth, exactly one entry.
/// It keeps TeX in title text, decodes XML entities, and ignores unrelated feed metadata.
fn atom_fields(source: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let invalid = || "arXiv returned malformed article metadata.".to_owned();
    let mut fields = BTreeMap::<String, Vec<String>>::new();
    let mut stack = Vec::<String>::new();
    let mut cursor = 0;
    let mut entries = 0;
    while cursor < source.len() {
        if !source[cursor..].starts_with('<') {
            let end = source[cursor..]
                .find('<')
                .map_or(source.len(), |end| cursor + end);
            let text = xml_text(&source[cursor..end])?;
            if let Some(field) = atom_field(&stack) {
                let values = fields.entry(field.into()).or_default();
                if let Some(value) = values.last_mut() {
                    value.push_str(&text);
                }
            }
            cursor = end;
            continue;
        }
        if source[cursor..].starts_with("<!--") {
            cursor += source[cursor + 4..].find("-->").ok_or_else(invalid)? + 7;
            continue;
        }
        if source[cursor..].starts_with("<?") {
            cursor += source[cursor + 2..].find("?>").ok_or_else(invalid)? + 4;
            continue;
        }
        if source[cursor..].starts_with("<![CDATA[") {
            let start = cursor + 9;
            let end = start + source[start..].find("]]>").ok_or_else(invalid)?;
            if let Some(field) = atom_field(&stack)
                && let Some(value) = fields.get_mut(field).and_then(|values| values.last_mut())
            {
                value.push_str(&source[start..end]);
            }
            cursor = end + 3;
            continue;
        }
        if source[cursor..].starts_with("<!") {
            return Err(invalid());
        }
        let mut quote = None;
        let mut end = cursor + 1;
        for byte in source.as_bytes().iter().skip(cursor + 1) {
            if quote == Some(*byte) {
                quote = None;
            } else if quote.is_none() && matches!(byte, b'\'' | b'"') {
                quote = Some(*byte);
            } else if quote.is_none() && *byte == b'>' {
                break;
            }
            end += 1;
        }
        if source.as_bytes().get(end) != Some(&b'>') {
            return Err(invalid());
        }
        let tag = source[cursor + 1..end].trim();
        cursor = end + 1;
        if let Some(name) = tag.strip_prefix('/') {
            if stack.pop().as_deref() != Some(name.trim()) {
                return Err(invalid());
            }
        } else {
            let name = tag
                .split(|c: char| c.is_ascii_whitespace() || c == '/')
                .next()
                .unwrap_or("");
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b':' | b'_' | b'-' | b'.'))
            {
                return Err(invalid());
            }
            if stack.is_empty() && name != "feed" {
                return Err(invalid());
            }
            if name == "entry" && stack.len() == 1 {
                entries += 1;
                if entries > 1 {
                    return Err(invalid());
                }
            }
            stack.push(name.into());
            if stack.len() > 64 {
                return Err(invalid());
            }
            if let Some(field) = atom_field(&stack) {
                fields.entry(field.into()).or_default().push(String::new());
            }
            if tag.ends_with('/') {
                stack.pop();
            }
        }
    }
    if !stack.is_empty() || entries != 1 {
        return Err(invalid());
    }
    for values in fields.values_mut() {
        for value in values {
            *value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        }
    }
    Ok(fields)
}

fn atom_field(stack: &[String]) -> Option<&'static str> {
    let local: Vec<_> = stack
        .iter()
        .map(|name| name.rsplit(':').next().unwrap_or(name))
        .collect();
    match local.as_slice() {
        ["feed", "entry", "id"] => Some("id"),
        ["feed", "entry", "title"] => Some("title"),
        ["feed", "entry", "published"] => Some("published"),
        ["feed", "entry", "doi"] => Some("doi"),
        ["feed", "entry", "author", "name"] => Some("name"),
        _ => None,
    }
}

fn xml_text(source: &str) -> Result<String, String> {
    let mut output = String::new();
    let mut remainder = source;
    while let Some(start) = remainder.find('&') {
        output.push_str(&remainder[..start]);
        let end = remainder[start..]
            .find(';')
            .filter(|end| *end < 16)
            .ok_or("arXiv returned an invalid XML entity.")?
            + start;
        let entity = &remainder[start + 1..end];
        let character = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let number = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    entity
                        .strip_prefix('#')
                        .and_then(|decimal| decimal.parse().ok())
                };
                number
                    .and_then(char::from_u32)
                    .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
                    .ok_or("arXiv returned an invalid XML entity.")?
            }
        };
        output.push(character);
        remainder = &remainder[end + 1..];
    }
    output.push_str(remainder);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_are_encoded_and_redirects_cannot_reach_other_hosts() {
        let (url, accept) = request(&Identifier::parse("10.1000/a?b#c").unwrap());
        assert_eq!(url.as_str(), "https://doi.org/10.1000/a%3Fb%23c");
        assert_eq!(accept, "application/x-bibtex");
        let (url, _) = request(&Identifier::parse("10.1000/100%correct").unwrap());
        assert_eq!(url.as_str(), "https://doi.org/10.1000/100%25correct");
        for url in [
            "http://doi.org/a",
            "https://localhost/a",
            "https://doi.org.evil/a",
            "https://user@doi.org/a",
            "https://doi.org:123/a",
        ] {
            assert!(!allowed_url(&url::Url::parse(url).unwrap()));
        }
        let (url, _) = request(&Identifier::Arxiv("math/0301234v2".into()));
        assert_eq!(url.query(), Some("id_list=math%2F0301234v2&max_results=1"));
    }
    #[test]
    fn atom_metadata_preserves_math_and_authors_and_rejects_other_articles() {
        let source = r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom"><title>not the paper</title><entry><id>http://arxiv.org/abs/2401.01234v2</id><title>An $L^p$ estimate &amp; a &#x3B1; result</title><published>2024-01-03T00:00:00Z</published><author><name>A. Smith</name></author><author><name>B. Jones</name></author><arxiv:doi>10.1000/EXAMPLE</arxiv:doi></entry></feed>"#;
        let bib = arxiv_bibtex(source, "2401.01234v2").unwrap();
        assert!(bib.contains("An $L^p$ estimate \\& a α result"));
        assert!(bib.contains("A. Smith and B. Jones"));
        assert!(bib.contains("doi = {10.1000/example}"));
        assert!(arxiv_bibtex(source, "2401.99999").is_err());
        assert!(arxiv_bibtex(source, "2401.01234v1").is_err());
        assert!(atom_fields("<!DOCTYPE feed><feed/>").is_err());
        assert!(atom_fields("<feed><entry></feed>").is_err());
    }
}
