use std::sync::{Arc, Mutex};

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
        if let Ok(mut guard) = self.local_offer.lock() {
            *guard = Some(offer);
        }
        self.create_answer().ok_or_else(|| "Failed to create answer".to_string())
    }

    pub async fn handle_answer(&mut self, answer: String) -> Result<(), String> {
        if let Ok(mut guard) = self.local_answer.lock() {
            *guard = Some(answer);
        }
        Ok(())
    }

    pub async fn add_remote_ice(&mut self, candidate: String, _sdp_mid: Option<String>) -> Result<(), String> {
        log::debug!("ICE: {}", &candidate[..candidate.len().min(50)]);
        Ok(())
    }
}

pub struct IceServer {
    pub urls: Vec<String>,
}

impl IceServer {
    pub fn google() -> Self {
        Self { urls: vec!["stun:stun.l.google.com:19302".into()] }
    }
    pub fn google_alt() -> Self {
        Self { urls: vec!["stun:stun1.l.google.com:19302".into()] }
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
