//! TLS configuration for the client: OS roots plus optional user CAs, or the explicit
//! verification-off mode (which the UI must WARN about — PROMPT.md 9: turning it off is
//! visible in a chip and the console).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};

/// How server certificates are treated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsMode {
    /// Verify against the OS trust store plus the PEM bundle at `ca_pem_path` (PROMPT.md 4.2:
    /// rustls-native-certs; user CA optional).
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
pub fn client_config(mode: &TlsMode, alpn: &[&[u8]]) -> Result<ClientConfig, String> {
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
            ClientConfig::builder().with_root_certificates(roots).with_no_client_auth()
        }
        TlsMode::Insecure => {
            let mut config =
                ClientConfig::builder().dangerous().with_custom_certificate_verifier(Arc::new(NoVerification::new()?)).with_no_client_auth();
            config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
            return Ok(config);
        }
    };
    let mut config = builder;
    config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    Ok(config)
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
