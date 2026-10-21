use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Identifier {
    Doi(String),
    Arxiv(String),
}

impl Identifier {
    pub(crate) fn parse(input: &str) -> Result<Self, String> {
        let mut value = input.trim().to_owned();
        if value.len() > 1024 || value.chars().any(char::is_control) {
            return Err("Enter one DOI or arXiv identifier or URL.".into());
        }
        if value.starts_with("https://") || value.starts_with("http://") {
            let url = url::Url::parse(&value).map_err(|_| "The citation URL is invalid.")?;
            if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
                return Err(
                    "Use an official DOI or arXiv URL without credentials or a port.".into(),
                );
            }
            let path = percent_decode(url.path())?;
            match url.host_str().unwrap_or("") {
                "doi.org" | "dx.doi.org" | "www.doi.org" => {
                    // DOI URL query/fragment delimiters must be encoded if part of the DOI.
                    if url.query().is_some() || url.fragment().is_some() {
                        return Err("The DOI URL must not contain a query or fragment.".into());
                    }
                    value = path.trim_start_matches('/').to_owned();
                }
                "arxiv.org" | "www.arxiv.org" | "export.arxiv.org" => {
                    let id = path
                        .strip_prefix("/abs/")
                        .or_else(|| path.strip_prefix("/pdf/"))
                        .or_else(|| path.strip_prefix("/html/"))
                        .ok_or("Use an arXiv abstract, PDF, or HTML article URL.")?;
                    return arxiv(id.strip_suffix(".pdf").unwrap_or(id)).map(Self::Arxiv);
                }
                _ => return Err("Only DOI and arXiv article URLs are supported.".into()),
            }
        }
        if value
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("doi:"))
        {
            value = value[4..].trim().to_owned();
        }
        if value
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("arxiv:"))
        {
            return arxiv(value[6..].trim()).map(Self::Arxiv);
        }
        if value.starts_with("10.") {
            doi(&value).map(Self::Doi)
        } else {
            arxiv(&value).map(Self::Arxiv)
        }
    }

    pub(crate) fn value(&self) -> &str {
        match self {
            Self::Doi(value) | Self::Arxiv(value) => value,
        }
    }
}

pub(super) fn doi(value: &str) -> Result<String, String> {
    let value = value.trim();
    let Some((prefix, suffix)) = value.split_once('/') else {
        return Err("A DOI must look like 10.1234/article-id.".into());
    };
    let registrant = prefix.strip_prefix("10.").unwrap_or("");
    if !(4..=9).contains(&registrant.len())
        || !registrant.bytes().all(|c| c.is_ascii_digit())
        || suffix.is_empty()
        || value.len() > 512
        || value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '\\'))
    {
        return Err("A DOI must look like 10.1234/article-id, without spaces.".into());
    }
    Ok(value.to_ascii_lowercase())
}

pub(super) fn arxiv(value: &str) -> Result<String, String> {
    static MODERN: OnceLock<regex::Regex> = OnceLock::new();
    static LEGACY: OnceLock<regex::Regex> = OnceLock::new();
    let modern = MODERN.get_or_init(|| {
        regex::Regex::new(r"^([0-9]{2})([0-9]{2})\.[0-9]{4,5}(?:v[1-9][0-9]*)?$").unwrap()
    });
    let legacy = LEGACY.get_or_init(|| {
        regex::Regex::new(
            r"^[a-z][a-z-]*(?:\.[A-Za-z]{2})?/([0-9]{2})([0-9]{2})[0-9]{3}(?:v[1-9][0-9]*)?$",
        )
        .unwrap()
    });
    let captures = modern
        .captures(value)
        .or_else(|| legacy.captures(value))
        .ok_or("An arXiv identifier must look like 2401.01234 or math/0301234.")?;
    let month: u8 = captures[2].parse().unwrap_or(0);
    if !(1..=12).contains(&month) {
        return Err("The arXiv identifier has an invalid month.".into());
    }
    Ok(value.to_owned())
}

pub(super) fn arxiv_identity(value: &str) -> Option<String> {
    let Identifier::Arxiv(value) = Identifier::parse(value).ok()? else {
        return None;
    };
    let base = value
        .rsplit_once('v')
        .filter(|(_, version)| version.bytes().all(|c| c.is_ascii_digit()))
        .map_or(value.as_str(), |(base, _)| base);
    Some(base.to_ascii_lowercase())
}

pub(super) fn percent_decode(value: &str) -> Result<String, String> {
    let mut output = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = bytes
                .get(index + 1..index + 3)
                .ok_or("The citation URL has invalid percent encoding.")?;
            let text = std::str::from_utf8(digits)
                .map_err(|_| "The citation URL has invalid percent encoding.")?;
            output.push(
                u8::from_str_radix(text, 16)
                    .map_err(|_| "The citation URL has invalid percent encoding.")?,
            );
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).map_err(|_| "The citation identifier is not UTF-8.".into())
}
