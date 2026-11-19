use crate::Error;
use alloc::{string::String, vec, vec::Vec};

pub(crate) const ADVERTISEMENT: &str = "application/x-git-upload-pack-advertisement";
pub(crate) const REQUEST: &str = "application/x-git-upload-pack-request";
pub(crate) const RESULT: &str = "application/x-git-upload-pack-result";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(&'static str, &'static str)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct Response<'a> {
    pub status: u16,
    pub content_type: Option<&'a str>,
    pub body: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    url: String,
}

impl Remote {
    pub fn new(url: &str) -> Result<Self, Error> {
        let rest = strip_scheme(url, "https://")
            .or_else(|| strip_scheme(url, "http://"))
            .ok_or(Error::InvalidUrl)?;
        let invalid = |byte: u8| byte <= b' ' || byte == 0x7f || byte == b'?' || byte == b'#';
        if rest.is_empty() || rest.starts_with('/') || url.bytes().any(invalid) {
            return Err(Error::InvalidUrl);
        }
        Ok(Self {
            url: url.trim_end_matches('/').into(),
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn discovery_request(&self) -> Request {
        Request {
            method: Method::Get,
            url: alloc::format!("{}/info/refs?service=git-upload-pack", self.url),
            headers: vec![("Accept", ADVERTISEMENT), ("Git-Protocol", "version=2")],
            body: Vec::new(),
        }
    }

    pub(crate) fn post(&self, body: Vec<u8>) -> Request {
        Request {
            method: Method::Post,
            url: alloc::format!("{}/git-upload-pack", self.url),
            headers: vec![
                ("Content-Type", REQUEST),
                ("Accept", RESULT),
                ("Git-Protocol", "version=2"),
            ],
            body,
        }
    }
}

fn strip_scheme<'a>(url: &'a str, scheme: &str) -> Option<&'a str> {
    let head = url.get(..scheme.len())?;
    head.eq_ignore_ascii_case(scheme)
        .then(|| &url[scheme.len()..])
}

pub(crate) fn body<'a>(response: &Response<'a>, content_type: &str) -> Result<&'a [u8], Error> {
    match response.status {
        200..=299 => {}
        401 | 403 => return Err(Error::AuthRequired(response.status)),
        404 => return Err(Error::NotFound),
        status => return Err(Error::Http(status)),
    }
    let media_type = response
        .content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if !media_type.is_some_and(|value| value.eq_ignore_ascii_case(content_type)) {
        return Err(Error::NotSmartHttp);
    }
    Ok(response.body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_urls_and_trims_trailing_slashes() {
        let remote = Remote::new("HTTPS://github.com/denoland/setup-deno.git/").unwrap();
        assert_eq!(remote.url(), "HTTPS://github.com/denoland/setup-deno.git");
        assert_eq!(
            remote.discovery_request().url,
            "HTTPS://github.com/denoland/setup-deno.git/info/refs?service=git-upload-pack"
        );
        assert_eq!(
            Remote::new("http://localhost:8080/repo")
                .unwrap()
                .post(Vec::new())
                .url,
            "http://localhost:8080/repo/git-upload-pack"
        );
    }

    #[test]
    fn rejects_other_schemes_queries_fragments_and_whitespace() {
        for url in [
            "ssh://github.com/a/b",
            "https://",
            "https:///path",
            "https://host/a?b",
            "https://host/a#b",
            "https://host/a b",
            "github.com/a/b",
        ] {
            assert_eq!(Remote::new(url), Err(Error::InvalidUrl), "{url}");
        }
    }

    #[test]
    fn maps_status_and_content_type() {
        let response = |status, content_type| Response {
            status,
            content_type,
            body: b"x",
        };
        assert_eq!(
            body(
                &response(200, Some("application/x-git-upload-pack-result; charset=x")),
                RESULT
            ),
            Ok(&b"x"[..])
        );
        assert_eq!(
            body(&response(401, None), RESULT),
            Err(Error::AuthRequired(401))
        );
        assert_eq!(
            body(&response(403, None), RESULT),
            Err(Error::AuthRequired(403))
        );
        assert_eq!(body(&response(404, None), RESULT), Err(Error::NotFound));
        assert_eq!(body(&response(500, None), RESULT), Err(Error::Http(500)));
        assert_eq!(
            body(&response(200, Some("text/plain")), RESULT),
            Err(Error::NotSmartHttp)
        );
        assert_eq!(body(&response(200, None), RESULT), Err(Error::NotSmartHttp));
    }
}
