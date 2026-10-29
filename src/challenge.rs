use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChallengeKind {
    CloudflareChallenge,
    Turnstile,
    HCaptcha,
    ReCaptcha,
}

pub fn detect(
    status: Option<u16>,
    headers: &[(String, String)],
    html: &str,
) -> Option<ChallengeKind> {
    let has_header = |name: &str, value: &str| {
        headers.iter().any(|(key, val)| {
            key.eq_ignore_ascii_case(name) && val.trim().eq_ignore_ascii_case(value)
        })
    };
    if has_header("cf-mitigated", "challenge")
        || (matches!(status, Some(403 | 503))
            && has_header("server", "cloudflare")
            && html.contains("Just a moment"))
    {
        Some(ChallengeKind::CloudflareChallenge)
    } else if html.contains("challenges.cloudflare.com/turnstile")
        || html.contains("class=\"cf-turnstile\"")
    {
        Some(ChallengeKind::Turnstile)
    } else if html.contains("hcaptcha.com/1/api.js") || html.contains("class=\"h-captcha\"") {
        Some(ChallengeKind::HCaptcha)
    } else if html.contains("/recaptcha/api.js")
        || html.contains("/recaptcha/enterprise.js")
        || html.contains("class=\"g-recaptcha\"")
    {
        Some(ChallengeKind::ReCaptcha)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(name: &str, value: &str) -> Vec<(String, String)> {
        vec![(name.into(), value.into())]
    }

    #[test]
    fn hcaptcha_privacy_link_is_not_challenge() {
        assert_eq!(
            detect(None, &[], r#"<a href="https://www.hcaptcha.com/privacy">"#),
            None
        );
    }

    #[test]
    fn cf_header_case_and_space() {
        assert_eq!(
            detect(
                Some(403),
                &headers("Server", " Cloudflare "),
                "Just a moment..."
            ),
            Some(ChallengeKind::CloudflareChallenge)
        );
    }

    #[test]
    fn cf_header() {
        for status in [None, Some(200), Some(403), Some(503)] {
            assert_eq!(
                detect(
                    status,
                    &headers("Cf-MiTiGaTeD", "challenge"),
                    "class=\"cf-turnstile\" class=\"h-captcha\" class=\"g-recaptcha\""
                ),
                Some(ChallengeKind::CloudflareChallenge)
            );
        }
        for kind in [
            ChallengeKind::CloudflareChallenge,
            ChallengeKind::Turnstile,
            ChallengeKind::HCaptcha,
            ChallengeKind::ReCaptcha,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(serde_json::from_str::<ChallengeKind>(&json).unwrap(), kind);
        }
    }

    #[test]
    fn cf_interstitial() {
        for status in [403, 503] {
            assert_eq!(
                detect(
                    Some(status),
                    &headers("SeRvEr", "cloudflare"),
                    "<title>Just a moment</title> class=\"cf-turnstile\""
                ),
                Some(ChallengeKind::CloudflareChallenge)
            );
        }
        assert_eq!(
            detect(Some(200), &headers("server", "cloudflare"), "Just a moment"),
            None
        );
        assert_eq!(detect(Some(403), &[], "Just a moment"), None);
        assert_eq!(
            detect(Some(503), &headers("server", "cloudflare"), "ordinary page"),
            None
        );
    }

    #[test]
    fn turnstile_iframe() {
        for html in [
            "<iframe src=\"https://challenges.cloudflare.com/turnstile/v0\"></iframe>",
            "<div class=\"cf-turnstile\"></div>",
        ] {
            assert_eq!(detect(None, &[], html), Some(ChallengeKind::Turnstile));
        }
        assert_eq!(
            detect(
                Some(200),
                &[],
                "class=\"g-recaptcha\" class=\"h-captcha\" class=\"cf-turnstile\""
            ),
            Some(ChallengeKind::Turnstile)
        );
    }

    #[test]
    fn hcaptcha_div() {
        for html in [
            "<script src=\"https://hcaptcha.com/1/api.js\"></script>",
            "<div class=\"h-captcha\"></div>",
        ] {
            assert_eq!(detect(None, &[], html), Some(ChallengeKind::HCaptcha));
        }
        assert_eq!(
            detect(None, &[], "class=\"g-recaptcha\" class=\"h-captcha\""),
            Some(ChallengeKind::HCaptcha)
        );
    }

    #[test]
    fn recaptcha_about_link_is_not_challenge() {
        assert_eq!(
            detect(
                None,
                &[],
                r#"<a href="https://www.google.com/recaptcha/about/">"#
            ),
            None
        );
    }

    #[test]
    fn recaptcha_net_script() {
        assert_eq!(
            detect(
                None,
                &[],
                r#"<script src="https://www.recaptcha.net/recaptcha/api.js"></script>"#
            ),
            Some(ChallengeKind::ReCaptcha)
        );
    }

    #[test]
    fn recaptcha_enterprise_script() {
        assert_eq!(
            detect(
                None,
                &[],
                r#"<script src="https://www.google.com/recaptcha/enterprise.js"></script>"#
            ),
            Some(ChallengeKind::ReCaptcha)
        );
    }

    #[test]
    fn recaptcha_script() {
        for html in [
            "<script src=\"https://google.com/recaptcha/api.js\"></script>",
            "<div class=\"g-recaptcha\"></div>",
        ] {
            assert_eq!(detect(None, &[], html), Some(ChallengeKind::ReCaptcha));
        }
    }

    #[test]
    fn plain_403_is_not_challenge() {
        assert_eq!(detect(Some(403), &[], "<h1>Forbidden</h1>"), None);
        assert_eq!(detect(Some(503), &[], "<h1>Unavailable</h1>"), None);
        assert_eq!(
            detect(
                Some(403),
                &headers("server", "cloudflare"),
                "<h1>Forbidden</h1>"
            ),
            None
        );
    }

    #[test]
    fn article_mentioning_captcha_is_not_challenge() {
        assert_eq!(
            detect(
                Some(200),
                &[],
                "<article>Cloudflare Turnstile, hCaptcha and reCAPTCHA are CAPTCHA services.</article>"
            ),
            None
        );
    }
}
