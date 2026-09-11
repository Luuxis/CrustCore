use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("token does not have three dot-separated segments")]
    Malformed,
    #[error("payload is not valid base64url: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("payload is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn decode_claims(token: &str) -> Result<serde_json::Value, Error> {
    let mut segments = token.split('.');
    let (Some(_header), Some(payload), Some(_signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(Error::Malformed);
    };
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('='))?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn string_claim(token: &str, claim: &str) -> Option<String> {
    decode_claims(token)
        .ok()?
        .get(claim)?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(payload: &str) -> String {
        format!("h.{}.s", URL_SAFE_NO_PAD.encode(payload))
    }

    #[test]
    fn decodes_payload() {
        let token = build(r#"{"xuid":"2535436228373764","exp":1765466158}"#);
        let claims = decode_claims(&token).unwrap();
        assert_eq!(claims["exp"], 1765466158);
        assert_eq!(
            string_claim(&token, "xuid").as_deref(),
            Some("2535436228373764")
        );
    }

    #[test]
    fn rejects_malformed_token() {
        assert!(matches!(decode_claims("a.b"), Err(Error::Malformed)));
        assert!(matches!(decode_claims("a.b.c.d"), Err(Error::Malformed)));
    }

    #[test]
    fn missing_claim_is_none() {
        let token = build(r#"{"sub":"x"}"#);
        assert_eq!(string_claim(&token, "xuid"), None);
    }
}
