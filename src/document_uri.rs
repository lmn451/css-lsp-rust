use ls_types::Uri;

/// A lossless file-URI path key, independent of percent-encoding spelling.
/// Preserve the scheme, authority and path separators; do not resolve filesystem aliases.
/// Protocol records retain the original URI. Non-file URIs keep exact identity.
pub(crate) fn document_uri_key(uri: &Uri) -> Uri {
    if !uri.scheme().as_str().eq_ignore_ascii_case("file")
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return uri.clone();
    }
    let path = uri.path().as_str();
    let mut key = uri.as_str()[..uri.as_str().len() - path.len()].to_owned();
    let mut index = 0;
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    while index < path.len() {
        let byte = path.as_bytes()[index];
        let escaped = byte == b'%';
        // Uri parsing guarantees that every percent escape has two ASCII hex digits.
        let byte = if escaped {
            index += 2;
            u8::from_str_radix(&path[index - 1..=index], 16).expect("valid URI escape")
        } else {
            byte
        };
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || (byte == b'/' && !escaped) {
            key.push(char::from(byte));
        } else {
            key.push('%');
            key.push(char::from(HEX[usize::from(byte >> 4)]));
            key.push(char::from(HEX[usize::from(byte & 15)]));
        }
        index += 1;
    }
    key.parse().unwrap_or_else(|_| uri.clone())
}

pub(crate) fn same_document(left: &Uri, right: &Uri) -> bool {
    left == right || document_uri_key(left) == document_uri_key(right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn file_identity_ignores_percent_encoding_spelling() {
        for (literal, encoded) in [
            (
                "file:///workspace/bang!.css",
                "file:///workspace/bang%21.css",
            ),
            (
                "file:///C:/workspace/bang!.css",
                "file:///C%3A/workspace/bang%21.css",
            ),
            (
                "file:///C:/workspace/bang!.css",
                "file:///C%3a/workspace/bang%21.css",
            ),
        ] {
            let literal = Uri::from_str(literal).unwrap();
            let encoded = Uri::from_str(encoded).unwrap();
            assert_ne!(literal, encoded);
            assert!(same_document(&literal, &encoded));
            let key = document_uri_key(&literal);
            assert_eq!(key, document_uri_key(&encoded));
            assert_eq!(key, document_uri_key(&key));
        }
    }

    #[test]
    fn distinct_documents_are_not_merged() {
        let uri = Uri::from_str("file:///workspace/bang!.css").unwrap();
        for other in [
            "file:///workspace/other.css",
            "file://other-host/workspace/bang!.css",
            "file:///workspace/bang%2521.css",
            "file:///workspace/bang!.css?version=1",
            "file:///workspace/bang!.css#style",
        ] {
            assert!(!same_document(&uri, &Uri::from_str(other).unwrap()));
        }
        let invalid_utf8_a = Uri::from_str("file:///workspace/%FF.css").unwrap();
        let invalid_utf8_b = Uri::from_str("file:///workspace/%FE.css").unwrap();
        assert!(!same_document(&invalid_utf8_a, &invalid_utf8_b));
        assert!(!same_document(
            &Uri::from_str("file:///workspace/a%2Fb.css").unwrap(),
            &Uri::from_str("file:///workspace/a/b.css").unwrap(),
        ));
        let virtual_uri = Uri::from_str("untitled:///bang%21.css").unwrap();
        assert_eq!(document_uri_key(&virtual_uri), virtual_uri);
        assert!(!same_document(
            &virtual_uri,
            &Uri::from_str("untitled:///bang!.css").unwrap()
        ));
    }
}
