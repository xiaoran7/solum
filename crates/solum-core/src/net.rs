//! Outbound endpoint policy — one rule, one place.
//!
//! Every configurable endpoint in Solum carries a credential *and* personal
//! content: the LLM base URL sends the API key plus the user's own words, the
//! sync relay sends a bearer token, Soulous sends access/refresh tokens. Plain
//! `http://` on any of them is a cleartext credential on the wire and a
//! trivially rewritable response coming back.
//!
//! So the rule is: **HTTPS, unless the host is unambiguously this machine.**
//! Loopback is exempt because a local relay or a local model server has no
//! network hop to intercept, and requiring certificates there would push people
//! toward disabling verification entirely — a worse outcome. The exemption is
//! deliberately narrow: literal `127.0.0.0/8`, `[::1]`, and `localhost`. It is
//! *not* extended to private LAN ranges (`192.168.*`, `10.*`), which do have a
//! hop and are exactly where a hostile device on the same Wi-Fi lives.

use crate::error::{CoreError, Result};

fn is_loopback_host(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(value) => value.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(value) => value.is_loopback(),
        url::Host::Ipv6(value) => value.is_loopback(),
    }
}

/// Validate a user-configured endpoint. `what` names the setting for the error
/// message (e.g. `"LLM base_url"`).
///
/// Returns the trimmed URL with any trailing slash removed, so callers can use
/// this as their single normalization step too.
pub fn validate_endpoint(url: &str, what: &str) -> Result<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let parsed =
        url::Url::parse(trimmed).map_err(|_| CoreError::Invalid(format!("{what} 不是有效 URL")))?;
    let host = parsed
        .host()
        .ok_or_else(|| CoreError::Invalid(format!("{what} 缺少主机名")))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(CoreError::Invalid(format!("{what} 不能包含用户名或密码")));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(CoreError::Invalid(format!("{what} 不能包含查询参数或片段")));
    }
    match parsed.scheme() {
        "https" => Ok(parsed.as_str().trim_end_matches('/').to_string()),
        "http" if is_loopback_host(host) => Ok(parsed.as_str().trim_end_matches('/').to_string()),
        "http" => {
            let host = parsed.host_str().unwrap_or("");
            Err(CoreError::Invalid(format!(
                "{what} 必须使用 https://。明文 http:// 会把密钥和个人内容暴露在网络上，\
                 只有本机地址（localhost / 127.0.0.1 / [::1]）可以例外，当前是 {host}"
            )))
        }
        _ => Err(CoreError::Invalid(format!(
            "{what} 需要以 https:// 开头（本机调试可用 http://localhost）"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_is_accepted_and_normalized() {
        assert_eq!(
            validate_endpoint("  https://api.example.com/  ", "x").unwrap(),
            "https://api.example.com"
        );
    }

    #[test]
    fn plain_http_to_the_internet_is_refused() {
        for bad in [
            "http://api.example.com",
            "http://192.168.1.9:8787",
            "http://10.0.0.4",
            "http://127.0.0.1.evil.com", // suffix trick, not loopback
        ] {
            assert!(validate_endpoint(bad, "x").is_err(), "should refuse {bad}");
        }
    }

    #[test]
    fn the_host_is_what_follows_the_userinfo() {
        // API endpoints never need embedded credentials, regardless of host.
        assert!(validate_endpoint("http://127.0.0.1@evil.com", "x").is_err());
        assert!(validate_endpoint("http://evil.com@127.0.0.1", "x").is_err());
    }

    #[test]
    fn loopback_over_http_is_allowed() {
        for ok in [
            "http://localhost:8787",
            "http://127.0.0.1:11434",
            "http://127.1.2.3",
            "http://[::1]:8787",
        ] {
            assert!(validate_endpoint(ok, "x").is_ok(), "should allow {ok}");
        }
    }

    #[test]
    fn other_schemes_are_refused() {
        assert!(validate_endpoint("ftp://x.com", "x").is_err());
        assert!(validate_endpoint("api.example.com", "x").is_err());
    }

    #[test]
    fn malformed_hosts_queries_and_fragments_are_refused() {
        for bad in [
            "http://127.0.0.999",
            "http://[::1].evil.example",
            "https://api.example.com/v1?token=x",
            "https://api.example.com/v1#fragment",
        ] {
            assert!(validate_endpoint(bad, "x").is_err(), "should refuse {bad}");
        }
    }
}
