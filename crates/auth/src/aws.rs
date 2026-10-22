//! AWS Signature Version 4: Authorization header or presigned query.
//! Signs the host and `x-amz-*` headers; payloads are SHA-256 hashed. Timestamps are formatted
//! without pulling a calendar crate (UTC civil-date conversion).

use postguy_model::auth::AwsV4Auth;

use crate::error::{Result, invalid, missing};
use crate::util::{MacAlgorithm, UrlParts, hex, hmac, pct, sha256_hex};

/// What the caller must place on the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsV4Output {
    pub header: Option<String>,
    pub query: Vec<(String, String)>,
    /// Headers to add (x-amz-date, x-amz-security-token).
    pub headers: Vec<(String, String)>,
}

/// Sign a request at `timestamp` (unix seconds).
pub fn sign(auth: &AwsV4Auth, method: &str, url: &str, headers: &[(String, String)], body: &[u8], timestamp: u64) -> Result<AwsV4Output> {
    if auth.access_key.is_empty() {
        return Err(missing("aws4", "access key"));
    }
    if auth.secret_key.is_empty() {
        return Err(missing("aws4", "secret key"));
    }
    let parts = UrlParts::parse(url)?;
    let (region, service) = infer_scope(auth, &parts.host);
    if region.is_empty() || service.is_empty() {
        return Err(missing("aws4", "region/service"));
    }
    let amz_date = format_amz_date(timestamp)?;
    let short_date = amz_date.get(..8).ok_or_else(|| invalid("aws4", "timestamp formatting failed"))?.to_string();
    let mut signed_headers = vec![("host".to_string(), parts.host_header())];
    if auth.add_auth_data_to_query {
        // Presigned URLs send the session token in the query, not as a signed header.
    } else {
        signed_headers.push(("x-amz-date".to_string(), amz_date.clone()));
        for (name, value) in headers {
            let lower = name.to_ascii_lowercase();
            if matches!(lower.as_str(), "authorization" | "user-agent" | "content-length" | "host") || lower.starts_with("x-amz-date") {
                continue;
            }
            if !signed_headers.iter().any(|(existing, _)| existing == &lower) {
                signed_headers.push((lower, value.trim().to_string()));
            }
        }
        if !auth.session_token.is_empty() {
            signed_headers.push(("x-amz-security-token".into(), auth.session_token.clone()));
        }
    }
    signed_headers.sort_by(|left, right| left.0.cmp(&right.0));
    let signed_names = signed_headers.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(";");
    let canonical_headers = signed_headers.iter().map(|(name, value)| format!("{name}:{}\n", value.trim())).collect::<String>();
    let canonical_uri = canonical_path(&parts.path);
    let mut query = if auth.add_auth_data_to_query {
        let mut query = parts.query.clone();
        query.extend([
            ("X-Amz-Algorithm".into(), "AWS4-HMAC-SHA256".into()),
            ("X-Amz-Credential".into(), format!("{}/{short_date}/{region}/{service}/aws4_request", auth.access_key)),
            ("X-Amz-Date".into(), amz_date.clone()),
            ("X-Amz-Expires".into(), "900".into()),
            ("X-Amz-SignedHeaders".into(), signed_names.clone()),
        ]);
        if !auth.session_token.is_empty() {
            query.push(("X-Amz-Security-Token".into(), auth.session_token.clone()));
        }
        query
    } else {
        parts.query.clone()
    };
    query.sort_by(|left, right| pct(&left.0).cmp(&pct(&right.0)).then_with(|| pct(&left.1).cmp(&pct(&right.1))));
    let canonical_query = query.iter().map(|(key, value)| format!("{}={}", pct(key), pct(value))).collect::<Vec<_>>().join("&");
    let payload_hash = if auth.add_auth_data_to_query { "UNSIGNED-PAYLOAD".to_string() } else { sha256_hex(body) };
    let canonical_request =
        format!("{}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_names}\n{payload_hash}", method.to_ascii_uppercase());
    let scope = format!("{short_date}/{region}/{service}/aws4_request");
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", sha256_hex(canonical_request.as_bytes()));
    let signature = hex(&hmac(MacAlgorithm::Sha256, &signing_key(&auth.secret_key, &short_date, &region, &service)?, string_to_sign.as_bytes())?);
    if auth.add_auth_data_to_query {
        let mut query = query;
        query.push(("X-Amz-Signature".into(), signature));
        return Ok(AwsV4Output {
            header: None,
            query: query.into_iter().map(|(key, value)| (pct(&key), pct(&value))).collect(),
            headers: Vec::new(),
        });
    }
    let mut add_headers = vec![("x-amz-date".to_string(), amz_date)];
    if !auth.session_token.is_empty() {
        add_headers.push(("x-amz-security-token".to_string(), auth.session_token.clone()));
    }
    let header = format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_names}, Signature={signature}", auth.access_key);
    Ok(AwsV4Output { header: Some(header), query: Vec::new(), headers: add_headers })
}

fn infer_scope(auth: &AwsV4Auth, host: &str) -> (String, String) {
    let mut region = auth.region.clone();
    let mut service = auth.service.clone();
    if region.is_empty() || service.is_empty() {
        let labels: Vec<&str> = host.split('.').collect();
        if labels.len() >= 3 {
            if service.is_empty() {
                service = labels[0].to_string();
            }
            if region.is_empty() {
                region = labels[1].to_string();
            }
        }
    }
    (region, service)
}

fn canonical_path(path: &str) -> String {
    if path.is_empty() {
        return "/".into();
    }
    path.split('/').map(pct).collect::<Vec<_>>().join("/")
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Result<Vec<u8>> {
    let date_key = hmac(MacAlgorithm::Sha256, format!("AWS4{secret}").as_bytes(), date.as_bytes())?;
    let region_key = hmac(MacAlgorithm::Sha256, &date_key, region.as_bytes())?;
    let service_key = hmac(MacAlgorithm::Sha256, &region_key, service.as_bytes())?;
    hmac(MacAlgorithm::Sha256, &service_key, b"aws4_request")
}

/// `YYYYMMDDTHHMMSSZ` for a unix timestamp.
pub fn format_amz_date(timestamp: u64) -> Result<String> {
    let days = i64::try_from(timestamp / 86_400).map_err(|_| invalid("aws4", "timestamp is out of range"))?;
    let seconds = timestamp % 86_400;
    let (year, month, day) = civil_from_days(days);
    Ok(format!("{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z", seconds / 3600, (seconds / 60) % 60, seconds % 60))
}

/// Howard Hinnant's civil_from_days (days since 1970-01-01).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, u32::try_from(month).unwrap_or(1), u32::try_from(day).unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_get_vanilla_vector_matches() {
        let auth = AwsV4Auth {
            access_key: "AKIDEXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            region: "us-east-1".into(),
            service: "service".into(),
            ..AwsV4Auth::default()
        };
        let output = sign(&auth, "GET", "https://example.amazonaws.com/", &[], b"", 1_440_938_160).expect("sign");
        let header = output.header.expect("header");
        assert!(header.contains("Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"), "{header}");
        assert!(header.contains("SignedHeaders=host;x-amz-date"), "{header}");
    }

    #[test]
    fn dates_format_known_instants() {
        assert_eq!(format_amz_date(0).expect("epoch"), "19700101T000000Z");
        assert_eq!(format_amz_date(1_440_938_160).expect("aws"), "20150830T123600Z");
    }

    #[test]
    fn missing_credentials_are_actionable() {
        let error = sign(&AwsV4Auth::default(), "GET", "https://example.amazonaws.com/", &[], b"", 0).expect_err("missing");
        assert!(error.to_string().contains("access key"), "{error}");
    }
}
