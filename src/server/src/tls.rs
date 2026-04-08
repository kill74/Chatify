//! TLS configuration helpers.

use std::sync::Arc;

use clifford::error::{ChatifyError, ChatifyResult};
use tokio_rustls::TlsAcceptor;

pub fn load_tls_config(cert_path: &str, key_path: &str) -> ChatifyResult<TlsAcceptor> {
    let cert_file = std::fs::File::open(cert_path).map_err(|e| {
        ChatifyError::Validation(format!("cannot open TLS cert '{}': {}", cert_path, e))
    })?;
    let mut cert_reader = std::io::BufReader::new(cert_file);
    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ChatifyError::Validation(format!("failed to parse TLS cert: {}", e)))?;
    if certs.is_empty() {
        return Err(ChatifyError::Validation(
            "TLS cert file is empty".to_string(),
        ));
    }

    let key_file = std::fs::File::open(key_path).map_err(|e| {
        ChatifyError::Validation(format!("cannot open TLS key '{}': {}", key_path, e))
    })?;
    let mut key_reader = std::io::BufReader::new(key_file);
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|e| ChatifyError::Validation(format!("failed to parse TLS key: {}", e)))?
        .ok_or_else(|| ChatifyError::Validation("TLS key file is empty".to_string()))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| ChatifyError::Validation(format!("TLS config error: {}", e)))?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}
