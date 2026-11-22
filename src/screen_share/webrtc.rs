//! WebRTC signalling for screen sharing.
//!
//! NOTE: this is currently a **signalling stub**. It exchanges SDP templates
//! and validates remote input, but does not perform real DTLS/SRTP key
//! negotiation or media transport. Media frames are relayed by the server's
//! `ss_frame` path instead. Do not rely on this module for confidentiality.

use std::sync::{Arc, Mutex};

/// Upper bound for remote SDP/ICE strings. Without a cap, a peer could push
/// arbitrarily large blobs into the stored offer/answer fields.
const MAX_SDP_LEN: usize = 64 * 1024;
const MAX_ICE_LEN: usize = 4 * 1024;

/// Validates that a remote SDP string is a minimally well-formed session
/// description before it is stored. This stub never negotiates real DTLS/SRTP
/// keys; validation prevents obviously malformed or oversized input.
fn validate_sdp(sdp: &str) -> Result<(), String> {
    if sdp.is_empty() || sdp.len() > MAX_SDP_LEN {
        return Err("SDP is empty or too large".to_string());
    }
    if !sdp.lines().any(|line| line.trim() == "v=0") {
        return Err("SDP is missing the v=0 version line".to_string());
    }
    if !sdp.lines().any(|line| line.starts_with("m=")) {
        return Err("SDP has no media sections".to_string());
    }
    Ok(())
}

pub struct WebRTCManager {
    local_offer: Arc<Mutex<Option<String>>>,
    local_answer: Arc<Mutex<Option<String>>>,
    #[allow(dead_code)]
    ice_servers: Vec<IceServer>,
}

impl Default for WebRTCManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WebRTCManager {
    pub fn new() -> Self {
        Self {
            local_offer: Arc::new(Mutex::new(None)),
            local_answer: Arc::new(Mutex::new(None)),
            ice_servers: vec![IceServer::google(), IceServer::google_alt()],
        }
    }

    pub fn get_local_offer(&self) -> Option<String> {
        self.local_offer.lock().ok().and_then(|o| o.clone())
    }

    pub fn create_offer(&self) -> Option<String> {
        let offer = SDP_TEMPLATE;
        if let Ok(mut guard) = self.local_offer.lock() {
            *guard = Some(offer.to_string());
        }
        Some(offer.to_string())
    }

    pub fn create_answer(&self) -> Option<String> {
        let answer = SDP_ANSWER_TEMPLATE;
        if let Ok(mut guard) = self.local_answer.lock() {
            *guard = Some(answer.to_string());
        }
        Some(answer.to_string())
    }

    pub async fn handle_offer(&mut self, offer: String) -> Result<String, String> {
        validate_sdp(&offer)?;
        let mut guard = self
            .local_offer
            .lock()
            .map_err(|_| "WebRTC offer lock poisoned".to_string())?;
        *guard = Some(offer);
        drop(guard);
        self.create_answer()
            .ok_or_else(|| "Failed to create answer".to_string())
    }

    pub async fn handle_answer(&mut self, answer: String) -> Result<(), String> {
        validate_sdp(&answer)?;
        let mut guard = self
            .local_answer
            .lock()
            .map_err(|_| "WebRTC answer lock poisoned".to_string())?;
        *guard = Some(answer);
        Ok(())
    }

    pub async fn add_remote_ice(
        &mut self,
        candidate: String,
        _sdp_mid: Option<String>,
    ) -> Result<(), String> {
        if candidate.is_empty() || candidate.len() > MAX_ICE_LEN {
            return Err("ICE candidate is empty or too large".to_string());
        }
        log::debug!("ICE: {}", &candidate[..candidate.len().min(50)]);
        Ok(())
    }
}

pub struct IceServer {
    pub urls: Vec<String>,
}

impl IceServer {
    pub fn google() -> Self {
        Self {
            urls: vec!["stun:stun.l.google.com:19302".into()],
        }
    }
    pub fn google_alt() -> Self {
        Self {
            urls: vec!["stun:stun1.l.google.com:19302".into()],
        }
    }
}

const SDP_TEMPLATE: &str = r#"v=0
o=- 0 0 IN IP4 127.0.0.1
s=Chatify
t=0 0
a=group:BUNDLE video audio
m=video 9 UDP/TLS/RTP/SAVPF 96
c=IN IP4 0.0.0.0
a=rtpmap:96 H264/90000
a=mid:video
a=recvonly
m=audio 9 UDP/TLS/RTP/SAVPF 111
c=IN IP4 0.0.0.0
a=rtpmap:111 Opus/48000/2
a=mid:audio
a=recvonly
"#;

const SDP_ANSWER_TEMPLATE: &str = r#"v=0
o=- 0 0 IN IP4 127.0.0.1
s=Chatify
t=0 0
a=group:BUNDLE video audio
m=video 9 UDP/TLS/RTP/SAVPF 96
c=IN IP4 0.0.0.0
a=rtpmap:96 H264/90000
a=mid:video
a=sendrecv
m=audio 9 UDP/TLS/RTP/SAVPF 111
c=IN IP4 0.0.0.0
a=rtpmap:111 Opus/48000/2
a=mid:audio
a=sendrecv
"#;
