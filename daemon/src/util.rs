//! Small dependency-free helpers: percent-decoding, constant-time compare,
//! XML escaping for the launchd plist.

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Decode `%XX` escapes in a query value. `+` is left literal (the producer is a
/// LocatorJS template that does not form-encode, so `+` means a literal `+`).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Constant-time byte comparison (length-dependent only, which is fine: tokens
/// are fixed-width hex).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Parse a query string into (key, decoded-value) pairs.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_string(), percent_decode(v)),
            None => (pair.to_string(), String::new()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_basics() {
        assert_eq!(percent_decode("/a/b.tsx"), "/a/b.tsx");
        assert_eq!(percent_decode("%2Fa%2Fb%20c.tsx"), "/a/b c.tsx");
        assert_eq!(percent_decode("a%3A12%3A5"), "a:12:5");
        // trailing/invalid percent left as-is
        assert_eq!(percent_decode("a%"), "a%");
        assert_eq!(percent_decode("a%zz"), "a%zz");
        // plus stays literal
        assert_eq!(percent_decode("foo+bar"), "foo+bar");
    }

    #[test]
    fn ct_eq_works() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn query_parse() {
        let q = parse_query("token=ab%20c&file=%2Fx%3A3%3A4");
        assert_eq!(q, vec![
            ("token".to_string(), "ab c".to_string()),
            ("file".to_string(), "/x:3:4".to_string()),
        ]);
    }

    #[test]
    fn xml_escapes() {
        assert_eq!(xml_escape("a&b<c>\"d'"), "a&amp;b&lt;c&gt;&quot;d&apos;");
    }
}
