//! Best-effort scrubbing so logs can be pasted into an issue or a chat without
//! leaking the public hostname, the Windows user name or token-like strings.

const HOST_SUFFIXES: [&str; 2] = [".ts.net", ".trycloudflare.com"];
const USERS_MARKER: &str = "\\users\\";
const MIN_SECRET_LEN: usize = 32;

pub fn redact(text: &str) -> String {
    redact_tokens(&redact_user_dirs(&redact_hosts(text)))
}

fn is_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '-'
}

fn redact_hosts(text: &str) -> String {
    HOST_SUFFIXES.iter().fold(text.to_owned(), |out, suffix| {
        redact_host_suffix(&out, suffix)
    })
}

fn redact_host_suffix(text: &str, suffix: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(suffix) {
        let host_start = rest[..index]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_host_char(*c))
            .last()
            .map_or(index, |(position, _)| position);
        out.push_str(&rest[..host_start]);
        out.push_str("<host>");
        out.push_str(suffix);
        rest = &rest[index + suffix.len()..];
    }
    out.push_str(rest);
    out
}

fn redact_user_dirs(text: &str) -> String {
    // ASCII lowercasing keeps byte offsets identical to `text`.
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(found) = lower[cursor..].find(USERS_MARKER) {
        let name_start = cursor + found + USERS_MARKER.len();
        let name_len = text[name_start..]
            // No space here on purpose: "Maria Silva" is a valid user folder, and
            // hiding a little too much beats leaking a surname.
            .find(['\\', '/', '"', '\'', ')', ',', ';', '\r', '\n'])
            .unwrap_or(text.len() - name_start);
        out.push_str(&text[cursor..name_start]);
        out.push_str("<usuario>");
        cursor = name_start + name_len;
    }
    out.push_str(&text[cursor..]);
    out
}

fn looks_secret(token: &str) -> bool {
    token.len() >= MIN_SECRET_LEN
        && token.chars().any(|c| c.is_ascii_digit())
        && token.chars().any(|c| c.is_ascii_alphabetic())
}

fn flush(out: &mut String, token: &mut String) {
    if looks_secret(token) {
        out.push_str("<redigido>");
    } else {
        out.push_str(token);
    }
    token.clear();
}

fn redact_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut token = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '=') {
            token.push(c);
        } else {
            flush(&mut out, &mut token);
            out.push(c);
        }
    }
    flush(&mut out, &mut token);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostnames_are_hidden() {
        assert_eq!(
            redact("HTTPS: https://ompo.tail53b0c1.ts.net/healthz => Ok(200)"),
            "HTTPS: https://<host>.ts.net/healthz => Ok(200)"
        );
        assert_eq!(
            redact("a.ts.net e b.c.ts.net"),
            "<host>.ts.net e <host>.ts.net"
        );
        assert_eq!(redact("termina em .ts.net"), "termina em <host>.ts.net");
        assert_eq!(
            redact("https://lone-expression-boats-atm.trycloudflare.com/mcp"),
            "https://<host>.trycloudflare.com/mcp"
        );
    }
    #[test]
    fn windows_user_names_are_hidden() {
        assert_eq!(
            redact(r"Raiz: C:\Users\maico\OneDrive\GitHub"),
            r"Raiz: C:\Users\<usuario>\OneDrive\GitHub"
        );
        assert_eq!(redact(r"c:\USERS\Maria Silva\x"), r"c:\USERS\<usuario>\x");
    }
    #[test]
    fn token_like_strings_are_hidden() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4ifQ.abcdefghijklmnopqrstuvwxyz0123456789";
        let output = redact(&format!("Authorization: Bearer {jwt}"));
        assert!(output.contains("<redigido>"));
        assert!(!output.contains("eyJzdWIi"));
        assert!(!output.contains("abcdefghijklmnopqrstuvwxyz0123456789"));
    }
    #[test]
    fn ordinary_log_lines_are_untouched() {
        for line in [
            "Instalação concluída.",
            "Pasta autorizada na próxima instalação: D:\\projetos\\app",
            "devspace-20261119-120000.out.log",
            "Local: http://127.0.0.1:7676/healthz => Ok(200)",
        ] {
            assert_eq!(redact(line), line);
        }
    }
}
