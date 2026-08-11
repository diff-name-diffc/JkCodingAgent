/// Normalizes a URL before handing it to the browser engine.
///
/// Chromium is the authority on supported schemes. Keeping a scheme allowlist here
/// would reject valid targets such as `file:`, `data:`, and `about:` and would drift
/// whenever the browser gains support for another URL type.
pub(crate) fn normalize_browser_url(url: String) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("URL 不能为空".to_string());
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::normalize_browser_url;

    #[test]
    fn accepts_urls_supported_by_the_browser_engine() {
        for url in [
            "https://example.com",
            "http://localhost:1420",
            "file:///tmp/formula_ui_test.html",
            "data:text/html,<h1>hello</h1>",
            "about:blank",
        ] {
            assert_eq!(normalize_browser_url(url.to_string()).unwrap(), url);
        }
    }

    #[test]
    fn trims_url_and_rejects_empty_input() {
        assert_eq!(
            normalize_browser_url("  file:///tmp/page.html  ".to_string()).unwrap(),
            "file:///tmp/page.html"
        );
        assert_eq!(
            normalize_browser_url("  ".to_string()).unwrap_err(),
            "URL 不能为空"
        );
    }
}
