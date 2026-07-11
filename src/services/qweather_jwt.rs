use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ed25519_dalek::pkcs8::DecodePrivateKey;
use ed25519_dalek::{Signer, SigningKey};
use once_cell::sync::Lazy;

const QWEATHER_KID: &str = match option_env!("QWEATHER_KID") {
    Some(value) => value,
    None => "",
};
const QWEATHER_PROJECT_ID: &str = match option_env!("QWEATHER_PROJECT_ID") {
    Some(value) => value,
    None => "",
};

const QWEATHER_PRIVATE_KEY_PEM: &str =
    include_str!(concat!(env!("OUT_DIR"), "/qweather_private_key.pem"));

const JWT_TTL_SECS: u64 = 900;
const JWT_REFRESH_MARGIN_SECS: u64 = 60;

static TOKEN_CACHE: Lazy<Mutex<Option<(String, u64)>>> = Lazy::new(|| Mutex::new(None));

pub fn jwt_configured() -> bool {
    !QWEATHER_KID.is_empty()
        && !QWEATHER_PROJECT_ID.is_empty()
        && !QWEATHER_PRIVATE_KEY_PEM.trim().is_empty()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn base64url_encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

fn signing_key() -> Result<SigningKey, String> {
    SigningKey::from_pkcs8_pem(QWEATHER_PRIVATE_KEY_PEM.trim())
        .map_err(|err| format!("invalid Ed25519 private key: {err}"))
}

fn generate_jwt() -> Result<(String, u64), String> {
    let iat = now_secs().saturating_sub(30);
    let exp = iat + JWT_TTL_SECS;

    let header = format!(r#"{{"alg":"EdDSA","kid":"{QWEATHER_KID}"}}"#);
    let payload = format!(r#"{{"sub":"{QWEATHER_PROJECT_ID}","iat":{iat},"exp":{exp}}}"#);

    let signing_input = format!(
        "{}.{}",
        base64url_encode(header.as_bytes()),
        base64url_encode(payload.as_bytes())
    );

    let signature = signing_key()?.sign(signing_input.as_bytes());
    let token = format!(
        "{}.{}",
        signing_input,
        base64url_encode(signature.to_bytes().as_ref())
    );

    Ok((token, exp))
}

/// Returns a cached JWT, regenerating it shortly before expiry.
pub fn bearer_token() -> Result<String, String> {
    if !jwt_configured() {
        return Err("QWeather JWT credentials are not configured".to_string());
    }

    let now = now_secs();
    if let Ok(mut cache) = TOKEN_CACHE.lock() {
        if let Some((token, exp)) = cache.as_ref() {
            if now + JWT_REFRESH_MARGIN_SECS < *exp {
                return Ok(token.clone());
            }
        }

        let (token, exp) = generate_jwt()?;
        *cache = Some((token.clone(), exp));
        return Ok(token);
    }

    generate_jwt().map(|(token, _)| token)
}
