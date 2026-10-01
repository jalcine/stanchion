//! Log sanitization for untrusted plugin text.
//!
//! A plugin's message is untrusted: without this it could embed newlines to forge log
//! lines attributed to other plugins or the host, emit ANSI/OSC escape sequences that
//! rewrite the terminal, or log unbounded text. See #40.

/// Largest log message the built-in `log` provider will emit, in characters.
pub const MAX_LOG_MESSAGE: usize = 4096;

/// Escapes control characters and caps length before a plugin's text reaches stderr.
pub fn sanitize_log(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_LOG_MESSAGE));
    let mut truncated = false;
    for (seen, ch) in text.chars().enumerate() {
        if seen >= MAX_LOG_MESSAGE {
            truncated = true;
            break;
        }
        if ch.is_control() {
            out.extend(ch.escape_default());
        } else {
            out.push(ch);
        }
    }
    if truncated {
        out.push_str("…(truncated)");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{MAX_LOG_MESSAGE, sanitize_log};

    #[test]
    fn escapes_newlines_and_escape_sequences() {
        let sanitized = sanitize_log("ok\n[trusted] granted admin\x1b[2J");
        assert!(!sanitized.contains('\n'));
        assert!(!sanitized.contains('\x1b'));
        assert!(sanitized.contains("\\n"));
    }

    #[test]
    fn ordinary_text_is_unchanged() {
        assert_eq!(sanitize_log("hello world 123"), "hello world 123");
    }

    #[test]
    fn long_message_is_truncated() {
        let sanitized = sanitize_log(&"a".repeat(MAX_LOG_MESSAGE * 2));
        assert!(sanitized.ends_with("…(truncated)"));
    }
}
