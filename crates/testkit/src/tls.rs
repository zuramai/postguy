//! TLS for the testkit: a fresh self-signed certificate per server (rcgen), served by
//! tokio-rustls. The PEM is exposed so the client under test can trust exactly this CA
//! (verification is ON in tests; the insecure path is tested by leaving the CA out).

use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// A generated server identity: cert chain (DER) and private key (DER PKCS#8), plus the PEM
/// certificate for the client to trust.
pub struct Identity {
    pub certificate_der: Vec<u8>,
    pub key_der: Vec<u8>,
    pub ca_pem: String,
}

/// Generate a self-signed certificate valid for `127.0.0.1` and `localhost` (SANs), so a
/// client can connect to either spelling without name mismatch.
pub fn generate() -> Result<Identity, String> {
    let mut params = CertificateParams::new(vec!["127.0.0.1".to_string(), "localhost".to_string()]).map_err(|err| err.to_string())?;
    params.distinguished_name.push(rcgen::DnType::CommonName, "postguy-testkit");
    let key_pair = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(|err| err.to_string())?;
    let certificate = params.self_signed(&key_pair).map_err(|err| err.to_string())?;
    Ok(Identity { certificate_der: certificate.der().to_vec(), key_der: key_pair.serialize_der(), ca_pem: certificate.pem() })
}

/// The rustls server config for one generated identity: TLS 1.2 minimum (matching the client
/// preference), ALPN `h2` and `http/1.1`.
pub fn server_config(identity: &Identity) -> Result<rustls::ServerConfig, String> {
    let certs = vec![CertificateDer::from(identity.certificate_der.clone())];
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.key_der.clone()));
    let mut config = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key).map_err(|err| err.to_string())?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_server_gets_a_usingable_identity() {
        let first = generate().expect("generates");
        let second = generate().expect("generates");
        assert!(first.ca_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert_ne!(first.certificate_der, second.certificate_der, "fresh certificate per server");
        let config = server_config(&first).expect("rustls config");
        assert_eq!(config.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
    }
}
