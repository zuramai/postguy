//! TLS for the testkit: a generated CA plus a leaf certificate for `127.0.0.1`/`localhost`
//! served by tokio-rustls. The CA PEM is exposed so the client under test can trust exactly
//! this authority (verification stays ON); the insecure path is tested by leaving it out.

use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// A generated server identity: leaf + issuing CA chain (DER), the private key (DER PKCS#8),
/// and the CA certificate in PEM for the client's trust store.
pub struct Identity {
    /// Leaf first, issuing CA second.
    pub chain_der: Vec<Vec<u8>>,
    pub key_der: Vec<u8>,
    pub ca_pem: String,
}

/// Generate a fresh CA and a leaf certificate it signs.
pub fn generate() -> Result<Identity, String> {
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).map_err(|err| err.to_string())?;
    ca_params.distinguished_name.push(rcgen::DnType::CommonName, "postguy-testkit CA");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(|err| err.to_string())?;
    let ca_certificate = ca_params.self_signed(&ca_key).map_err(|err| err.to_string())?;

    let mut leaf_params = CertificateParams::new(vec!["127.0.0.1".to_string(), "localhost".to_string()]).map_err(|err| err.to_string())?;
    leaf_params.distinguished_name.push(rcgen::DnType::CommonName, "postguy-testkit");
    let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(|err| err.to_string())?;
    let leaf_certificate = leaf_params.signed_by(&leaf_key, &ca_certificate, &ca_key).map_err(|err| err.to_string())?;

    Ok(Identity {
        chain_der: vec![leaf_certificate.der().to_vec(), ca_certificate.der().to_vec()],
        key_der: leaf_key.serialize_der(),
        ca_pem: ca_certificate.pem(),
    })
}

/// The rustls server config for one generated identity: TLS 1.2 minimum (matching the client
/// preference), ALPN `h2` and `http/1.1`.
pub fn server_config(identity: &Identity) -> Result<rustls::ServerConfig, String> {
    let certs: Vec<CertificateDer<'static>> = identity.chain_der.iter().map(|der| CertificateDer::from(der.clone())).collect();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.key_der.clone()));
    let mut config = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key).map_err(|err| err.to_string())?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_server_gets_a_leaf_signed_by_a_fresh_ca() {
        let first = generate().expect("generates");
        let second = generate().expect("generates");
        assert!(first.ca_pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert_eq!(first.chain_der.len(), 2, "leaf + issuing CA");
        assert_ne!(first.chain_der[0], second.chain_der[0], "fresh leaf per server");
        assert_ne!(first.chain_der[1], second.chain_der[1], "fresh CA per server");
        let config = server_config(&first).expect("rustls config");
        assert_eq!(config.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
    }
}
