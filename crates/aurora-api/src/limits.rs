//! Input bounds for every caller-supplied value on the OAuth surface.
//!
//! The browser adapter's handlers are reachable by anything that can talk to
//! the loopback listener, so nothing an attacker can choose is used before it
//! has been length- and charset-validated here. Keeping the rules in one place
//! stops the handlers from drifting apart: a value rejected as a cookie must
//! not be accepted as a query parameter.
//!
//! Every rule is "fixed length range, fixed alphabet". Length alone is not a
//! bound worth trusting: `str::len` counts bytes, so a `len() <= 128` check
//! admits 128 bytes of arbitrary Unicode — including quotes, control bytes and
//! path separators — straight into a hash or a SQL binding.

/// Session cookie tokens are 22 base64url characters (a base64url-encoded
/// 128-bit UUID, as issued by [`crate::random`]). The accepted window is wider
/// than what this build issues so a rotated token shape still validates, but it
/// is narrow and admits nothing outside the base64url alphabet.
pub(crate) const SESSION_TOKEN_LEN: std::ops::RangeInclusive<usize> = 16..=128;
/// CSRF tokens share the session token's shape.
pub(crate) const CSRF_TOKEN_LEN: std::ops::RangeInclusive<usize> = 16..=128;
/// A one-shot `state` is issued by the same generator as a session token.
pub(crate) const STATE_LEN: std::ops::RangeInclusive<usize> = 16..=256;
/// Authorization codes are opaque; providers issue short tokens. Bounded so a
/// caller cannot make us hold an arbitrarily large string and put it into an
/// outbound form body.
pub(crate) const CODE_LEN: std::ops::RangeInclusive<usize> = 1..=2048;
/// Provider playlist identifiers: Spotify base-62 ids, YouTube `PL`/`UU` ids.
/// The separator set is what the playlist endpoints actually use.
pub(crate) const PLAYLIST_ID_LEN: std::ops::RangeInclusive<usize> = 1..=256;
/// A Spotify `offset` cursor is a decimal byte offset into a playlist.
pub(crate) const CURSOR_MAX: u32 = 100_000;
/// A YouTube `pageToken` is an opaque base64url blob.
pub(crate) const PAGE_TOKEN_LEN: std::ops::RangeInclusive<usize> = 1..=512;

/// True when `raw` is within `len` and consists only of ASCII alphanumerics
/// plus `-_` (the base64url alphabet without padding).
pub(crate) fn is_base64url_token(raw: &str, len: std::ops::RangeInclusive<usize>) -> bool {
    len.contains(&raw.len()) && raw.bytes().all(is_base64url_byte)
}

fn is_base64url_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// A session cookie token must be exactly a base64url token of the accepted
/// length. Anything else — padding, `=`, a `;`, a quote, whitespace, a
/// non-ASCII byte, a multi-byte character — is rejected rather than truncated
/// or hashed.
pub(crate) fn is_session_token(raw: &str) -> bool {
    is_base64url_token(raw, SESSION_TOKEN_LEN)
}

/// Same shape as a session token; kept separate so the two can be tightened
/// independently without one silently inheriting the other's bound.
pub(crate) fn is_user_token(raw: &str) -> bool {
    is_base64url_token(raw, SESSION_TOKEN_LEN)
}

pub(crate) fn is_csrf_token(raw: &str) -> bool {
    is_base64url_token(raw, CSRF_TOKEN_LEN)
}

/// An OAuth `state` value: the one-shot correlation token from the begin step.
pub(crate) fn is_oauth_state(raw: &str) -> bool {
    is_base64url_token(raw, STATE_LEN)
}

/// An authorization code. Opaque to us, so the rule is a byte range plus a
/// charset that cannot smuggle a newline or a form separator into the outbound
/// request body.
pub(crate) fn is_authorization_code(raw: &str) -> bool {
    CODE_LEN.contains(&raw.len())
        && raw
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'&' && b != b'=')
}

/// A provider playlist identifier.
pub(crate) fn is_playlist_id(raw: &str) -> bool {
    PLAYLIST_ID_LEN.contains(&raw.len())
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// Spotify's `offset` pagination cursor: a canonical decimal integer within a
/// fixed range. Sign, leading zeros, whitespace, non-ASCII digits and any
/// non-digit byte are rejected; the value is also capped, so a caller cannot
/// ask the provider to index arbitrarily far into a playlist.
pub(crate) fn is_spotify_cursor(raw: &str) -> bool {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if raw.len() > 1 && raw.starts_with('0') {
        return false;
    }
    raw.parse::<u32>().is_ok_and(|n| n <= CURSOR_MAX)
}

/// YouTube's `pageToken` pagination cursor.
pub(crate) fn is_page_token(raw: &str) -> bool {
    is_base64url_token(raw, PAGE_TOKEN_LEN)
}

/// The yt-dlp media-source allowlist, shared by every path that validates a
/// media URL (Tauri commands in `aurora-tauri`, the browser media endpoints
/// here). One list, one definition: if a host is legal anywhere it is legal
/// everywhere, and adding a source means editing this table (and its test).
///
/// Deliberately HTTPS-only sources that host content the user can legitimately
/// download: the Internet Archive and YouTube's public video endpoints. This
/// is the *policy* allowlist, not a technical restriction — extending it is a
/// product decision about what Aurora will fetch, and it must not be widened
/// to work around a failure.
pub(crate) const MEDIA_ALLOWED_HOSTS: [&str; 5] = [
    "archive.org",
    "www.archive.org",
    "youtube.com",
    "www.youtube.com",
    "youtu.be",
];

/// A media-source URL: HTTPS, host on the allowlist, no userinfo, and a
/// bounded total length so a hostile caller cannot hand us a multi-kilobyte
/// "URL" to hold or spawn.
pub(crate) fn is_media_url(raw: &str) -> bool {
    const MAX_URL_LEN: usize = 2048;
    if raw.len() > MAX_URL_LEN {
        return false;
    }
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url
            .host_str()
            .is_some_and(|host| MEDIA_ALLOWED_HOSTS.contains(&host.to_ascii_lowercase().as_str()))
}

/// A pagination cursor, dispatched on the provider that produced it.
pub(crate) fn is_cursor(provider: &str, raw: &str) -> bool {
    match provider {
        "spotify" => is_spotify_cursor(raw),
        "youtube-music" => is_page_token(raw),
        _ => false,
    }
}

/// Upper bound on a request body this service will read.
///
/// The OAuth surface takes no request bodies: `begin`, `callback`, `status` and
/// `disconnect` are parameterless. The bound exists so that a request which
/// does carry a body cannot make the process buffer it.
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 8 * 1024;

/// Upper bound on a provider response body read.
pub(crate) const MAX_PROVIDER_BODY_BYTES: usize = 512 * 1024;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn session_tokens_reject_everything_that_is_not_base64url() {
        assert!(is_session_token("abcdefghijklmnop"));
        assert!(is_session_token(&"A".repeat(128)));
        assert!(is_session_token("A-_b9abcdefghijkl"));

        // Padding, separators, quotes, whitespace and control bytes: the
        // characters a length-only check would have waved through.
        for hostile in [
            "abcdefghijklmno=",
            "abcdefghijklmnop;",
            "abcdefghijklmnop ",
            "abcdefghijklmnop\n",
            "abc\u{7f}defghijklmnop",
            "abcdefghijklmnop\u{0}",
            "abcdefghijklmnop\"",
            "../abcdefghijklmnop",
            "abcdefghijklmnop+",
            "abcdefghijklmnop/",
            "\u{430}\u{431}\u{432}\u{433}\u{434}\u{435}\u{436}\u{437}\u{438}\u{439}\u{43a}\u{43b}\u{43c}\u{43d}\u{43e}\u{43f}",
            "abcdefg\u{e9}hijklmnop",
        ] {
            assert!(!is_session_token(hostile), "accepted {hostile:?}");
        }

        // Length bounds still apply on both ends.
        assert!(!is_session_token(""));
        assert!(!is_session_token(&"A".repeat(15)));
        assert!(!is_session_token(&"A".repeat(129)));
    }

    #[test]
    fn csrf_and_state_tokens_use_the_same_base64url_rule() {
        assert!(is_csrf_token("abcdefghijklmnop"));
        assert!(!is_csrf_token("abcdefghijklmnop="));
        assert!(is_oauth_state("abcdefghijklmnop"));
        assert!(!is_oauth_state("abcdefghijklmnop;"));
        assert!(!is_oauth_state(""));
        assert!(!is_oauth_state(&"A".repeat(257)));
    }

    #[test]
    fn authorization_codes_cannot_smuggle_form_separators() {
        assert!(is_authorization_code("AQDf-1234_abcd"));
        assert!(is_authorization_code("4/0AX42cR"));
        assert!(!is_authorization_code(""));
        assert!(!is_authorization_code("code&grant_type=refresh_token"));
        assert!(!is_authorization_code("code=injected"));
        assert!(!is_authorization_code("code\nInjected: header"));
        assert!(!is_authorization_code(&"a".repeat(2049)));
    }

    #[test]
    fn playlist_ids_are_bounded_and_charset_limited() {
        assert!(is_playlist_id("37i9dQZF1DXcBWIGoYBM5M"));
        assert!(is_playlist_id("PLBCF2DAC6FFB574DE"));
        assert!(is_playlist_id("abc:def_ghi-jkl.mno"));
        assert!(!is_playlist_id(""));
        assert!(!is_playlist_id("../../etc/passwd"));
        assert!(!is_playlist_id("abc/def"));
        assert!(!is_playlist_id("abc def"));
        assert!(!is_playlist_id("abc\"def"));
        assert!(!is_playlist_id("\u{430}\u{431}\u{432}"));
        assert!(!is_playlist_id(&"a".repeat(257)));
    }

    #[test]
    fn spotify_cursors_are_canonical_decimal_offsets_in_range() {
        assert!(is_spotify_cursor("0"));
        assert!(is_spotify_cursor("100"));
        assert!(is_spotify_cursor("100000"));
        assert!(!is_spotify_cursor(""));
        assert!(!is_spotify_cursor("100001"));
        assert!(!is_spotify_cursor("4294967296"));
        assert!(!is_spotify_cursor("-1"));
        assert!(!is_spotify_cursor("+5"));
        assert!(!is_spotify_cursor("007"));
        assert!(!is_spotify_cursor("1e3"));
        assert!(!is_spotify_cursor("1 0"));
        assert!(!is_spotify_cursor(&"9".repeat(64)));
    }

    #[test]
    fn page_tokens_are_bounded_base64url() {
        assert!(is_page_token("CAEQABgE"));
        assert!(!is_page_token(""));
        assert!(!is_page_token("CAEQABgE="));
        assert!(!is_page_token("CAE QABgE"));
        assert!(!is_page_token(&"A".repeat(513)));
    }

    #[test]
    fn cursors_are_dispatched_per_provider() {
        assert!(is_cursor("spotify", "100"));
        assert!(!is_cursor("spotify", "CAEQABgE"));
        assert!(is_cursor("youtube-music", "CAEQABgE"));
        // Digits are base64url, so a Spotify-shaped decimal offset is a
        // structurally valid page token; only its charset/length bounds are
        // enforced, not its "shape".
        assert!(is_cursor("youtube-music", "1000000000000000"));
        assert!(!is_cursor("youtube-music", "1 0"));
        assert!(!is_cursor("apple-music", "1"));
    }
}
