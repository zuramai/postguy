//! TLS configuration for the client: OS roots plus optional user CAs, or the explicit
//! verification-off mode (which the UI must WARN about: turning it off is
//! visible in a chip and the console).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};

/// A PEM client certificate chain and private key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientIdentity {
    pub cert_path: String,
    pub key_path: String,
}

/// How server certificates are treated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsMode {
    /// Verify against the OS trust store plus the PEM bundle at `ca_pem_path`.
    Verify { ca_pem_path: Option<String> },
    /// Explicit off switch: NO verification. Encrypted but unauthenticated; the UI flags it.
    Insecure,
}

impl Default for TlsMode {
    fn default() -> Self {
        TlsMode::Verify { ca_pem_path: None }
    }
}

/// Build the rustls client config for a mode and ALPN preference.
pub fn client_config(mode: &TlsMode, alpn: &[&[u8]], identity: Option<&ClientIdentity>) -> Result<ClientConfig, String> {
    let builder = match mode {
        TlsMode::Verify { ca_pem_path } => {
            let mut roots = RootCertStore::empty();
            let native = rustls_native_certs::load_native_certs();
            for certificate in native.certs {
                // Skip individual roots that rustls refuses (a hardened platform store can
                // contain distinct-key or unsupported entries); the remaining roots still work.
                let _ = roots.add(certificate);
            }
            if let Some(path) = ca_pem_path {
                let pem = std::fs::read(path).map_err(|err| format!("reading CA bundle {path}: {err}"))?;
                let mut reader = std::io::BufReader::new(pem.as_slice());
                let mut added = 0usize;
                for certificate in rustls_pemfile::certs(&mut reader) {
                    let certificate = certificate.map_err(|err| format!("parsing CA bundle {path}: {err}"))?;
                    roots.add(certificate).map_err(|err| format!("CA bundle {path}: {err}"))?;
                    added += 1;
                }
                if added == 0 {
                    return Err(format!("CA bundle {path} contains no certificates"));
                }
            }
            let builder = ClientConfig::builder().with_root_certificates(roots);
            with_identity(builder, identity)?
        }
        TlsMode::Insecure => {
            let builder = ClientConfig::builder().dangerous().with_custom_certificate_verifier(Arc::new(NoVerification::new()?));
            with_identity(builder, identity)?
        }
    };
    let mut config = builder;
    config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    Ok(config)
}

fn with_identity(
    builder: rustls::ConfigBuilder<ClientConfig, rustls::client::WantsClientCert>,
    identity: Option<&ClientIdentity>,
) -> Result<ClientConfig, String> {
    let Some(identity) = identity else { return Ok(builder.with_no_client_auth()) };
    if identity.cert_path.trim().is_empty() || identity.key_path.trim().is_empty() {
        return Err("client certificate and key paths must both be set".into());
    }
    let cert_bytes = std::fs::read(&identity.cert_path).map_err(|err| format!("reading client certificate {}: {err}", identity.cert_path))?;
    let mut cert_reader = std::io::BufReader::new(cert_bytes.as_slice());
    let certs = rustls_pemfile::certs(&mut cert_reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|err| format!("parsing client certificate {}: {err}", identity.cert_path))?;
    if certs.is_empty() {
        return Err(format!("client certificate {} contains no certificates", identity.cert_path));
    }
    let key_bytes = std::fs::read(&identity.key_path).map_err(|err| format!("reading client key {}: {err}", identity.key_path))?;
    let mut key_reader = std::io::BufReader::new(key_bytes.as_slice());
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|err| format!("parsing client key {}: {err}", identity.key_path))?
        .ok_or_else(|| format!("client key {} contains no private key", identity.key_path))?;
    builder.with_client_auth_cert(certs, key).map_err(|err| format!("client certificate/key mismatch: {err}"))
}

/// Certificate verifier that accepts everything, ONLY for [`TlsMode::Insecure`].
#[derive(Debug)]
struct NoVerification {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl NoVerification {
    fn new() -> Result<NoVerification, String> {
        let provider = rustls::crypto::ring::default_provider();
        Ok(NoVerification { provider: Arc::new(provider) })
    }
}

impl ServerCertVerifier for NoVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_pem_identity_builds_a_client_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = rcgen::KeyPair::generate().expect("key");
        let mut params = rcgen::CertificateParams::new(vec!["client.example".into()]).expect("params");
        params.distinguished_name.push(rcgen::DnType::CommonName, "Postguy test client");
        let cert = params.self_signed(&key).expect("cert");
        let cert_path = dir.path().join("client.pem");
        let key_path = dir.path().join("client.key");
        std::fs::write(&cert_path, cert.pem()).expect("cert file");
        std::fs::write(&key_path, key.serialize_pem()).expect("key file");
        let identity = ClientIdentity { cert_path: cert_path.display().to_string(), key_path: key_path.display().to_string() };
        let config = client_config(&TlsMode::Verify { ca_pem_path: None }, &[b"http/1.1"], Some(&identity)).expect("config");
        assert_eq!(config.alpn_protocols, vec![b"http/1.1".to_vec()]);
    }

    #[test]
    fn missing_identity_paths_are_actionable() {
        let identity = ClientIdentity { cert_path: "/missing/client.pem".into(), key_path: "/missing/client.key".into() };
        let error = client_config(&TlsMode::default(), &[], Some(&identity)).expect_err("missing");
        assert!(error.contains("reading client certificate"), "{error}");
        let partial = ClientIdentity { cert_path: "/tmp/cert.pem".into(), key_path: String::new() };
        let error = client_config(&TlsMode::default(), &[], Some(&partial)).expect_err("partial");
        assert!(error.contains("both be set"), "{error}");
    }
}
