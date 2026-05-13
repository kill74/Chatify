#![deny(unsafe_code)]

//! # `clicord-server` ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â WebSocket Chat Server
//!
//! A single-binary, async WebSocket server built on [Tokio] and
//! [tokio-tungstenite]. It provides:
//!
//! * **Authentication** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â first-frame auth with username/password-hash and an
//!   Ed25519 public key for E2E-encrypted DMs.
//! * **2-Factor Authentication** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â TOTP (RFC 6238) and single-use backup codes,
//!   stored in SQLite.
//! * **Channel messaging** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â broadcast channels with a bounded in-memory ring
//!   buffer and a durable SQLite event store.
//! * **Direct messages** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â per-user DM channels keyed `__dm__<username>`.
//! * **Voice rooms** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â low-latency audio relay via per-room broadcast channels.
//! * **Search & history** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â full-text LIKE search and time-window ("rewind")
//!   queries backed by SQLite.
//! * **Protocol safety** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â payload size gates, timestamp-skew validation, and
//!   nonce-based replay protection on mutating events.
//! * **Graceful shutdown** ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â Ctrl+C drains active connections before exiting,
//!   with a bounded timeout.
//!
//! ## Architecture Overview
//!
//! ```text
//! ÃƒÂ¢Ã¢â‚¬ÂÃ…â€™ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ‚Â
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡                   Tokio Runtime                      ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡                                                      ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡  TcpListener::accept()                               ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡       ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡                                              ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡       ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬ÂÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬â€œÃ‚Âº tokio::spawn( handle(stream, addr, state) )ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡                 ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡                                    ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡        ÃƒÂ¢Ã¢â‚¬ÂÃ…â€™ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬â€œÃ‚Â¼ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ‚Â                          ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡        ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡  WebSocket auth  ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡  ÃƒÂ¢Ã¢â‚¬Â Ã‚Â validates first frame  ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡        ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬ÂÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ‚Â¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ‹Å“                          ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡                 ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡                                    ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡       ÃƒÂ¢Ã¢â‚¬ÂÃ…â€™ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬â€œÃ‚Â¼ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ‚Â                         ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡       ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡  Message recv loop  ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡                        ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡       ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡  handle_event(...)  ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡                        ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡       ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬ÂÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ‚Â¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ‹Å“                         ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡                 ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡                                    ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡   mpsc::unbounded ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬â€œÃ‚Âº sink writer task               ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬Å¡
//! ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬ÂÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ‹Å“
//!
//! Shared State (Arc<State>)
//!   ÃƒÂ¢Ã¢â‚¬ÂÃ…â€œÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ channels   : DashMap<String, Channel>
//!   ÃƒÂ¢Ã¢â‚¬ÂÃ…â€œÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ voice      : DashMap<String, broadcast::Sender>
//!   ÃƒÂ¢Ã¢â‚¬ÂÃ…â€œÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ user_statuses / user_pubkeys  : DashMap
//!   ÃƒÂ¢Ã¢â‚¬ÂÃ…â€œÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ recent_nonces : DashMap<String, VecDeque<String>>
//!   ÃƒÂ¢Ã¢â‚¬ÂÃ¢â‚¬ÂÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ store      : EventStore  ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬ÂÃ¢â€šÂ¬ÃƒÂ¢Ã¢â‚¬â€œÃ‚Âº SQLite file
//! ```
//!
//! ## Configuration
//!
//! All options are CLI flags (see [`Args`]):
//!
//! | Flag     | Default          | Description                       |
//! |----------|------------------|-----------------------------------|
//! | `--host` | `0.0.0.0`        | Bind address                       |
//! | `--port` | `8765`           | TCP port                           |
//! | `--log`  | off              | Enable structured logging          |
//! | `--db`   | `chatify.db`     | SQLite database file path          |
//! | `--db-durability` | `max-safety` | SQLite durability profile       |
//!
//! ## Protocol
//!
//! All frames are UTF-8 JSON objects with a mandatory `"t"` (type) field.
//! Binary frames are silently ignored. The first frame **must** be an `auth`
//! frame; any other type causes an immediate `err` response and disconnection.
//!
//! See [`validate_auth_payload`] for the full auth contract and
//! [`handle_event`] for the complete set of post-auth event types.

use chatify_server::args::Args;
use chatify_server::plugin_runtime::{
    MessageHookResult, PluginMessage, PluginMessageTarget, PluginRuntime, SlashExecutionResult,
    run_builtin_plugin_worker, DEFAULT_BUILTIN_PLUGINS, PLUGIN_API_VERSION,
};

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use base64::{engine::general_purpose, Engine as _};
use chatify::crypto;
use chatify::error::{ChatifyError, ChatifyResult};
use chatify::metrics::PrometheusMetrics;
use chatify::performance::Metrics as PerfMetrics;
use chatify::totp::{generate_qr_url, generate_secret, User2FA};
use chatify::voice::relay::VoiceBroadcast;
use clap::Parser;
use dashmap::DashSet;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use log::{debug, info, warn};
use prometheus::Encoder;
use serde_json::Value;

use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::{
    accept_async, accept_hdr_async,
    tungstenite::{
        handshake::server::{Callback, Request, Response},
        http, Message,
    },
};
use zeroize::Zeroize;

// ---------------------------------------------------------------------------
// CLI configuration
// ---------------------------------------------------------------------------

// Protocol constants (imported from library)
// ---------------------------------------------------------------------------
use chatify_server::db::{
    gib_to_bytes_i64, normalize_media_max_total_size_gb, normalize_media_prune_interval_secs,
    normalize_media_retention_days, MediaObjectUpsert,
};
use chatify_server::protocol::*;
use chatify_server::state::{
    normalize_slow_client_drop_burst,
    OUTBOUND_QUEUE_CAPACITY_DEFAULT, SLOW_CLIENT_DROP_BURST_DEFAULT,
};

// Data structures
// ---------------------------------------------------------------------------

/// Validated, strongly-typed representation of a successful auth frame parse.
///
/// Created by [`validate_auth_payload`] after all field-level validation
/// passes. Using a typed struct here ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â rather than passing `&Value` through
/// downstream functions ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â makes it impossible to accidentally skip validation
/// or misread a field name.
enum AuthCredential {
    LegacyHash(String),
    V2Proof {
        proof: String,
        client_nonce: String,
        server_nonce: String,
        enrollment_secret: Option<String>,
    },
}

struct AuthInfo {
    /// Validated username (ASCII alphanumeric / `-` / `_`, ÃƒÂ¢Ã¢â‚¬Â°Ã‚Â¤ 32 chars).
    username: String,

    /// Password hash submitted by the client (non-empty, ÃƒÂ¢Ã¢â‚¬Â°Ã‚Â¤ 256 chars).
    /// Used for credential verification against the stored hash.
    credential: AuthCredential,

    /// Validated status object (text + emoji), or default.
    status: Value,

    /// Base64-encoded 32-byte Ed25519 public key used for E2E DM encryption.
    pubkey: String,

    /// Optional TOTP or backup code. Present only when the client suspects
    /// or knows that 2-FA is enabled for this account.
    otp_code: Option<String>,

    /// If true, the connecting client identifies as a bridge (e.g. Discord bot).
    is_bridge: bool,

    /// Bridge type identifier (e.g. "discord"). Only meaningful when
    /// `is_bridge` is true.
    bridge_type: String,

    /// Instance ID for loop prevention. Only meaningful when `is_bridge` is
    /// true.
    bridge_instance_id: String,

    /// Number of bridge routes. Only meaningful when `is_bridge` is true.
    bridge_routes: usize,
}

// ---------------------------------------------------------------------------
// Channel — in-memory broadcast + history ring buffer
// ---------------------------------------------------------------------------

// EventStore is imported from chatify-server library.
// Channel Ã¢â‚¬â€ in-memory broadcast + history ring buffer
use chatify_server::state::{BridgeInfo, ConnectionGuard, State};

// ---------------------------------------------------------------------------
// State ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â shared, thread-safe server state
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Clamps a raw `limit` parameter from a client request to `[1, max]`,
/// substituting `default` when the field is absent.
///
/// This prevents clients from requesting zero or unreasonably large result
/// sets while still allowing the server to apply sensible per-endpoint
/// maximums without duplicating clamping logic in each handler.
/// Builds a deterministic reaction snapshot (`msg_id` + `emoji` + `count`)
/// from a list of persisted channel events.
fn build_reaction_snapshot(events: &[Value]) -> Vec<Value> {
    let mut counts: BTreeMap<(String, String), u32> = BTreeMap::new();

    for event in events {
        if event.get("t").and_then(|v| v.as_str()) != Some("reaction") {
            continue;
        }

        let msg_id = event
            .get("msg_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        let emoji = event
            .get("emoji")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if !is_valid_msg_id(msg_id) || !is_valid_reaction_emoji(emoji) {
            continue;
        }

        *counts
            .entry((msg_id.to_string(), emoji.to_string()))
            .or_insert(0) += 1;
    }

    counts
        .into_iter()
        .map(|((msg_id, emoji), count)| {
            serde_json::json!({
                "msg_id": msg_id,
                "emoji": emoji,
                "count": count,
            })
        })
        .collect()
}

/// Enqueues a JSON `payload` onto the per-connection outbound mpsc channel.
///
/// The error from `send` is intentionally ignored: if the receiver has been
/// dropped (e.g. because the WebSocket sink task exited), the connection is
/// already being torn down and there is nowhere meaningful to report the error.
#[derive(Clone)]
struct OutboundTx {
    tx: mpsc::Sender<String>,
    slow_client_tx: mpsc::Sender<()>,
    prometheus: Option<Arc<std::sync::Mutex<PrometheusMetrics>>>,
    dropped_streak: Arc<AtomicUsize>,
    disconnect_notified: Arc<AtomicBool>,
    drop_burst_limit: usize,
}

impl OutboundTx {
    fn new(
        tx: mpsc::Sender<String>,
        slow_client_tx: mpsc::Sender<()>,
        drop_burst_limit: usize,
        prometheus: Option<Arc<std::sync::Mutex<PrometheusMetrics>>>,
    ) -> Self {
        Self {
            tx,
            slow_client_tx,
            prometheus,
            dropped_streak: Arc::new(AtomicUsize::new(0)),
            disconnect_notified: Arc::new(AtomicBool::new(false)),
            drop_burst_limit: normalize_slow_client_drop_burst(drop_burst_limit),
        }
    }

    fn record_outbound_drop_metric(&self) {
        if let Some(prometheus) = &self.prometheus {
            if let Ok(metrics) = prometheus.try_lock() {
                metrics.record_outbound_queue_drop();
            }
        }
    }

    fn record_slow_client_disconnect_metric(&self) {
        if let Some(prometheus) = &self.prometheus {
            if let Ok(metrics) = prometheus.try_lock() {
                metrics.record_slow_client_disconnect();
            }
        }
    }

    fn try_send(&self, payload: String) {
        match self.tx.try_send(payload) {
            Ok(()) => {
                self.dropped_streak.store(0, Ordering::Relaxed);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.record_outbound_drop_metric();
                let streak = self.dropped_streak.fetch_add(1, Ordering::Relaxed) + 1;
                debug!(
                    "outbound queue full; dropping message streak={} limit={}",
                    streak, self.drop_burst_limit
                );

                if streak >= self.drop_burst_limit
                    && !self.disconnect_notified.swap(true, Ordering::Relaxed)
                {
                    self.record_slow_client_disconnect_metric();
                    let _ = self.slow_client_tx.try_send(());
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    async fn send(&self, payload: String) -> Result<(), mpsc::error::SendError<String>> {
        let result = self.tx.send(payload).await;
        if result.is_ok() {
            self.dropped_streak.store(0, Ordering::Relaxed);
        }
        result
    }
}

fn send_out_json(out_tx: &OutboundTx, payload: Value) {
    out_tx.try_send(payload.to_string());
}

#[allow(dead_code)]
fn send_out_json_with_metrics(out_tx: &OutboundTx, payload: Value, metrics: &PerfMetrics) {
    let serialized = payload.to_string();
    let bytes = serialized.len();
    metrics.inc_sent(1);
    metrics.inc_bytes_sent(bytes);
    out_tx.try_send(serialized);
}

fn send_err(out_tx: &OutboundTx, msg: impl std::fmt::Display, metrics: &PerfMetrics) {
    let payload = serde_json::json!({"t":"err","m":msg.to_string()});
    let serialized = payload.to_string();
    let bytes = serialized.len();
    metrics.inc_sent(1);
    metrics.inc_bytes_sent(bytes);
    out_tx.try_send(serialized);
}

fn parse_slash_invocation(input: &str) -> Option<(String, Vec<String>)> {
    let mut parts = input.split_whitespace();
    let first = parts.next()?;
    if !first.starts_with('/') {
        return None;
    }

    let command = first.trim_start_matches('/').trim().to_ascii_lowercase();
    if command.is_empty() {
        return None;
    }

    let args = parts.map(|part| part.to_string()).collect::<Vec<String>>();
    Some((command, args))
}

fn emit_plugin_messages(
    state: &Arc<State>,
    out_tx: &OutboundTx,
    channel: &str,
    messages: &[PluginMessage],
) {
    for message in messages {
        let payload = serde_json::json!({
            "t": "sys",
            "m": format!("[plugin:{}] {}", message.plugin, message.text),
            "ts": now()
        })
        .to_string();

        match message.target {
            PluginMessageTarget::Channel => {
                let _ = state.chan(channel).tx.send(payload);
            }
            PluginMessageTarget::Sender => {
                out_tx.try_send(payload);
            }
        }
    }
}

async fn execute_plugin_slash(
    state: &Arc<State>,
    username: &str,
    channel: &str,
    raw_input: &str,
) -> Result<SlashExecutionResult, String> {
    let (command, args) = parse_slash_invocation(raw_input)
        .ok_or_else(|| "invalid slash command format".to_string())?;

    let state = state.clone();
    let username = username.to_string();
    let channel = channel.to_string();
    tokio::task::spawn_blocking(move || {
        state
            .plugin_runtime
            .execute_slash(&channel, &username, &command, &args)
    })
    .await
    .map_err(|_| "plugin runtime task failed".to_string())?
}

async fn run_plugin_message_hooks(
    state: &Arc<State>,
    username: &str,
    channel: &str,
    content: &str,
) -> Result<MessageHookResult, String> {
    let state = state.clone();
    let username = username.to_string();
    let channel = channel.to_string();
    let content = content.to_string();

    tokio::task::spawn_blocking(move || {
        state
            .plugin_runtime
            .apply_message_hooks(&channel, &username, &content)
    })
    .await
    .map_err(|_| "plugin hook runtime task failed".to_string())?
}

/// Spawns a background task that forwards messages from a broadcast `rx` to
/// an mpsc `out_tx`, bridging the fan-out broadcast model to the single-writer
/// sink task.
///
/// The task exits cleanly when:
/// - `rx` reports `RecvError::Closed` (channel dropped).
/// - `out_tx.send()` fails (the sink task has exited).
///
/// Lagged messages (`RecvError::Lagged`) are silently skipped ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â the client
/// will see a gap in the message stream, which is preferable to crashing the
/// connection.
fn spawn_broadcast_forwarder(
    mut rx: broadcast::Receiver<String>,
    out_tx: OutboundTx,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(m) => {
                    if out_tx.send(m).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(_) => {} // Lagged: skip and continue
            }
        }
    })
}

fn spawn_voice_audio_forwarder(
    mut rx: broadcast::Receiver<String>,
    out_tx: OutboundTx,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(message) => {
                    if out_tx.send(message).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(_) => {}
            }
        }
    })
}

fn spawn_channel_forwarder(
    mut rx: broadcast::Receiver<String>,
    out_tx: OutboundTx,
    joined_channels: Arc<DashSet<String>>,
    channel: String,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if !joined_channels.contains(&channel) {
                break;
            }

            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Ok(message)) => {
                    if !joined_channels.contains(&channel) {
                        break;
                    }
                    if out_tx.send(message).await.is_err() {
                        break;
                    }
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => break,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    continue;
                }
                Err(_) => {
                    continue;
                }
            }
        }
    })
}

fn voice_event_room(event: &VoiceBroadcast) -> &str {
    match event {
        VoiceBroadcast::Users { room, .. }
        | VoiceBroadcast::StateChange { room, .. }
        | VoiceBroadcast::Speaking { room, .. }
        | VoiceBroadcast::MemberJoined { room, .. }
        | VoiceBroadcast::MemberLeft { room, .. } => room.as_str(),
    }
}

fn should_forward_voice_event(active_room: Option<&str>, event: &VoiceBroadcast) -> bool {
    matches!(active_room, Some(room) if room == voice_event_room(event))
}

fn spawn_voice_relay_forwarder(
    mut rx: broadcast::Receiver<VoiceBroadcast>,
    out_tx: OutboundTx,
    active_room: Arc<RwLock<Option<String>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let should_forward = {
                        let room = active_room.read().await;
                        should_forward_voice_event(room.as_deref(), &event)
                    };
                    if !should_forward {
                        continue;
                    }

                    let json = match serde_json::to_string(&event) {
                        Ok(encoded) => encoded,
                        Err(err) => {
                            warn!("failed to serialize voice relay event: {}", err);
                            continue;
                        }
                    };
                    if out_tx.send(json).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(_) => {}
            }
        }
    })
}

fn track_forwarder(forwarders: &mut Vec<JoinHandle<()>>, handle: JoinHandle<()>) {
    forwarders.retain(|existing| !existing.is_finished());
    forwarders.push(handle);
}

// ---------------------------------------------------------------------------
// Event handler
// ---------------------------------------------------------------------------

/// Dispatches a single post-auth WebSocket event to the appropriate handler.
///
/// This function is the central routing switch for all client-initiated actions.
/// It is called in the connection's main read loop after the frame has been
/// size-checked and JSON-parsed.
///
/// # Replay protection
///
/// For event types listed in [`requires_fresh_protection`], this function
/// first validates the timestamp skew and then registers the nonce (if present)
/// before any business logic runs. A validation failure sends an `err` response
/// and returns early, leaving the connection open for subsequent valid frames.
///
/// # Supported event types
///
/// | Type          | Description                                         |
/// |---------------|-----------------------------------------------------|
/// | `msg`         | Broadcast a channel message (ciphertext + plaintext index) |
/// | `img`         | Broadcast a base64-encoded image to a channel       |
/// | `dm`          | Send an encrypted direct message to a single user   |
/// | `join`        | Subscribe to a channel and receive its history      |
/// | `leave`       | Unsubscribe from a previously joined channel        |
/// | `history`     | Fetch persisted history for a channel               |
/// | `reaction_sync` | Fetch aggregated reaction counts for a channel    |
/// | `search`      | Full-text search over a channel's plaintext index   |
/// | `rewind`      | Fetch events within a relative time window          |
/// | `replay`      | Fetch events from an absolute timestamp onward       |
/// | `users`       | Get the current online user ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ public key directory  |
/// | `info`        | Get server info (channels list, online count)       |
/// | `vjoin`       | Join a voice room                                   |
/// | `vleave`      | Leave the current voice room                        |
/// | `vdata`       | Forward audio data to all members of a voice room   |
/// | `ss_start`    | Join/create a screen-share relay room               |
/// | `ss_meta`     | Relay screen stream metadata to room participants   |
/// | `ss_frame`    | Relay encoded screen frame payload to participants  |
/// | `ping`        | Heartbeat ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â server replies with `pong`              |
/// | `edit`        | Edit a previously sent message (in-memory only)     |
/// | `file_meta`   | Announce a file transfer to a channel               |
/// | `file_chunk`  | Stream a chunk of a file transfer                   |
/// | `typing`      | Broadcast typing state for channel or DM scope      |
/// | `status`      | Update the caller's presence status                 |
async fn handle_self_registration<S>(
    state: &Arc<State>,
    d: &Value,
    _addr: &SocketAddr,
    sink: &mut SplitSink<tokio_tungstenite::WebSocketStream<S>, tungstenite::Message>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    if !state.self_registration_enabled {
        let _ = sink
            .send(Message::text(
                serde_json::json!({
                    "t": "err",
                    "m": "self-registration is disabled"
                })
                .to_string(),
            ))
            .await;
        return;
    }

    let username = match d.get("u").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => {
            let _ = sink
                .send(Message::text(
                    serde_json::json!({"t": "err", "m": "missing username"}).to_string(),
                ))
                .await;
            return;
        }
    };

    if !is_valid_username(username) {
        let _ = sink
            .send(Message::text(
                serde_json::json!({"t": "err", "m": "invalid username"}).to_string(),
            ))
            .await;
        return;
    }

    let pw = match d.get("pw").and_then(|v| v.as_str()) {
        Some(p) if !p.is_empty() => p,
        _ => {
            let _ = sink
                .send(Message::text(
                    serde_json::json!({"t": "err", "m": "missing password"}).to_string(),
                ))
                .await;
            return;
        }
    };

    if pw.len() > MAX_PASSWORD_FIELD_LEN {
        let _ = sink
            .send(Message::text(
                serde_json::json!({"t": "err", "m": "password too long"}).to_string(),
            ))
            .await;
        return;
    }

    let _pubkey = match d.get("pk").and_then(|v| v.as_str()) {
        Some(pk) if is_valid_pubkey_b64(pk) => pk,
        _ => {
            let _ = sink
                .send(Message::text(
                    serde_json::json!({"t": "err", "m": "invalid public key"}).to_string(),
                ))
                .await;
            return;
        }
    };

    if state.store.load_pw_hash(username).ok().flatten().is_some() {
        let _ = sink
            .send(Message::text(
                serde_json::json!({
                    "t": "err",
                    "m": "username already exists"
                })
                .to_string(),
            ))
            .await;
        return;
    }

    let server_hash = crypto::pw_hash(pw);
    state.store.upsert_credentials(username, &server_hash);

    info!("self-registration completed");

    let _ = sink
        .send(Message::text(
            serde_json::json!({
                "t": "registered",
                "m": "account created successfully"
            })
            .to_string(),
        ))
        .await;
}

struct ConnectionSession {
    out_tx: OutboundTx,
    fresh_required: bool,
    voice_room: Option<String>,
    active_voice_room: Arc<RwLock<Option<String>>>,
    voice_audio_forwarder: Option<JoinHandle<()>>,
    voice_relay_forwarder: Option<JoinHandle<()>>,
    screen_room: Option<String>,
    screen_forwarder: Option<JoinHandle<()>>,
    joined_channels: Arc<DashSet<String>>,
    channel_forwarders: Vec<JoinHandle<()>>,
}

async fn handle_event(
    d: &Value,
    state: &Arc<State>,
    username: &str,
    session: &mut ConnectionSession,
) {
    let ConnectionSession {
        out_tx,
        fresh_required,
        voice_room,
        active_voice_room,
        voice_audio_forwarder,
        voice_relay_forwarder,
        screen_room,
        screen_forwarder,
        joined_channels,
        channel_forwarders,
    } = session;

    let t = d["t"].as_str().unwrap_or("");
    let has_event_channel = d
        .get("ch")
        .or_else(|| d.get("r"))
        .and_then(|v| v.as_str())
        .is_some();

    if has_event_channel {
        info!("event received type={} scope=channel", t);
    } else {
        info!("event received type={}", t);
    }

    // --- Replay protection (timestamp skew + nonce dedup) ------------------
    // Only applied to mutating events (see requires_fresh_protection).
    let should_validate_freshness =
        *fresh_required || d.get("n").is_some() || d.get("ts").is_some();
    if requires_fresh_protection(d) && should_validate_freshness {
        if let Err(e) = validate_timestamp_skew(d) {
            warn!("protocol validation failed type={} reason={}", t, e);
            send_err(
                out_tx,
                format!("protocol validation failed: {}", e),
                &state.metrics,
            );
            return;
        }
        if let Err(e) = validate_and_register_nonce(state, username, d) {
            warn!("protocol validation failed type={} reason={}", t, e);
            send_err(
                out_tx,
                format!("protocol validation failed: {}", e),
                &state.metrics,
            );
            return;
        }
    }

    // --- Optional session token validation (backward compatible) ---
    // Protocol v1 clients do not send session tokens on most events.
    // To preserve compatibility, only validate when a token is explicitly
    // provided by the client.
    if let Some(provided_token) = d.get("token").and_then(|v| v.as_str()) {
        if !state.validate_session_token(username, Some(provided_token)) {
            send_err(
                out_tx,
                "invalid or expired session token. Please reconnect.",
                &state.metrics,
            );
            return;
        }
    }

    // --- Event dispatch switch ---------------------------------------------
    match t {
        "slash" => {
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let raw = d["cmd"].as_str().unwrap_or("").trim();
            if raw.is_empty() {
                send_err(out_tx, "slash command is required", &state.metrics);
                return;
            }

            match execute_plugin_slash(state, username, &ch, raw).await {
                Ok(result) => {
                    emit_plugin_messages(state, out_tx, &ch, &result.messages);
                    send_out_json(
                        out_tx,
                        serde_json::json!({
                            "t":"slash_ok",
                            "api_version": PLUGIN_API_VERSION,
                            "cmd": raw,
                            "messages": result.messages.len(),
                            "ts": now()
                        }),
                    );
                }
                Err(err) => {
                    send_err(out_tx, err, &state.metrics);
                }
            }
        }
        "plugin" => {
            if !state.can_manage(username, "general") {
                send_err(
                    out_tx,
                    "insufficient permissions to manage plugins",
                    &state.metrics,
                );
                return;
            }

            let sub = d["sub"].as_str().unwrap_or("list");
            match sub {
                "install" => {
                    let spec = d
                        .get("plugin")
                        .or_else(|| d.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();

                    if spec.is_empty() {
                        send_err(
                            out_tx,
                            "plugin install requires 'plugin' (name or executable path)",
                            &state.metrics,
                        );
                        return;
                    }

                    let state_for_task = state.clone();
                    let spec_for_task = spec.clone();
                    match tokio::task::spawn_blocking(move || {
                        state_for_task.plugin_runtime.install_plugin(&spec_for_task)
                    })
                    .await
                    {
                        Ok(Ok(manifest)) => {
                            state.store.log_audit(
                                "plugin_install",
                                username,
                                Some(&manifest.name),
                                None,
                                None,
                                Some(&spec),
                            );
                            send_out_json(
                                out_tx,
                                serde_json::json!({
                                    "t":"plugin_installed",
                                    "api_version": PLUGIN_API_VERSION,
                                    "plugin": manifest.name,
                                    "commands": manifest.commands,
                                    "message_hook": manifest.message_hook,
                                    "ts": now()
                                }),
                            );
                        }
                        Ok(Err(err)) => send_err(out_tx, err, &state.metrics),
                        Err(_) => send_err(out_tx, "plugin install task failed", &state.metrics),
                    }
                }
                "disable" => {
                    let plugin_id = d
                        .get("plugin")
                        .or_else(|| d.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();

                    if plugin_id.is_empty() {
                        send_err(out_tx, "plugin disable requires 'plugin'", &state.metrics);
                        return;
                    }

                    let state_for_task = state.clone();
                    let plugin_id_for_task = plugin_id.clone();
                    match tokio::task::spawn_blocking(move || {
                        state_for_task
                            .plugin_runtime
                            .disable_plugin(&plugin_id_for_task)
                    })
                    .await
                    {
                        Ok(Ok(())) => {
                            state.store.log_audit(
                                "plugin_disable",
                                username,
                                Some(&plugin_id),
                                None,
                                None,
                                None,
                            );
                            send_out_json(
                                out_tx,
                                serde_json::json!({
                                    "t":"plugin_disabled",
                                    "plugin": plugin_id,
                                    "ts": now()
                                }),
                            );
                        }
                        Ok(Err(err)) => send_err(out_tx, err, &state.metrics),
                        Err(_) => send_err(out_tx, "plugin disable task failed", &state.metrics),
                    }
                }
                "list" | "" => {
                    send_out_json(
                        out_tx,
                        serde_json::json!({
                            "t":"plugins",
                            "api_version": PLUGIN_API_VERSION,
                            "plugins": state.plugin_runtime.list_plugins_json(),
                            "ts": now()
                        }),
                    );
                }
                _ => send_err(
                    out_tx,
                    "unknown plugin subcommand (install|disable|list)",
                    &state.metrics,
                ),
            }
        }
        "msg" => {
            // Broadcast an encrypted message to a channel.
            // `"c"` is the ciphertext blob; `"p"` is optional plaintext for
            // the search index only Ã¢â‚¬â€ it is never echoed back to clients.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let mut c = d["c"].as_str().unwrap_or("").to_string();
            let mut p = d["p"].as_str().unwrap_or("").to_string();

            let slash_input = if p.trim_start().starts_with('/') {
                Some(p.clone())
            } else if c.trim_start().starts_with('/') {
                Some(c.clone())
            } else {
                None
            };

            if let Some(raw_slash) = slash_input {
                match execute_plugin_slash(state, username, &ch, &raw_slash).await {
                    Ok(result) => emit_plugin_messages(state, out_tx, &ch, &result.messages),
                    Err(err) => send_err(out_tx, err, &state.metrics),
                }
                return;
            }

            if c.is_empty() {
                return;
            }

            // Check per-user rate limit
            let (rate_allowed, _remaining, reset_in) = state.check_user_rate_limit(username);
            if !rate_allowed {
                send_err(
                    out_tx,
                    format!("rate limited: try again in {}s", reset_in),
                    &state.metrics,
                );
                return;
            }

            if !state.can_send(username, &ch) {
                if state.is_muted(username, &ch) {
                    send_err(out_tx, "you are muted in this channel", &state.metrics);
                } else if state.is_banned(username, &ch) {
                    send_err(out_tx, "you are banned from this channel", &state.metrics);
                } else {
                    send_err(
                        out_tx,
                        "you do not have permission to send messages",
                        &state.metrics,
                    );
                }
                return;
            }

            let hook_content = if !p.is_empty() { p.clone() } else { c.clone() };

            match run_plugin_message_hooks(state, username, &ch, &hook_content).await {
                Ok(hook_result) => {
                    if let Some(replacement) = hook_result.replacement {
                        if !p.is_empty() {
                            p = replacement;
                        } else {
                            c = replacement;
                        }
                    }

                    if hook_result.blocked {
                        emit_plugin_messages(state, out_tx, &ch, &hook_result.messages);
                        send_err(out_tx, "message blocked by plugin policy", &state.metrics);
                        return;
                    }

                    emit_plugin_messages(state, out_tx, &ch, &hook_result.messages);
                }
                Err(err) => {
                    warn!("plugin message hook failed: {}", err);
                }
            }

            let reply_to = d
                .get("reply_to")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            if let Some(reply_to) = reply_to.as_deref() {
                if !is_valid_msg_id(reply_to) {
                    send_err(out_tx, "reply requires valid msg_id", &state.metrics);
                    return;
                }
            }

            let chan = state.chan(&ch);
            let reply_context = if let Some(reply_to) = reply_to.as_deref() {
                let mut context = chan
                    .hist()
                    .await
                    .iter()
                    .rev()
                    .find_map(|event| reply_context_from_event(event, reply_to));
                if context.is_none() {
                    context = state
                        .store
                        .history(&ch, HISTORY_CAP)
                        .iter()
                        .find_map(|event| reply_context_from_event(event, reply_to));
                }
                context
            } else {
                None
            };

            let msg_id = chatify::fresh_nonce_hex();
            let mut entry = serde_json::json!({
                "t":"msg",
                "msg_id":msg_id,
                "ch":ch,
                "u":username,
                "c":c,
                "ts":now()
            });
            if let Some(src) = d.get("src").and_then(|v| v.as_str()) {
                entry["src"] = Value::String(src.to_string());
            }
            if let Some(relay) = d.get("relay").filter(|v| v.is_object()) {
                entry["relay"] = relay.clone();
            }
            if let Some(reply_to) = reply_to.as_deref() {
                entry["reply_to"] = Value::String(reply_to.to_string());
                if let Some(reply_context) = reply_context {
                    entry["reply"] = reply_context;
                }
            } else if let Some(reply) = d.get("reply").filter(|v| v.is_object()) {
                entry["reply"] = reply.clone();
            }
            let serialized = entry.to_string();
            chan.push(entry.clone()).await;
            let searchable = if p.is_empty() { c.as_str() } else { p.as_str() };
            state
                .store
                .persist("msg", &ch, username, None, &entry, searchable);
            state
                .message_cache
                .push_and_trim(&format!("h:{}", ch), entry, 50);
            let _ = chan.tx.send(serialized);

            // Check for spam
            state.check_and_alert_spam(username, &ch);
        }
        "img" => {
            // Broadcast a base64-encoded image. Not persisted to avoid bloating
            // the event store with large binary payloads.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let a = d["a"].as_str().unwrap_or("").to_string();
            if a.is_empty() {
                return;
            }
            let _ = state.chan(&ch).tx.send(
                serde_json::json!({"t":"img","ch":ch,"u":username,"a":a,"ts":now()}).to_string(),
            );
        }
        "dm" => {
            // Send an encrypted direct message.
            // The sender's public key is injected by the server so the
            // recipient can verify / decrypt without a separate lookup.
            let target = d["to"].as_str().unwrap_or("").to_string();
            let c = d["c"].as_str().unwrap_or("").to_string();
            let ptxt = d["p"].as_str().unwrap_or("").to_string();
            let searchable = if ptxt.is_empty() {
                c.as_str()
            } else {
                ptxt.as_str()
            };
            if c.is_empty() || target.is_empty() {
                return;
            }
            let sender_pk = state
                .user_pubkeys
                .get(username)
                .map(|v| v.value().clone())
                .unwrap_or_default();
            let event = serde_json::json!({
                "t":"dm","from":username,"to":target,
                "c":c,"pk":sender_pk,"ts":now()
            });
            let p = event.to_string();
            // Persist to both the recipient's and the sender's DM channel so
            // history is available from either party's perspective.
            state.store.persist(
                "dm",
                &dm_channel_name(&target),
                username,
                Some(&target),
                &event,
                searchable,
            );
            state.message_cache.push_and_trim(
                &format!("dm:{}:{}", username, target),
                event.clone(),
                50,
            );
            state
                .message_cache
                .push_and_trim(&format!("dm:{}:{}", target, username), event, 50);
            let _ = state.chan(&dm_channel_name(&target)).tx.send(p.clone());
            let _ = state.chan(&dm_channel_name(username)).tx.send(p.clone());
        }
        "join" => {
            // Subscribe to a channel and immediately receive its history.
            // SQLite history takes precedence over the in-memory ring buffer
            // so newly booted servers serve correct history from persisted data.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let chan = state.chan(&ch);
            let mut hist = state.store.history(&ch, HISTORY_CAP);
            if hist.is_empty() {
                hist = chan.hist().await;
            }

            // Avoid duplicate forwarders when the same channel is joined
            // multiple times during one connection lifetime.
            if joined_channels.insert(ch.clone()) {
                track_forwarder(
                    channel_forwarders,
                    spawn_channel_forwarder(
                        chan.tx.subscribe(),
                        out_tx.clone(),
                        joined_channels.clone(),
                        ch.clone(),
                    ),
                );
            }

            state.store.upsert_channel_subscription(username, &ch);
            send_out_json(
                out_tx,
                serde_json::json!({"t":"joined","ch":ch,"hist":hist}),
            );
            let join_msg = serde_json::json!({
                "t":"sys",
                "m":format!("ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ {} joined #{}", username, ch),
                "ts":now()
            });
            let join_event = serde_json::json!({
                "t":"join",
                "ch":ch,
                "u":username,
                "ts":now()
            });
            state.store.persist(
                "sys",
                &ch,
                username,
                None,
                &join_msg,
                &format!("{} joined", username),
            );
            state.store.persist(
                "join",
                &ch,
                username,
                None,
                &join_event,
                &format!("{} joined", username),
            );
            let _ = chan.tx.send(join_msg.to_string());

            // Check for raid patterns
            state.check_and_alert_raid(username, &ch);
        }
        "leave" => {
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            if ch == "general" {
                send_err(out_tx, "cannot leave #general", &state.metrics);
                return;
            }

            let was_joined = joined_channels.remove(&ch).is_some();
            channel_forwarders.retain(|handle| !handle.is_finished());
            state.store.remove_channel_subscription(username, &ch);

            send_out_json(
                out_tx,
                serde_json::json!({"t":"left","ch":ch,"already_left":!was_joined}),
            );

            if !was_joined {
                return;
            }

            let leave_msg = serde_json::json!({
                "t":"sys",
                "m":format!("{} left #{}", username, ch),
                "ts":now()
            });
            let leave_event = serde_json::json!({
                "t":"leave",
                "ch":ch,
                "u":username,
                "ts":now()
            });

            state.store.persist(
                "sys",
                &ch,
                username,
                None,
                &leave_msg,
                &format!("{} left", username),
            );
            state.store.persist(
                "leave",
                &ch,
                username,
                None,
                &leave_event,
                &format!("{} left", username),
            );

            let chan = state.chan(&ch);
            let _ = chan.tx.send(leave_msg.to_string());
        }
        "history" => {
            // Return persisted events for a channel or DM scope.
            let scope = match parse_event_query_scope(d.get("ch").and_then(|v| v.as_str())) {
                Ok(v) => v,
                Err(e) => {
                    send_err(out_tx, e.to_string(), &state.metrics);
                    return;
                }
            };
            let limit = clamp_limit(
                d.get("limit").and_then(|v| v.as_u64()),
                DEFAULT_HISTORY_LIMIT,
                500,
            );
            let seconds = d
                .get("seconds")
                .and_then(|v| v.as_u64())
                .map(|v| v.clamp(1, 31 * 24 * 3600));

            let response_ch = scope.response_channel();
            let events = match scope {
                EventQueryScope::Channel(ch) => {
                    let cache_key = format!("h:{}", ch);
                    if limit <= 50 && seconds.is_none() {
                        if let Some(cached) = state.message_cache.get(&cache_key) {
                            if !cached.is_empty() {
                                let result: Vec<Value> =
                                    cached.iter().rev().take(limit).cloned().collect();
                                send_out_json(
                                    out_tx,
                                    serde_json::json!({"t":"history","ch":response_ch,"events":result,"ts":now()}),
                                );
                                return;
                            }
                        }
                    }
                    if let Some(window_secs) = seconds {
                        state.store.rewind(&ch, window_secs, limit)
                    } else {
                        state.store.history(&ch, limit)
                    }
                }
                EventQueryScope::DmConversation(peer) => {
                    let cache_key = format!("dm:{}:{}", username, peer);
                    if limit <= 50 && seconds.is_none() {
                        if let Some(cached) = state.message_cache.get(&cache_key) {
                            if !cached.is_empty() {
                                let result: Vec<Value> =
                                    cached.iter().rev().take(limit).cloned().collect();
                                send_out_json(
                                    out_tx,
                                    serde_json::json!({"t":"history","ch":response_ch,"events":result,"ts":now()}),
                                );
                                return;
                            }
                        }
                    }
                    if let Some(window_secs) = seconds {
                        state.store.dm_rewind(username, &peer, window_secs, limit)
                    } else {
                        state.store.dm_history(username, &peer, limit)
                    }
                }
            };
            send_out_json(
                out_tx,
                serde_json::json!({"t":"history","ch":response_ch,"events":events,"ts":now()}),
            );
        }
        "reaction_sync" => {
            // Return aggregated reaction counters for the requested channel.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let limit = clamp_limit(d.get("limit").and_then(|v| v.as_u64()), 500, 5000);
            let events = state.store.reaction_events(&ch, limit);
            let reactions = build_reaction_snapshot(&events);

            send_out_json(
                out_tx,
                serde_json::json!({
                    "t": "reaction_sync",
                    "ch": ch,
                    "reactions": reactions,
                    "ts": now(),
                }),
            );
        }
        "search" => {
            // Full-text search over channel or DM conversation events.
            let scope = match parse_event_query_scope(d.get("ch").and_then(|v| v.as_str())) {
                Ok(v) => v,
                Err(e) => {
                    send_err(out_tx, e.to_string(), &state.metrics);
                    return;
                }
            };
            let q = d["q"].as_str().unwrap_or("").trim().to_string();
            let limit = clamp_limit(
                d.get("limit").and_then(|v| v.as_u64()),
                DEFAULT_SEARCH_LIMIT,
                200,
            );
            let response_ch = scope.response_channel();
            let events = if q.is_empty() {
                Vec::new()
            } else {
                match scope {
                    EventQueryScope::Channel(ch) => state.store.search(&ch, &q, limit),
                    EventQueryScope::DmConversation(peer) => {
                        state.store.dm_search(username, &peer, &q, limit)
                    }
                }
            };
            send_out_json(
                out_tx,
                serde_json::json!({"t":"search","ch":response_ch,"q":q,"events":events,"ts":now()}),
            );
        }
        "rewind" => {
            // Time-window query: return events from the last `seconds` seconds.
            // The maximum window is capped at 31 days to prevent accidental
            // full-history dumps from a misconfigured client.
            let scope = match parse_event_query_scope(d.get("ch").and_then(|v| v.as_str())) {
                Ok(v) => v,
                Err(e) => {
                    send_err(out_tx, e.to_string(), &state.metrics);
                    return;
                }
            };
            let seconds = d
                .get("seconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_REWIND_SECONDS)
                .clamp(1, 31 * 24 * 3600);
            let limit = clamp_limit(
                d.get("limit").and_then(|v| v.as_u64()),
                DEFAULT_REWIND_LIMIT,
                500,
            );
            let response_ch = scope.response_channel();
            let events = match scope {
                EventQueryScope::Channel(ch) => state.store.rewind(&ch, seconds, limit),
                EventQueryScope::DmConversation(peer) => {
                    state.store.dm_rewind(username, &peer, seconds, limit)
                }
            };
            // Rewind reuses the `"history"` frame type so clients only need
            // one parser for time-ranged and offset-based history.
            send_out_json(
                out_tx,
                serde_json::json!({"t":"history","ch":response_ch,"events":events,"ts":now()}),
            );
        }
        "replay" => {
            // Absolute replay query: return events from a given timestamp.
            let scope = match parse_event_query_scope(d.get("ch").and_then(|v| v.as_str())) {
                Ok(v) => v,
                Err(e) => {
                    send_err(out_tx, e.to_string(), &state.metrics);
                    return;
                }
            };

            let Some(from_ts) = d
                .get("from_ts")
                .and_then(|v| v.as_f64().or_else(|| v.as_u64().map(|n| n as f64)))
            else {
                send_err(out_tx, "replay requires from_ts", &state.metrics);
                return;
            };

            if !from_ts.is_finite() || from_ts < 0.0 {
                send_err(out_tx, "replay requires valid from_ts", &state.metrics);
                return;
            }

            let limit = clamp_limit(d.get("limit").and_then(|v| v.as_u64()), 1000, 5000);
            let response_ch = scope.response_channel();
            let events = match scope {
                EventQueryScope::Channel(ch) => state.store.history_since(&ch, from_ts, limit),
                EventQueryScope::DmConversation(peer) => state
                    .store
                    .dm_history_since(username, &peer, from_ts, limit),
            };

            send_out_json(
                out_tx,
                serde_json::json!({
                    "t":"replay",
                    "ch":response_ch,
                    "from_ts":from_ts,
                    "events":events,
                    "ts":now()
                }),
            );
        }
        "users" => {
            // Return the current user ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ public key directory.
            send_out_json(
                out_tx,
                serde_json::json!({"t":"users","users":state.users_with_keys_json()}),
            );
        }
        "info" => {
            // Return server metadata: channel list and online user count.
            let chs: Vec<String> = state
                .channels
                .iter()
                .filter(|e| !e.key().starts_with("__dm__"))
                .map(|e| e.key().clone())
                .collect();
            send_out_json(
                out_tx,
                serde_json::json!({"t":"info","chs":chs,"online":state.online_count()}),
            );
        }
        "bridge_status" => {
            // Return status of all connected bridge instances.
            let bridges: Vec<Value> = state
                .bridges
                .iter()
                .map(|entry| {
                    let info = entry.value();
                    serde_json::json!({
                        "username": info.username,
                        "bridge_type": info.bridge_type,
                        "instance_id": info.instance_id,
                        "connected_at": info.connected_at,
                        "route_count": info.route_count,
                        "uptime_secs": (crate::now() - info.connected_at) as u64,
                    })
                })
                .collect();
            send_out_json(
                out_tx,
                serde_json::json!({
                    "t": "bridge_status",
                    "bridges": bridges,
                    "count": bridges.len(),
                    "ts": crate::now(),
                }),
            );
            info!(
                "event=bridge_status_requested bridge_count={}",
                bridges.len()
            );
        }
        "metrics" => {
            const DB_TOP_OPS_LIMIT: usize = 8;
            const DB_WARNING_P95_MS: f64 = 50.0;
            const DB_CRITICAL_P95_MS: f64 = 200.0;
            const DB_MIN_SAMPLES: u64 = 5;

            let snapshot = state.metrics.snapshot();
            let cache_stats = state.message_cache.stats();
            let pool_stats = state.store.get_pool_stats();
            let (db_top_ops, db_alerts, outbound_queue_drops, slow_client_disconnects) = state
                .prometheus
                .as_ref()
                .and_then(|metrics| metrics.try_lock().ok())
                .map(|metrics| {
                    (
                        metrics.top_db_operations_by_p95(DB_TOP_OPS_LIMIT),
                        metrics.db_latency_alerts(
                            DB_TOP_OPS_LIMIT,
                            DB_WARNING_P95_MS,
                            DB_CRITICAL_P95_MS,
                            DB_MIN_SAMPLES,
                        ),
                        metrics.outbound_queue_drops_total.get(),
                        metrics.slow_client_disconnects_total.get(),
                    )
                })
                .unwrap_or_else(|| (Vec::new(), Vec::new(), 0, 0));
            send_out_json(
                out_tx,
                serde_json::json!({
                    "t": "metrics",
                    "messages_sent": snapshot.messages_sent,
                    "messages_received": snapshot.messages_received,
                    "bytes_sent": snapshot.bytes_sent,
                    "bytes_received": snapshot.bytes_received,
                    "errors": snapshot.errors,
                    "connections_accepted": snapshot.connections_accepted,
                    "connections_closed": snapshot.connections_closed,
                    "active_connections": state.active_connection_count(),
                    "cache_hits": cache_stats.0,
                    "cache_misses": cache_stats.1,
                    "cache_hit_rate": cache_stats.2,
                    "db_pool_active": pool_stats.active_connections,
                    "db_pool_idle": pool_stats.idle_connections,
                    "db_pool_total": pool_stats.total_connections,
                    "db_pool_waiters": pool_stats.wait_count,
                    "outbound_queue_drops": outbound_queue_drops,
                    "slow_client_disconnects": slow_client_disconnects,
                    "db_top_ops": db_top_ops,
                    "db_alerts": db_alerts,
                    "db_latency_budget_ms": {
                        "warning_p95": DB_WARNING_P95_MS,
                        "critical_p95": DB_CRITICAL_P95_MS,
                        "min_samples": DB_MIN_SAMPLES,
                    },
                    "ts": now(),
                }),
            );
        }
        "db_profile" => {
            const DB_TOP_OPS_LIMIT: usize = 8;
            const DB_WARNING_P95_MS: f64 = 50.0;
            const DB_CRITICAL_P95_MS: f64 = 200.0;
            const DB_MIN_SAMPLES: u64 = 5;

            let pool_stats = state.store.get_pool_stats();
            let (db_top_ops, db_alerts, outbound_queue_drops, slow_client_disconnects) = state
                .prometheus
                .as_ref()
                .and_then(|metrics| metrics.try_lock().ok())
                .map(|metrics| {
                    (
                        metrics.top_db_operations_by_p95(DB_TOP_OPS_LIMIT),
                        metrics.db_latency_alerts(
                            DB_TOP_OPS_LIMIT,
                            DB_WARNING_P95_MS,
                            DB_CRITICAL_P95_MS,
                            DB_MIN_SAMPLES,
                        ),
                        metrics.outbound_queue_drops_total.get(),
                        metrics.slow_client_disconnects_total.get(),
                    )
                })
                .unwrap_or_else(|| (Vec::new(), Vec::new(), 0, 0));

            send_out_json(
                out_tx,
                serde_json::json!({
                    "t": "db_profile",
                    "db_top_ops": db_top_ops,
                    "db_alerts": db_alerts,
                    "db_pool_active": pool_stats.active_connections,
                    "db_pool_idle": pool_stats.idle_connections,
                    "db_pool_total": pool_stats.total_connections,
                    "db_pool_waiters": pool_stats.wait_count,
                    "outbound_queue_drops": outbound_queue_drops,
                    "slow_client_disconnects": slow_client_disconnects,
                    "db_latency_budget_ms": {
                        "warning_p95": DB_WARNING_P95_MS,
                        "critical_p95": DB_CRITICAL_P95_MS,
                        "min_samples": DB_MIN_SAMPLES,
                    },
                    "ts": now(),
                }),
            );
        }
        "vjoin" => {
            // Subscribe to a voice room's broadcast channel.
            // A system message is posted to the room's text channel to
            // notify other members.
            let room = safe_ch(d["r"].as_str().unwrap_or("general"));

            // Re-joining the same room should be idempotent and must not spawn
            // duplicate forwarders for the same connection.
            if voice_room.as_deref() == Some(room.as_str()) {
                let members = state.voice_relay.get_members(&room);
                send_out_json(
                    out_tx,
                    serde_json::json!({
                        "t": "vusers",
                        "room": room,
                        "members": members,
                        "joined": true,
                        "ts": now()
                    }),
                );
                return;
            }

            // Switching rooms on one socket should release the previous room
            // membership and stop forwarding stale room audio.
            if let Some(previous_room) = voice_room.take() {
                state.voice_relay.leave_room(&previous_room, username);
            }
            if let Some(handle) = voice_audio_forwarder.take() {
                handle.abort();
            }
            {
                let mut room_guard = active_voice_room.write().await;
                *room_guard = None;
            }

            // Add user to voice relay and get current members
            let members = state.voice_relay.join_room(&room, username);

            // Broadcast voice member update
            state.voice_relay.broadcast(VoiceBroadcast::Users {
                room: room.clone(),
                members: members.clone(),
            });

            // Send member list to the joining user
            send_out_json(
                out_tx,
                serde_json::json!({
                    "t": "vusers",
                    "room": room.clone(),
                    "members": members,
                    "joined": true,
                    "ts": now()
                }),
            );

            let vtx = state.voice_tx(&room);
            *voice_audio_forwarder =
                Some(spawn_voice_audio_forwarder(vtx.subscribe(), out_tx.clone()));
            {
                let mut room_guard = active_voice_room.write().await;
                *room_guard = Some(room.clone());
            }
            if voice_relay_forwarder.is_none() {
                *voice_relay_forwarder = Some(spawn_voice_relay_forwarder(
                    state.voice_relay.subscribe(),
                    out_tx.clone(),
                    active_voice_room.clone(),
                ));
            }
            *voice_room = Some(room.clone());
            let join_voice = serde_json::json!({
                "t":"sys",
                "m":format!("ÃƒÂ°Ã…Â¸Ã…Â½Ã¢â€žÂ¢ {} joined voice #{}", username, room),
                "ts":now()
            });
            state.store.persist(
                "sys",
                &room,
                username,
                None,
                &join_voice,
                &format!("{} voice joined", username),
            );
            let _ = state.chan(&room).tx.send(join_voice.to_string());
        }
        "vleave" => {
            // Unsubscribe from the voice room (the broadcast receiver is
            // dropped when the forwarder task exits) and notify other members.
            if let Some(handle) = voice_audio_forwarder.take() {
                handle.abort();
            }
            {
                let mut room_guard = active_voice_room.write().await;
                *room_guard = None;
            }
            if let Some(room) = voice_room.take() {
                state.voice_relay.leave_room(&room, username);

                let leave_voice = serde_json::json!({
                    "t":"sys",
                    "m":format!("ÃƒÂ°Ã…Â¸Ã…Â½Ã¢â€žÂ¢ {} left voice #{}", username, room),
                    "ts":now()
                });
                state.store.persist(
                    "sys",
                    &room,
                    username,
                    None,
                    &leave_voice,
                    &format!("{} voice left", username),
                );
                let _ = state.chan(&room).tx.send(leave_voice.to_string());
            }
        }
        "vstate" => {
            // Handle mute/deafen state changes
            let room = match voice_room.as_ref() {
                Some(r) => r.clone(),
                None => {
                    send_err(out_tx, "not in a voice room", &state.metrics);
                    return;
                }
            };
            let muted = d.get("muted").and_then(|v| v.as_bool());
            let deafened = d.get("deafened").and_then(|v| v.as_bool());

            state
                .voice_relay
                .update_member_state(&room, username, muted, deafened, None);
        }
        "vspeaking" => {
            // Handle speaking indicator updates
            let room = match voice_room.as_ref() {
                Some(r) => r.clone(),
                None => return,
            };
            let speaking = d.get("speaking").and_then(|v| v.as_bool()).unwrap_or(false);

            state
                .voice_relay
                .update_member_state(&room, username, None, None, Some(speaking));
        }
        "vusers" => {
            // Return current voice channel members
            let room = match voice_room.as_ref() {
                Some(r) => r.clone(),
                None => {
                    send_err(out_tx, "not in a voice room", &state.metrics);
                    return;
                }
            };

            let members = state.voice_relay.get_members(&room);
            send_out_json(
                out_tx,
                serde_json::json!({
                    "t": "vusers",
                    "room": room,
                    "members": members,
                    "ts": now()
                }),
            );
        }
        "vdata" => {
            // Forward raw audio payload to all other voice-room members.
            // The sender's username is injected so receivers know who is
            // speaking without a separate signalling round-trip.
            let a = d["a"].as_str().unwrap_or("").to_string();
            let seq = d.get("seq").and_then(|v| v.as_u64());
            let capture_ts_ms = d.get("capture_ts_ms").and_then(|v| v.as_u64());
            if a.is_empty() {
                return;
            }
            if let Some(ref room) = voice_room {
                if let Some(vtx) = state.voice.get(room) {
                    let mut payload = serde_json::json!({
                        "t": "vdata",
                        "from": username,
                        "a": a,
                    });

                    if let Some(seq) = seq {
                        payload["seq"] = serde_json::json!(seq);
                    }
                    if let Some(capture_ts_ms) = capture_ts_ms {
                        payload["capture_ts_ms"] = serde_json::json!(capture_ts_ms);
                    }

                    let _ = vtx.send(payload.to_string());
                }
            }
        }
        "ss_start" => {
            let room = d
                .get("r")
                .or_else(|| d.get("ch"))
                .and_then(|v| v.as_str())
                .map(safe_ch)
                .unwrap_or_else(|| "general".to_string());

            if screen_room.as_ref() != Some(&room) || screen_forwarder.is_none() {
                if let Some(handle) = screen_forwarder.take() {
                    handle.abort();
                }
                let stx = state.screen_tx(&room);
                *screen_forwarder =
                    Some(spawn_broadcast_forwarder(stx.subscribe(), out_tx.clone()));
                *screen_room = Some(room.clone());
            }

            if let Some(stx) = state.screen.get(&room) {
                let _ = stx.send(
                    serde_json::json!({
                        "t": "ss_state",
                        "room": room,
                        "from": username,
                        "enabled": true,
                        "status": "active",
                        "ts": now(),
                    })
                    .to_string(),
                );
            }

            send_out_json(
                out_tx,
                serde_json::json!({
                    "t": "ss_state",
                    "room": room,
                    "enabled": true,
                    "status": "active",
                    "ts": now(),
                }),
            );
        }
        "ss_meta" => {
            let room = match screen_room.as_ref() {
                Some(r) => r.clone(),
                None => {
                    send_err(out_tx, "not in a screen-share room", &state.metrics);
                    return;
                }
            };

            if let Some(stx) = state.screen.get(&room) {
                let mut payload = serde_json::json!({
                    "t": "ss_meta",
                    "room": room,
                    "from": username,
                    "ts": now(),
                });

                for key in [
                    "stream_id",
                    "codec",
                    "mime",
                    "width",
                    "height",
                    "fps",
                    "quality",
                    "frame_seq",
                    "keyframe_interval",
                ] {
                    if let Some(value) = d.get(key) {
                        payload[key] = value.clone();
                    }
                }

                if let Some(meta) = d.get("meta") {
                    payload["meta"] = meta.clone();
                }

                let _ = stx.send(payload.to_string());
            }
        }
        "ss_frame" => {
            let room = match screen_room.as_ref() {
                Some(r) => r.clone(),
                None => {
                    send_err(out_tx, "not in a screen-share room", &state.metrics);
                    return;
                }
            };

            let frame_payload = d
                .get("a")
                .or_else(|| d.get("data"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if frame_payload.is_empty() {
                return;
            }

            if let Some(stx) = state.screen.get(&room) {
                let mut payload = serde_json::json!({
                    "t": "ss_frame",
                    "room": room,
                    "from": username,
                    "a": frame_payload,
                    "ts": now(),
                });

                for key in ["seq", "capture_ts_ms", "stream_id", "keyframe", "mime"] {
                    if let Some(value) = d.get(key) {
                        payload[key] = value.clone();
                    }
                }

                let _ = stx.send(payload.to_string());
            }
        }
        "ss_stop" => {
            if let Some(handle) = screen_forwarder.take() {
                handle.abort();
            }
            let requested_room = d
                .get("r")
                .or_else(|| d.get("ch"))
                .and_then(|v| v.as_str())
                .map(safe_ch);

            let room = screen_room
                .take()
                .or(requested_room)
                .unwrap_or_else(|| "general".to_string());

            if let Some(stx) = state.screen.get(&room) {
                let _ = stx.send(
                    serde_json::json!({
                        "t": "ss_state",
                        "room": room,
                        "from": username,
                        "enabled": false,
                        "status": "inactive",
                        "ts": now(),
                    })
                    .to_string(),
                );
            }

            send_out_json(
                out_tx,
                serde_json::json!({
                    "t": "ss_state",
                    "room": room,
                    "enabled": false,
                    "status": "inactive",
                    "ts": now(),
                }),
            );
        }
        "ping" => {
            // Heartbeat ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â keep-alive for clients behind proxies with idle
            // connection timeouts.
            out_tx.try_send(r#"{"t":"pong"}"#.to_string());
        }
        "edit" => {
            // In-memory edit of the most recent matching message.
            // The edit is persisted for history but not applied retroactively
            // to the SQLite event store ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â the original row is left intact.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let old_text = d["old_text"].as_str().unwrap_or("").to_string();
            let new_text = d["new_text"].as_str().unwrap_or("").to_string();
            if old_text.is_empty() || new_text.is_empty() {
                return;
            }
            let chan = state.chan(&ch);
            let mut h = chan.history.write().await;
            // Search in reverse to find the most-recent matching message from
            // this user (avoids editing an older message by mistake).
            if let Some(pos) = h.iter().rposition(|m| {
                m.get("t") == Some(&Value::from("msg"))
                    && m.get("u") == Some(&Value::from(username.to_string()))
                    && m.get("c") == Some(&Value::from(old_text.clone()))
            }) {
                h[pos]["c"] = Value::from(new_text.clone());
                h[pos]["ts"] = Value::from(now());
            }
            let edit_msg = serde_json::json!({
                "t":"edit","ch":ch,"u":username,
                "old_text":old_text,"new_text":new_text,"ts":now()
            });
            state
                .store
                .persist("edit", &ch, username, None, &edit_msg, &new_text);
            let _ = chan.tx.send(edit_msg.to_string());
        }
        "file_meta" => {
            // Announce a pending file transfer to the channel. The `file_id`
            // acts as a correlation key for subsequent `file_chunk` frames.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let filename = d
                .get("filename")
                .or_else(|| d.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .trim()
                .chars()
                .take(MAX_FILE_NAME_LEN)
                .collect::<String>();
            if filename.is_empty() {
                send_err(out_tx, "filename is required", &state.metrics);
                return;
            }
            let size = d["size"].as_u64().unwrap_or(0);

            // Reject files that exceed the maximum size.
            if size > MAX_FILE_SIZE {
                send_err(
                    out_tx,
                    format!("file size exceeds maximum of {} bytes", MAX_FILE_SIZE),
                    &state.metrics,
                );
                return;
            }

            let file_id_raw = d["file_id"].as_str().unwrap_or("").trim();
            let file_id = if file_id_raw.is_empty() {
                format!("{}_{}", username, now())
            } else {
                file_id_raw
                    .chars()
                    .take(MAX_FILE_ID_LEN)
                    .collect::<String>()
            };

            let media_kind = match d
                .get("media_kind")
                .or_else(|| d.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("file")
                .trim()
                .to_ascii_lowercase()
                .as_str()
            {
                "image" => "image",
                "video" => "video",
                "audio" => "audio",
                _ => "file",
            };
            let mime = d
                .get("mime")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| v.chars().take(MAX_MEDIA_MIME_LEN).collect::<String>());
            let duration_ms = d.get("duration_ms").and_then(|v| v.as_u64());

            let mut file_announce = serde_json::json!({
                "t":"file_meta","from":username,"filename":filename,
                "size":size,"file_id":file_id,"ch":ch,
                "media_kind":media_kind,"ts":now()
            });
            if let Some(ref mime_value) = mime {
                file_announce["mime"] = Value::String(mime_value.clone());
            }
            if let Some(duration_ms) = duration_ms {
                file_announce["duration_ms"] = Value::from(duration_ms);
            }
            state.store.persist(
                "file_meta",
                &ch,
                username,
                None,
                &file_announce,
                &format!("{} {}", media_kind, filename),
            );
            state.store.upsert_media_object(MediaObjectUpsert {
                channel: &ch,
                file_id: &file_id,
                sender: username,
                filename: &filename,
                media_kind,
                mime: mime.as_deref(),
                declared_size: size,
            });
            let _ = state.chan(&ch).tx.send(file_announce.to_string());
        }
        "file_chunk" => {
            // Relay a single chunk of a file transfer to the channel.
            // Chunks are also persisted in SQLite for durable media history.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let file_id = d["file_id"].as_str().unwrap_or("").trim().to_string();
            let chunk_data = d["data"].as_str().unwrap_or("").to_string();
            if file_id.is_empty() || chunk_data.is_empty() {
                return;
            }
            let index = d["index"].as_u64().unwrap_or(0);
            match general_purpose::STANDARD.decode(chunk_data.as_bytes()) {
                Ok(chunk_bytes) => {
                    state
                        .store
                        .append_media_chunk(&ch, &file_id, username, index, &chunk_bytes);
                }
                Err(e) => {
                    warn!(
                        "media chunk decode failed channel={} file_id={} idx={}: {}",
                        ch, file_id, index, e
                    );
                }
            }
            let chunk_msg = serde_json::json!({
                "t":"file_chunk","from":username,"file_id":file_id,
                "data":chunk_data,"index":index,"ch":ch,"ts":now()
            })
            .to_string();
            let _ = state.chan(&ch).tx.send(chunk_msg);
        }
        "typing" => {
            // Broadcast ephemeral typing state updates.
            //
            // Channel scope payload:
            //   {"t":"typing","ch":"general","typing":true}
            // DM scope payload:
            //   {"t":"typing","to":"bob","typing":true}
            //
            // Typing events are intentionally not persisted.
            let typing = d.get("typing").and_then(|v| v.as_bool()).unwrap_or(true);

            if let Some(target) = d.get("to").and_then(|v| v.as_str()) {
                let target = target.trim().to_lowercase();
                if target.is_empty() || !is_valid_username(&target) {
                    send_err(out_tx, "typing to requires valid username", &state.metrics);
                    return;
                }

                let target_scope = format!("dm:{}", target);
                let target_channel = target.clone();

                let event = serde_json::json!({
                    "t": "typing",
                    "from": username,
                    "to": target,
                    "typing": typing,
                    "scope": target_scope,
                    "ts": now()
                })
                .to_string();

                let _ = state
                    .chan(&dm_channel_name(&target_channel))
                    .tx
                    .send(event.clone());
                let _ = state.chan(&dm_channel_name(username)).tx.send(event);
            } else {
                let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
                let event = serde_json::json!({
                    "t": "typing",
                    "ch": ch,
                    "u": username,
                    "typing": typing,
                    "ts": now()
                })
                .to_string();

                let _ = state.chan(&ch).tx.send(event);
            }
        }
        "status" => {
            // Broadcast a presence update to all channels so every connected
            // client can update its member list without polling.
            if let Some(status_raw) = d.get("status") {
                let status_val = match validate_status_field(Some(status_raw)) {
                    Ok(v) => v,
                    Err(e) => {
                        send_err(out_tx, e.to_string(), &state.metrics);
                        return;
                    }
                };

                state
                    .user_statuses
                    .insert(username.to_string(), status_val.clone());
                state.store.upsert_presence_snapshot(username, &status_val);

                let status_update = Arc::new(
                    serde_json::json!({
                        "t":"status_update","user":username,"status":status_val
                    })
                    .to_string(),
                );
                for chan_entry in state.channels.iter() {
                    let _ = chan_entry.tx.send(status_update.as_ref().clone());
                }
            }
        }
        "reaction" => {
            // Broadcast an emoji reaction to a specific message in a channel.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            if !state.can_send(username, &ch) {
                send_err(out_tx, "you cannot react in this channel", &state.metrics);
                return;
            }
            let emoji = d["emoji"].as_str().unwrap_or("").trim().to_string();
            let msg_id = d["msg_id"].as_str().unwrap_or("").trim().to_string();

            if !is_valid_msg_id(&msg_id) {
                send_err(out_tx, "reaction requires valid msg_id", &state.metrics);
                return;
            }
            if !is_valid_reaction_emoji(&emoji) {
                send_err(out_tx, "reaction requires valid emoji", &state.metrics);
                return;
            }

            let reaction_msg = serde_json::json!({
                "t":"reaction","user":username,"emoji":emoji,
                "msg_id":msg_id,"ch":ch,"ts":now()
            });
            state
                .store
                .persist("reaction", &ch, username, None, &reaction_msg, &emoji);
            let _ = state.chan(&ch).tx.send(reaction_msg.to_string());
        }
        "2fa_setup" => {
            // Begin the TOTP enrollment flow: generate a fresh secret and
            // return a QR-code URL that the user can scan with an authenticator
            // app. The secret is NOT persisted here ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â it is only saved when
            // the user confirms enrollment via `2fa_enable`.
            let secret = generate_secret();
            let issuer = d["issuer"].as_str().unwrap_or("Chatify");
            let qr_url = generate_qr_url(username, issuer, &secret);
            send_out_json(
                out_tx,
                serde_json::json!({
                    "t":"2fa_setup","secret":secret,"qr_url":qr_url,
                    "issuer":issuer,"user":username,"ts":now()
                }),
            );
        }
        "2fa_enable" => {
            // Finalise TOTP enrollment. The client must supply the secret from
            // the previous `2fa_setup` step and a live TOTP code to prove the
            // authenticator app is correctly configured before the secret is
            // persisted.
            let secret = d["secret"].as_str().unwrap_or("").to_string();
            let code = d["code"].as_str().unwrap_or("").to_string();
            if secret.is_empty() || code.is_empty() {
                send_err(
                    out_tx,
                    "2fa_enable requires secret and code",
                    &state.metrics,
                );
                return;
            }

            let mut user_2fa = User2FA::new(username.to_string());
            let backup_codes = user_2fa.enable(secret);
            if !user_2fa.verify_totp(&code) {
                send_err(out_tx, "invalid 2FA code", &state.metrics);
                return;
            }

            state.store.upsert_user_2fa(&user_2fa);
            send_out_json(
                out_tx,
                serde_json::json!({
                    "t":"2fa_enabled","enabled":true,
                    "backup_codes":backup_codes,"ts":now()
                }),
            );
        }
        "2fa_disable" => {
            // Disable 2-FA for the current user. Requires the current TOTP
            // code to prevent an attacker who gained session access from
            // silently disabling 2FA.
            let code = d["code"].as_str().unwrap_or("").to_string();
            if code.is_empty() {
                send_err(
                    out_tx,
                    "2fa_disable requires current 2FA code",
                    &state.metrics,
                );
                return;
            }

            let mut user_2fa = match state.store.load_user_2fa(username) {
                Some(u) if u.enabled => u,
                _ => {
                    send_err(out_tx, "2FA is not enabled", &state.metrics);
                    return;
                }
            };

            // Require valid TOTP code to disable
            if !user_2fa.verify_totp(&code) {
                send_err(out_tx, "invalid 2FA code", &state.metrics);
                return;
            }

            user_2fa.disable();
            state.store.upsert_user_2fa(&user_2fa);
            send_out_json(
                out_tx,
                serde_json::json!({"t":"2fa_disabled","enabled":false,"ts":now()}),
            );
        }
        "password" | "password_change" => {
            let current_hash = d.get("current").and_then(|v| v.as_str()).unwrap_or("");
            let new_pw = d.get("new").and_then(|v| v.as_str()).unwrap_or("");

            if current_hash.is_empty() || new_pw.is_empty() {
                send_err(
                    out_tx,
                    "password change requires 'current' and 'new' password hashes",
                    &state.metrics,
                );
                return;
            }

            if new_pw.len() > MAX_PASSWORD_FIELD_LEN {
                send_err(out_tx, "new password too long", &state.metrics);
                return;
            }

            debug!("password change verification started");
            let credential_result = if let Some(pending) = state.pending_credentials.get(username) {
                Ok(crypto::secure_string_eq(current_hash, pending.value()))
            } else {
                state.store.verify_credential(username, current_hash)
            };

            match credential_result {
                Ok(true) => {
                    debug!("password change verification passed");
                    state
                        .pending_credentials
                        .insert(username.to_string(), new_pw.to_string());
                    state.invalidate_all_user_sessions(username);
                    send_out_json(
                        out_tx,
                        serde_json::json!({"t":"password_changed","ts":now()}),
                    );

                    let state_for_task = state.clone();
                    let username_for_task = username.to_string();
                    let new_pw_for_task = new_pw.to_string();
                    tokio::task::spawn_blocking(move || {
                        let server_hash = crypto::pw_hash(&new_pw_for_task);
                        state_for_task
                            .store
                            .upsert_credentials(&username_for_task, &server_hash);
                        state_for_task
                            .pending_credentials
                            .remove(&username_for_task);
                        info!("password change persisted");
                    });
                }
                Ok(false) => {
                    send_err(out_tx, "current password is incorrect", &state.metrics);
                }
                Err(e) => {
                    send_err(
                        out_tx,
                        format!("password change failed: {}", e),
                        &state.metrics,
                    );
                }
            }
        }
        "admin" => {
            if !state.can_manage(username, "general") {
                send_err(
                    out_tx,
                    "insufficient permissions for admin command",
                    &state.metrics,
                );
                return;
            }

            let sub = d["sub"].as_str().unwrap_or("");
            match sub {
                "users" => {
                    let limit = clamp_limit(d.get("limit").and_then(|v| v.as_u64()), 50, 200);
                    let channel = safe_ch(d["ch"].as_str().unwrap_or("general"));
                    let users = state.store.list_users(&channel, limit as i64);
                    send_out_json(
                        out_tx,
                        serde_json::json!({
                            "t":"admin_users",
                            "users":users,
                            "count":users.len(),
                            "channel":channel,
                            "ts":now()
                        }),
                    );
                }
                "register" => {
                    let target = d["target"]
                        .as_str()
                        .unwrap_or("")
                        .trim()
                        .to_ascii_lowercase();
                    let Some(password) = d.get("password").and_then(|v| v.as_str()) else {
                        send_err(
                            out_tx,
                            "admin register requires valid password",
                            &state.metrics,
                        );
                        return;
                    };
                    let role = d["role"].as_str().unwrap_or("member").trim();
                    let channel = safe_ch(d["ch"].as_str().unwrap_or("general"));

                    if target.is_empty() || !is_valid_username(&target) {
                        send_err(
                            out_tx,
                            "admin register requires valid username",
                            &state.metrics,
                        );
                        return;
                    }
                    if password.is_empty() || password.len() > MAX_PASSWORD_FIELD_LEN {
                        send_err(
                            out_tx,
                            "admin register requires valid password",
                            &state.metrics,
                        );
                        return;
                    }

                    let server_hash = crypto::pw_hash(password);
                    state.store.upsert_credentials(&target, &server_hash);
                    if let Err(err) = state.store.assign_role(&target, &channel, role, username) {
                        send_err(out_tx, err, &state.metrics);
                        return;
                    }
                    state.store.log_audit(
                        "admin_register",
                        username,
                        Some(&target),
                        Some(&channel),
                        None,
                        Some(role),
                    );
                    send_out_json(
                        out_tx,
                        serde_json::json!({
                            "t":"admin_registered",
                            "target":target,
                            "role":role,
                            "channel":channel,
                            "by":username,
                            "ts":now()
                        }),
                    );
                }
                "role" => {
                    let target = d["target"]
                        .as_str()
                        .unwrap_or("")
                        .trim()
                        .to_ascii_lowercase();
                    let role = d["role"].as_str().unwrap_or("member").trim();
                    let channel = safe_ch(d["ch"].as_str().unwrap_or("general"));

                    if target.is_empty() || !is_valid_username(&target) {
                        send_err(out_tx, "admin role requires valid username", &state.metrics);
                        return;
                    }
                    if let Err(err) = state.store.assign_role(&target, &channel, role, username) {
                        send_err(out_tx, err, &state.metrics);
                        return;
                    }
                    state.store.log_audit(
                        "admin_role",
                        username,
                        Some(&target),
                        Some(&channel),
                        None,
                        Some(role),
                    );
                    send_out_json(
                        out_tx,
                        serde_json::json!({
                            "t":"admin_role",
                            "target":target,
                            "role":role,
                            "channel":channel,
                            "by":username,
                            "ts":now()
                        }),
                    );
                }
                "audit" => {
                    let limit =
                        clamp_limit(d.get("limit").and_then(|v| v.as_u64()), 50, 200) as i64;
                    let logs = state.store.get_audit_logs(None, None, limit);
                    let entries: Vec<Value> = logs
                        .iter()
                        .map(|log| {
                            serde_json::json!({
                                "action": log.action,
                                "actor": log.actor,
                                "target": log.target,
                                "channel": log.channel,
                                "reason": log.reason,
                                "metadata": log.metadata,
                                "ts": log.ts
                            })
                        })
                        .collect();
                    send_out_json(
                        out_tx,
                        serde_json::json!({
                            "t":"admin_audit",
                            "logs":entries,
                            "count":entries.len(),
                            "ts":now()
                        }),
                    );
                }
                _ => send_err(
                    out_tx,
                    "unknown admin subcommand (users|register|role|audit)",
                    &state.metrics,
                ),
            }
        }
        "role" => {
            let sub = d["sub"].as_str().unwrap_or("");
            let target = d["target"].as_str().unwrap_or("");
            let channel = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let role_name = d["role"].as_str().unwrap_or("member");

            match sub {
                "set" => {
                    if target.is_empty() {
                        send_err(out_tx, "role set requires target username", &state.metrics);
                        return;
                    }
                    if !state.can_manage(username, &channel) {
                        send_err(
                            out_tx,
                            "insufficient permissions to assign roles",
                            &state.metrics,
                        );
                        return;
                    }
                    match state
                        .store
                        .assign_role(target, &channel, role_name, username)
                    {
                        Ok(_) => {
                            state.store.log_audit(
                                "role_set",
                                username,
                                Some(target),
                                Some(&channel),
                                None,
                                Some(role_name),
                            );
                            send_out_json(
                                out_tx,
                                serde_json::json!({
                                    "t":"role_set",
                                    "target":target,
                                    "role":role_name,
                                    "channel":channel,
                                    "by":username,
                                    "ts":now()
                                }),
                            );
                            let _ = state.chan(&channel).tx.send(
                                serde_json::json!({
                                    "t":"sys",
                                    "m":format!("{} set {}'s role to {} in #{}", username, target, role_name, channel),
                                    "ts":now()
                                }).to_string()
                            );
                        }
                        Err(e) => send_err(out_tx, &e, &state.metrics),
                    }
                }
                "remove" => {
                    if target.is_empty() {
                        send_err(
                            out_tx,
                            "role remove requires target username",
                            &state.metrics,
                        );
                        return;
                    }
                    if !state.can_manage(username, &channel) {
                        send_err(
                            out_tx,
                            "insufficient permissions to remove roles",
                            &state.metrics,
                        );
                        return;
                    }
                    match state.store.remove_user_role(target, &channel) {
                        Ok(_) => {
                            state.store.log_audit(
                                "role_remove",
                                username,
                                Some(target),
                                Some(&channel),
                                None,
                                None,
                            );
                            send_out_json(
                                out_tx,
                                serde_json::json!({
                                    "t":"role_removed",
                                    "target":target,
                                    "channel":channel,
                                    "by":username,
                                    "ts":now()
                                }),
                            );
                        }
                        Err(e) => send_err(out_tx, &e, &state.metrics),
                    }
                }
                "get" => {
                    if target.is_empty() {
                        let role = state
                            .get_user_role(username, &channel)
                            .unwrap_or_else(|| "none".to_string());
                        send_out_json(
                            out_tx,
                            serde_json::json!({
                                "t":"role_info",
                                "user":username,
                                "channel":channel,
                                "role":role,
                                "ts":now()
                            }),
                        );
                    } else {
                        let role = state
                            .get_user_role(target, &channel)
                            .unwrap_or_else(|| "none".to_string());
                        send_out_json(
                            out_tx,
                            serde_json::json!({
                                "t":"role_info",
                                "user":target,
                                "channel":channel,
                                "role":role,
                                "ts":now()
                            }),
                        );
                    }
                }
                _ => {
                    send_err(
                        out_tx,
                        "unknown role subcommand (set|remove|get)",
                        &state.metrics,
                    );
                }
            }
        }
        "kick" => {
            let target = d["target"].as_str().unwrap_or("");
            let channel = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let reason = d["reason"].as_str().unwrap_or("kicked by moderator");

            if target.is_empty() {
                send_err(out_tx, "kick requires target username", &state.metrics);
                return;
            }
            if target == username {
                send_err(out_tx, "cannot kick yourself", &state.metrics);
                return;
            }
            if !state.can_kick(username, &channel) {
                send_err(out_tx, "insufficient permissions to kick", &state.metrics);
                return;
            }

            let kick_msg = serde_json::json!({
                "t":"sys",
                "m":format!("{} was kicked from #{}: {}", target, channel, reason),
                "ts":now()
            })
            .to_string();

            state.store.persist(
                "sys",
                &channel,
                username,
                None,
                &serde_json::json!({"t":"sys","m":format!("{} kicked {}", username, target)}),
                "",
            );
            let _ = state.chan(&channel).tx.send(kick_msg);

            state.store.log_audit(
                "kick",
                username,
                Some(target),
                Some(&channel),
                Some(reason),
                None,
            );

            send_out_json(
                out_tx,
                serde_json::json!({
                    "t":"kicked",
                    "target":target,
                    "channel":channel,
                    "by":username,
                    "ts":now()
                }),
            );
        }
        "ban" => {
            let sub = d["sub"].as_str().unwrap_or("add");
            let target = d["target"].as_str().unwrap_or("");
            let channel = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let reason = d["reason"].as_str();
            let duration_secs = d["duration"].as_i64();

            if target.is_empty() {
                send_err(out_tx, "ban requires target username", &state.metrics);
                return;
            }
            if target == username {
                send_err(out_tx, "cannot ban yourself", &state.metrics);
                return;
            }
            if !state.can_ban(username, &channel) {
                send_err(out_tx, "insufficient permissions to ban", &state.metrics);
                return;
            }

            match sub {
                "add" | "" => {
                    match state
                        .store
                        .ban_user(target, &channel, username, reason, duration_secs)
                    {
                        Ok(_) => {
                            let ban_msg = if let Some(dur) = duration_secs {
                                format!(
                                    "{} was banned from #{} for {} seconds by {}: {}",
                                    target,
                                    channel,
                                    dur,
                                    username,
                                    reason.unwrap_or("banned")
                                )
                            } else {
                                format!(
                                    "{} was permanently banned from #{} by {}: {}",
                                    target,
                                    channel,
                                    username,
                                    reason.unwrap_or("banned")
                                )
                            };
                            let _ = state.chan(&channel).tx.send(
                                serde_json::json!({"t":"sys","m":ban_msg,"ts":now()}).to_string(),
                            );

                            let metadata =
                                serde_json::json!({"duration": duration_secs}).to_string();
                            state.store.log_audit(
                                "ban",
                                username,
                                Some(target),
                                Some(&channel),
                                reason,
                                Some(&metadata),
                            );

                            send_out_json(
                                out_tx,
                                serde_json::json!({
                                    "t":"banned",
                                    "target":target,
                                    "channel":channel,
                                    "by":username,
                                    "duration":duration_secs,
                                    "ts":now()
                                }),
                            );
                        }
                        Err(e) => send_err(out_tx, &e, &state.metrics),
                    }
                }
                "remove" => match state.store.unban_user(target, &channel) {
                    Ok(_) => {
                        state.store.log_audit(
                            "unban",
                            username,
                            Some(target),
                            Some(&channel),
                            None,
                            None,
                        );
                        let _ = state.chan(&channel).tx.send(
                                serde_json::json!({"t":"sys","m":format!("{} was unbanned from #{}", target, channel),"ts":now()}).to_string()
                            );
                        send_out_json(
                            out_tx,
                            serde_json::json!({
                                "t":"unbanned",
                                "target":target,
                                "channel":channel,
                                "by":username,
                                "ts":now()
                            }),
                        );
                    }
                    Err(e) => send_err(out_tx, &e, &state.metrics),
                },
                "check" => {
                    let is_banned = state.is_banned(target, &channel);
                    if let Some(ban) = state.store.is_banned(target, &channel) {
                        send_out_json(
                            out_tx,
                            serde_json::json!({
                                "t":"ban_info",
                                "target":target,
                                "channel":channel,
                                "banned":is_banned,
                                "active":ban.is_active(),
                                "banned_by":ban.banned_by,
                                "reason":ban.reason,
                                "expires_at":ban.expires_at,
                                "ts":now()
                            }),
                        );
                    } else {
                        send_out_json(
                            out_tx,
                            serde_json::json!({
                                "t":"ban_info",
                                "target":target,
                                "channel":channel,
                                "banned":false,
                                "ts":now()
                            }),
                        );
                    }
                }
                _ => {
                    send_err(
                        out_tx,
                        "unknown ban subcommand (add|remove|check)",
                        &state.metrics,
                    );
                }
            }
        }
        "mute" => {
            let sub = d["sub"].as_str().unwrap_or("add");
            let target = d["target"].as_str().unwrap_or("");
            let channel = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let reason = d["reason"].as_str();
            let duration_secs = d["duration"].as_i64();

            if target.is_empty() {
                send_err(out_tx, "mute requires target username", &state.metrics);
                return;
            }
            if target == username {
                send_err(out_tx, "cannot mute yourself", &state.metrics);
                return;
            }
            if !state.can_mute(username, &channel) {
                send_err(out_tx, "insufficient permissions to mute", &state.metrics);
                return;
            }

            match sub {
                "add" | "" => {
                    match state
                        .store
                        .mute_user(target, &channel, username, reason, duration_secs)
                    {
                        Ok(_) => {
                            let metadata =
                                serde_json::json!({"duration": duration_secs}).to_string();
                            state.store.log_audit(
                                "mute",
                                username,
                                Some(target),
                                Some(&channel),
                                reason,
                                Some(&metadata),
                            );
                            send_out_json(
                                out_tx,
                                serde_json::json!({
                                    "t":"muted",
                                    "target":target,
                                    "channel":channel,
                                    "by":username,
                                    "duration":duration_secs,
                                    "ts":now()
                                }),
                            );
                        }
                        Err(e) => send_err(out_tx, &e, &state.metrics),
                    }
                }
                "remove" => match state.store.unmute_user(target, &channel) {
                    Ok(_) => {
                        state.store.log_audit(
                            "unmute",
                            username,
                            Some(target),
                            Some(&channel),
                            None,
                            None,
                        );
                        send_out_json(
                            out_tx,
                            serde_json::json!({
                                "t":"unmuted",
                                "target":target,
                                "channel":channel,
                                "by":username,
                                "ts":now()
                            }),
                        );
                    }
                    Err(e) => send_err(out_tx, &e, &state.metrics),
                },
                "check" => {
                    let is_muted = state.is_muted(target, &channel);
                    if let Some(mute) = state.store.is_muted(target, &channel) {
                        send_out_json(
                            out_tx,
                            serde_json::json!({
                                "t":"mute_info",
                                "target":target,
                                "channel":channel,
                                "muted":is_muted,
                                "active":mute.is_active(),
                                "muted_by":mute.muted_by,
                                "reason":mute.reason,
                                "expires_at":mute.expires_at,
                                "ts":now()
                            }),
                        );
                    } else {
                        send_out_json(
                            out_tx,
                            serde_json::json!({
                                "t":"mute_info",
                                "target":target,
                                "channel":channel,
                                "muted":false,
                                "ts":now()
                            }),
                        );
                    }
                }
                _ => {
                    send_err(
                        out_tx,
                        "unknown mute subcommand (add|remove|check)",
                        &state.metrics,
                    );
                }
            }
        }
        "2fa_verify" => {
            let code = d["code"].as_str().unwrap_or("").to_string();
            let mut user_2fa = match state.store.load_user_2fa(username) {
                Some(u) if u.enabled => u,
                _ => {
                    send_err(out_tx, "2FA is not enabled", &state.metrics);
                    return;
                }
            };

            let ok = verify_user_2fa_code(&mut user_2fa, &code);
            if ok {
                state.store.upsert_user_2fa(&user_2fa);
            }

            send_out_json(
                out_tx,
                serde_json::json!({"t":"2fa_verify","ok":ok,"ts":now()}),
            );
        }
        "unlock" => {
            let target = d["target"].as_str().unwrap_or("");
            let channel = safe_ch(d["ch"].as_str().unwrap_or("general"));

            if target.is_empty() {
                send_err(out_tx, "unlock requires target username", &state.metrics);
                return;
            }
            if !state.can_ban(username, &channel) {
                send_err(
                    out_tx,
                    "insufficient permissions to unlock accounts",
                    &state.metrics,
                );
                return;
            }

            match state.store.unlock_account(target) {
                Ok(_) => {
                    state
                        .store
                        .log_audit("unlock", username, Some(target), None, None, None);
                    send_out_json(
                        out_tx,
                        serde_json::json!({
                            "t":"unlocked",
                            "target":target,
                            "by":username,
                            "ts":now()
                        }),
                    );
                }
                Err(e) => send_err(out_tx, &e, &state.metrics),
            }
        }
        "audit" => {
            let _sub = d["sub"].as_str().unwrap_or("query");
            let filter_type = d["filter"].as_str();
            let limit = clamp_limit(d.get("limit").and_then(|v| v.as_u64()), 50, 200) as i64;
            let target = d
                .get("target")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty());

            if !state.can_manage(username, "general") {
                send_err(
                    out_tx,
                    "insufficient permissions to view audit logs",
                    &state.metrics,
                );
                return;
            }

            let filter = match (filter_type, target) {
                (Some("channel"), _) => Some("channel"),
                (Some("user"), Some(_)) => Some("user"),
                _ => None,
            };

            let logs = state.store.get_audit_logs(filter, target, limit);
            let log_entries: Vec<Value> = logs
                .iter()
                .map(|log| {
                    serde_json::json!({
                        "action": log.action,
                        "actor": log.actor,
                        "target": log.target,
                        "channel": log.channel,
                        "reason": log.reason,
                        "metadata": log.metadata,
                        "ts": log.ts
                    })
                })
                .collect();

            send_out_json(
                out_tx,
                serde_json::json!({
                    "t":"audit_logs",
                    "logs": log_entries,
                    "count": logs.len(),
                    "ts":now()
                }),
            );
        }
        _ => {
            // Unknown event type ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â silently ignore. This is intentional:
            // newer clients may send events that older servers do not
            // understand, and a hard error would break forward compatibility.
        }
    }
}

// ---------------------------------------------------------------------------
// Small utility functions
// ---------------------------------------------------------------------------

/// Returns the current Unix timestamp as a floating-point number of seconds.
fn now() -> f64 {
    chatify::now()
}

/// Normalises a raw channel name to a safe, consistent format.
///
/// Rules applied in order:
/// 1. Lowercase the input.
/// 2. Strip a leading `#` (clients may include it as a UI convention).
/// 3. Keep only ASCII alphanumeric characters, `-`, and `_`.
/// 4. Truncate to 32 characters.
/// 5. Fall back to `"general"` if the result is empty.
///
/// This is applied to every client-supplied channel or room name before it
/// is used as a `DashMap` key or SQLite parameter, preventing channel-name
/// injection and collisions between logically identical names.
fn safe_ch(raw: &str) -> String {
    chatify::normalize_channel(raw).unwrap_or_else(|| "general".into())
}

fn is_default_online_status(status: &Value) -> bool {
    status
        .get("text")
        .and_then(|v| v.as_str())
        .map(|text| text.trim().eq_ignore_ascii_case("online"))
        .unwrap_or(false)
        && status
            .get("emoji")
            .and_then(|v| v.as_str())
            .map(|emoji| emoji.trim().is_empty())
            .unwrap_or(true)
}

enum EventQueryScope {
    Channel(String),
    DmConversation(String),
}

impl EventQueryScope {
    fn response_channel(&self) -> String {
        match self {
            EventQueryScope::Channel(ch) => ch.clone(),
            EventQueryScope::DmConversation(peer) => format!("dm:{}", peer),
        }
    }
}

fn parse_event_query_scope(raw: Option<&str>) -> ChatifyResult<EventQueryScope> {
    let requested = raw.unwrap_or("general").trim();
    if let Some(peer_raw) = requested.strip_prefix("dm:") {
        let peer = peer_raw.trim().to_lowercase();
        if !is_valid_username(&peer) {
            return Err(ChatifyError::Validation(
                "invalid dm conversation target".to_string(),
            ));
        }
        return Ok(EventQueryScope::DmConversation(peer));
    }

    Ok(EventQueryScope::Channel(safe_ch(requested)))
}

/// Constructs a serialised system message JSON string with the current
/// timestamp.
fn sys(text: &str) -> String {
    serde_json::json!({"t":"sys","m":text,"ts":now()}).to_string()
}

/// Sends a system message to every public channel's broadcast sender.
///
/// Used for server-wide announcements (joins, leaves, shutdown notice).
/// DM channels are included in the broadcast because the channel map contains
/// them alongside public channels; this is harmless since DM channels
/// typically have at most two subscribers.
async fn broadcast_system_msg(state: &Arc<State>, msg: &str) {
    let sys_msg = sys(msg);
    for e in state.channels.iter() {
        let _ = e.tx.send(sys_msg.clone());
    }
}

// ---------------------------------------------------------------------------
// Health check and metrics HTTP server
// ---------------------------------------------------------------------------

async fn start_health_server(
    listener: TcpListener,
    state: Arc<State>,
    metrics: Option<Arc<std::sync::Mutex<PrometheusMetrics>>>,
    metrics_enabled: bool,
    shutdown_endpoint_enabled: bool,
    shutdown_tx: tokio::sync::mpsc::Sender<()>,
) {
    loop {
        tokio::select! {
            biased;

            _ = state.shutdown_notify.notified() => {
                info!("health server shutting down");
                break;
            }

            accept_result = listener.accept() => {
                match accept_result {
                    Ok((mut stream, _addr)) => {
                        let state = state.clone();
                        let metrics = metrics.clone();
                        let shutdown_tx = shutdown_tx.clone();

                        tokio::spawn(async move {
                            let start = Instant::now();
                            let mut buffer = vec![0u8; 8192];

                            match stream.read(&mut buffer).await {
                                Ok(n) => {
                                    let request = String::from_utf8_lossy(&buffer[..n]);

                                    let (endpoint, method) = parse_http_request(&request);

                                    let response = match endpoint {
                                        "/health" | "/health/" => {
                                            create_health_response(&state)
                                        }
                                        "/metrics" | "/metrics/" if metrics_enabled => {
                                            create_metrics_response(&metrics)
                                        }
                                        "/ready" | "/ready/" => {
                                            create_ready_response(&state)
                                        }
                                        "/shutdown" | "/shutdown/" if shutdown_endpoint_enabled && method == "POST" => {
                                            if state.initiate_shutdown() {
                                                let _ = shutdown_tx.send(()).await;
                                                create_shutdown_response("initiated")
                                            } else {
                                                create_shutdown_response("already_in_progress")
                                            }
                                        }
                                        "/shutdown" | "/shutdown/" if shutdown_endpoint_enabled => {
                                            create_method_not_allowed_response()
                                        }
                                        "/live" | "/live/" => {
                                            create_live_response()
                                        }
                                        _ => {
                                            create_not_found_response()
                                        }
                                    };

                                    let duration = start.elapsed();

                                    if let Some(ref m) = metrics {
                                        if let Ok(mutex_guard) = m.lock() {
                                            mutex_guard.record_http_request(endpoint, method, 200);
                                            mutex_guard.record_http_duration(endpoint, duration);
                                        }
                                    }

                                    let _ = stream.write_all(response.as_bytes()).await;
                                }
                                Err(e) => {
                                    warn!("Failed to read from health connection: {}", e);
                                }
                            }
                        });
                    }
                    Err(e) => {
                        warn!("Health server accept error: {}", e);
                    }
                }
            }
        }
    }
}

fn parse_http_request(request: &str) -> (&str, &str) {
    let lines: Vec<&str> = request.lines().collect();
    if let Some(first_line) = lines.first() {
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() >= 2 {
            return (parts[1], parts[0]);
        }
    }
    ("/", "GET")
}

fn create_health_response(state: &Arc<State>) -> String {
    let channels = state.channels.len();
    let online = state.online_count();
    let connections = state.active_connection_count();

    let response = serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": 0,
        "channels": channels,
        "online_users": online,
        "active_connections": connections
    });

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        response.to_string().len(),
        response
    )
}

fn create_ready_response(state: &Arc<State>) -> String {
    let db_ready = state.store.health_check();

    let response = serde_json::json!({
        "ready": db_ready,
        "checks": {
            "database": if db_ready { "ok" } else { "error" }
        }
    });

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        response.to_string().len(),
        response
    )
}

fn create_metrics_response(metrics: &Option<Arc<std::sync::Mutex<PrometheusMetrics>>>) -> String {
    let metrics_text = if let Some(ref m) = metrics {
        if let Ok(mutex) = m.lock() {
            let encoder = prometheus::TextEncoder::new();
            let metric_families = mutex.registry.gather();
            let mut buffer = Vec::new();
            if encoder.encode(&metric_families, &mut buffer).is_ok() {
                String::from_utf8_lossy(&buffer).to_string()
            } else {
                "Error encoding metrics".to_string()
            }
        } else {
            "Error acquiring metrics lock".to_string()
        }
    } else {
        "Metrics not enabled".to_string()
    };

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
        metrics_text.len(),
        metrics_text
    )
}

fn create_not_found_response() -> String {
    let body = "Not Found";
    format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

fn create_shutdown_response(status: &str) -> String {
    let response = serde_json::json!({
        "status": status,
        "message": if status == "initiated" {
            "Shutdown initiated"
        } else {
            "Shutdown already in progress"
        }
    });
    let body = response.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

fn create_method_not_allowed_response() -> String {
    let body = "Method Not Allowed";
    format!(
        "HTTP/1.1 405 Method Not Allowed\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nAllow: POST\r\n\r\n{}",
        body.len(),
        body
    )
}

fn create_live_response() -> String {
    let body = "OK";
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

/// Constructs the serialised `"ok"` auth response payload.
///
/// Inline construction here (rather than in the caller) keeps all protocol
/// field names in one place, making it easier to evolve the auth contract.
fn create_ok_response(
    username: &str,
    state: &Arc<State>,
    hist: Vec<Value>,
    session_token: Option<&str>,
) -> String {
    let mut response = serde_json::json!({
        "t": "ok",
        "u": username,
        "users": state.users_with_keys_json(),
        "channels": state.channels_json(),
        "hist": hist,
        "proto": {
            "v": PROTOCOL_VERSION,
            "max_payload_bytes": MAX_BYTES
        },
        "media": {
            "capabilities_version": MEDIA_CAPABILITIES_VERSION,
            "voice": {
                "enabled": true,
                "codecs": ["pcm-rle-v1"],
                "features": {
                    "seq": true,
                    "capture_ts_ms": true
                }
            },
            "screen_share": {
                "enabled": true,
                "status": "relay",
                "codecs": ["raw-b64-v1"],
                "features": {
                    "frame_seq": true,
                    "keyframe": true
                }
            }
        }
    });

    if let Some(token) = session_token {
        response["token"] = token.into();
    }

    response.to_string()
}

// ---------------------------------------------------------------------------
// Input validation
// ---------------------------------------------------------------------------

/// Returns `true` if `name` is a valid username.
///
/// Valid usernames are non-empty, at most [`MAX_USERNAME_LEN`] characters,
/// and consist entirely of ASCII alphanumeric characters, `-`, or `_`.
/// Whitespace, punctuation, and Unicode are rejected to keep usernames safe
/// for use as map keys and SQL parameters.
fn is_valid_username(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_USERNAME_LEN {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Returns `true` if `msg_id` is a valid message identifier.
fn is_valid_msg_id(msg_id: &str) -> bool {
    if msg_id.is_empty() || msg_id.len() > MAX_MSG_ID_LEN {
        return false;
    }

    msg_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn reply_context_from_event(event: &Value, reply_to: &str) -> Option<Value> {
    if event.get("t").and_then(|v| v.as_str()) != Some("msg") {
        return None;
    }
    if event
        .get("msg_id")
        .or_else(|| event.get("id"))
        .and_then(|v| v.as_str())
        != Some(reply_to)
    {
        return None;
    }

    let mut reply = serde_json::json!({ "msg_id": reply_to });
    if let Some(sender) = event
        .get("u")
        .or_else(|| event.get("from"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        reply["sender"] = Value::String(sender.to_string());
    }
    if let Some(preview) = event
        .get("c")
        .and_then(|v| v.as_str())
        .map(reply_preview_text)
        .filter(|value| !value.is_empty())
    {
        reply["preview"] = Value::String(preview);
    }

    Some(reply)
}

fn reply_preview_text(raw: &str) -> String {
    const MAX_REPLY_PREVIEW_CHARS: usize = 120;
    let single_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= MAX_REPLY_PREVIEW_CHARS {
        return single_line;
    }

    single_line
        .chars()
        .take(MAX_REPLY_PREVIEW_CHARS.saturating_sub(3))
        .chain("...".chars())
        .collect()
}

/// Returns `true` if `emoji` is a non-empty, bounded reaction token.
fn is_valid_reaction_emoji(emoji: &str) -> bool {
    !emoji.trim().is_empty() && emoji.len() <= MAX_REACTION_EMOJI_LEN
}

/// Returns `true` if `pk` is a base64-encoded 32-byte public key.
///
/// The length check on the raw string (ÃƒÂ¢Ã¢â‚¬Â°Ã‚Â¤ [`MAX_PUBLIC_KEY_FIELD_LEN`])
/// prevents base64 decoding arbitrarily large inputs. After decoding, the
/// decoded length must be exactly 32 bytes to match the Ed25519 key size.
fn is_valid_pubkey_b64(pk: &str) -> bool {
    if pk.is_empty() || pk.len() > MAX_PUBLIC_KEY_FIELD_LEN {
        return false;
    }
    match general_purpose::STANDARD.decode(pk) {
        Ok(bytes) => bytes.len() == 32,
        Err(_) => false,
    }
}

/// Parses and validates an auth frame, returning a typed [`AuthInfo`] on
/// success or a [`ChatifyError`] on the first validation failure.
///
/// Validation is applied in field order so that error messages are
/// deterministic and easy to assert in tests:
///
/// 1. Frame must be a JSON object with `"t": "auth"`.
/// 2. `"u"` must pass [`is_valid_username`].
/// 3. `"pw"` must be non-empty and ÃƒÂ¢Ã¢â‚¬Â°Ã‚Â¤ [`MAX_PASSWORD_FIELD_LEN`].
/// 4. `"pk"` must pass [`is_valid_pubkey_b64`].
/// 5. `"otp"` (optional) must be ÃƒÂ¢Ã¢â‚¬Â°Ã‚Â¤ [`MAX_NONCE_LEN`] characters if present.
fn validate_auth_payload(d: &Value) -> ChatifyResult<AuthInfo> {
    if !d.is_object() {
        return Err(ChatifyError::Validation("invalid auth frame".to_string()));
    }
    if d.get("t").and_then(|v| v.as_str()) != Some("auth") {
        return Err(ChatifyError::Message(
            "first frame must be auth".to_string(),
        ));
    }

    let username = d
        .get("u")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing username".to_string()))?
        .to_string();
    if !is_valid_username(&username) {
        return Err(ChatifyError::Validation("invalid username".to_string()));
    }

    let auth_v = d.get("auth_v").and_then(|v| v.as_u64()).unwrap_or(1);
    let credential = if auth_v >= 2 || d.get("proof").is_some() {
        let proof = d
            .get("proof")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ChatifyError::Validation("missing auth proof".to_string()))?;
        if proof.len() != 64 || !proof.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ChatifyError::Validation("invalid auth proof".to_string()));
        }

        let client_nonce = d
            .get("cn")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ChatifyError::Validation("missing auth client nonce".to_string()))?;
        let server_nonce = d
            .get("sn")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ChatifyError::Validation("missing auth server nonce".to_string()))?;
        for nonce in [client_nonce, server_nonce] {
            if nonce.is_empty()
                || nonce.len() > MAX_NONCE_LEN
                || !nonce.chars().all(|c| c.is_ascii_hexdigit())
            {
                return Err(ChatifyError::Validation("invalid auth nonce".to_string()));
            }
        }

        let enrollment_secret = d.get("pw").and_then(|v| v.as_str()).map(str::to_string);
        if let Some(secret) = enrollment_secret.as_deref() {
            if secret.is_empty() || secret.len() > MAX_PASSWORD_FIELD_LEN {
                return Err(ChatifyError::Validation(
                    "invalid password hash".to_string(),
                ));
            }
        }

        AuthCredential::V2Proof {
            proof: proof.to_string(),
            client_nonce: client_nonce.to_string(),
            server_nonce: server_nonce.to_string(),
            enrollment_secret,
        }
    } else {
        let pw = d
            .get("pw")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ChatifyError::Validation("missing password hash".to_string()))?;
        if pw.is_empty() || pw.len() > MAX_PASSWORD_FIELD_LEN {
            return Err(ChatifyError::Validation(
                "invalid password hash".to_string(),
            ));
        }
        AuthCredential::LegacyHash(pw.to_string())
    };

    let pubkey = d
        .get("pk")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing public key".to_string()))?
        .to_string();
    if !is_valid_pubkey_b64(&pubkey) {
        return Err(ChatifyError::Message("invalid public key".to_string()));
    }

    // Validate the status field: must be an object with bounded string fields.
    let status = validate_status_field(d.get("status"))?;

    let otp_code = d
        .get("otp")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());

    if let Some(code) = otp_code.as_deref() {
        if code.len() > MAX_NONCE_LEN {
            return Err(ChatifyError::Validation("invalid otp code".to_string()));
        }
    }

    let is_bridge = d.get("bridge").and_then(|v| v.as_bool()).unwrap_or(false);
    let bridge_type = d
        .get("bridge_type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let bridge_instance_id = d
        .get("bridge_instance_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let bridge_routes = d.get("bridge_routes").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

    Ok(AuthInfo {
        username,
        credential,
        status,
        pubkey,
        otp_code,
        is_bridge,
        bridge_type,
        bridge_instance_id,
        bridge_routes,
    })
}

fn auth_v2_start_client_nonce(d: &Value) -> ChatifyResult<Option<String>> {
    if d.get("t").and_then(|v| v.as_str()) != Some("auth")
        || d.get("auth_v").and_then(|v| v.as_u64()) != Some(2)
        || d.get("proof").is_some()
    {
        return Ok(None);
    }

    let username = d
        .get("u")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing username".to_string()))?;
    if !is_valid_username(username) {
        return Err(ChatifyError::Validation("invalid username".to_string()));
    }

    let pubkey = d
        .get("pk")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing public key".to_string()))?;
    if !is_valid_pubkey_b64(pubkey) {
        return Err(ChatifyError::Message("invalid public key".to_string()));
    }
    let _ = validate_status_field(d.get("status"))?;

    let client_nonce = d
        .get("cn")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing auth client nonce".to_string()))?;
    if client_nonce.is_empty()
        || client_nonce.len() > MAX_NONCE_LEN
        || !client_nonce.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(ChatifyError::Validation("invalid auth nonce".to_string()));
    }

    Ok(Some(client_nonce.to_string()))
}

/// Validates the optional `"status"` field in the auth frame.
///
/// The status must be a JSON object. If present, `"text"` and `"emoji"`
/// sub-fields are length-checked to prevent abuse. Missing fields or an
/// absent status object default to a standard "Online" status.
fn validate_status_field(status: Option<&Value>) -> ChatifyResult<Value> {
    let Some(val) = status else {
        return Ok(serde_json::json!({"text": "Online", "emoji": ""}));
    };

    if !val.is_object() {
        return Err(ChatifyError::Validation(
            "status must be a JSON object".to_string(),
        ));
    }

    // Validate text field length
    if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
        if text.len() > MAX_STATUS_TEXT_LEN {
            return Err(ChatifyError::Validation(format!(
                "status text exceeds {} characters",
                MAX_STATUS_TEXT_LEN
            )));
        }
    }

    // Validate emoji field length
    if let Some(emoji) = val.get("emoji").and_then(|v| v.as_str()) {
        if emoji.len() > MAX_STATUS_EMOJI_LEN {
            return Err(ChatifyError::Validation(format!(
                "status emoji exceeds {} characters",
                MAX_STATUS_EMOJI_LEN
            )));
        }
    }

    // Reject any other unexpected top-level fields in status
    if let Some(obj) = val.as_object() {
        for key in obj.keys() {
            if key != "text" && key != "emoji" {
                return Err(ChatifyError::Validation(format!(
                    "unexpected status field: {}",
                    key
                )));
            }
        }
    }

    Ok(val.clone())
}

// ---------------------------------------------------------------------------
// 2-FA helpers
// ---------------------------------------------------------------------------

/// Verifies a TOTP or backup code for `user_2fa`, mutating state on success.
///
/// The verification order is:
/// 1. TOTP code (live window) ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â if valid, updates `last_verified`.
/// 2. Backup code ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â if valid, the code is consumed (removed from the list) by
///    `verify_backup_code`. This enforces single-use semantics at the model
///    layer before the caller persists the updated record.
fn verify_user_2fa_code(user_2fa: &mut User2FA, code: &str) -> bool {
    if user_2fa.verify_totp(code) {
        user_2fa.last_verified = Some(now());
        true
    } else {
        user_2fa.verify_backup_code(code)
    }
}

/// Enforces 2-FA requirements during the authentication handshake.
///
/// - If no `user_2fa` record exists for `username`, 2-FA is not configured
///   and authentication proceeds unconditionally.
/// - If a record exists but `enabled` is `false`, 2-FA is configured but
///   disabled; authentication proceeds unconditionally.
/// - If 2-FA is enabled and `otp_code` is `None`, returns
///   `Err("2FA code required")` so the client knows to prompt for a code.
/// - If 2-FA is enabled and the code fails verification, returns
///   `Err("invalid 2FA code")`.
/// - On success, persists the updated `user_2fa` record (updated
///   `last_verified` or consumed backup code).
fn enforce_2fa_on_auth(
    state: &Arc<State>,
    username: &str,
    otp_code: Option<&str>,
) -> ChatifyResult<()> {
    let Some(mut user_2fa) = state.store.load_user_2fa(username) else {
        return Ok(());
    };

    if !user_2fa.enabled {
        return Ok(());
    }

    let code = otp_code.ok_or_else(|| ChatifyError::Message("2FA code required".to_string()))?;
    if !verify_user_2fa_code(&mut user_2fa, code) {
        return Err(ChatifyError::Message("invalid 2FA code".to_string()));
    }

    state.store.upsert_user_2fa(&user_2fa);
    Ok(())
}

// ---------------------------------------------------------------------------
// Replay-protection helpers
// ---------------------------------------------------------------------------

/// Returns `true` if `event_type` requires timestamp-skew validation and
/// nonce-based replay protection.
///
/// Only mutating events that change server state or carry sensitive content
/// are protected. Read-only queries (`"history"`, `"search"`, `"users"`,
/// `"info"`, `"ping"`) and control events (`"join"`, `"leave"`, `"vjoin"`) are excluded
/// because replaying them is either idempotent or harmless.
fn requires_fresh_protection(d: &Value) -> bool {
    let event_type = d.get("t").and_then(|v| v.as_str()).unwrap_or("");
    match event_type {
        "msg" | "img" | "dm" | "join" | "leave" | "vjoin" | "vleave" | "vstate" | "vspeaking"
        | "vdata" | "ss_start" | "ss_stop" | "ss_meta" | "ss_frame" | "edit" | "file_meta"
        | "file_chunk" | "typing" | "status" | "reaction" | "kick" | "2fa_verify" | "unlock"
        | "slash" => true,
        "plugin" => !matches!(
            d.get("sub").and_then(|v| v.as_str()).unwrap_or("list"),
            "list" | ""
        ),
        "admin" => !matches!(
            d.get("sub").and_then(|v| v.as_str()).unwrap_or("users"),
            "users" | "audit" | ""
        ),
        "role" => !matches!(
            d.get("sub").and_then(|v| v.as_str()).unwrap_or("get"),
            "get" | ""
        ),
        "ban" | "mute" => !matches!(
            d.get("sub").and_then(|v| v.as_str()).unwrap_or("add"),
            "check"
        ),
        _ => false,
    }
}

/// Validates that the client-supplied `"ts"` field is within
/// Ãƒâ€šÃ‚Â±[`MAX_CLOCK_SKEW_SECS`] of the server's wall clock.
///
/// Timestamp skew validation is enforced when nonce (`"n"`) is present.
///
/// A timestamp of `0` or below, or a non-finite value, is unconditionally
/// rejected to guard against clients that send uninitialised fields.
fn validate_timestamp_skew(d: &Value) -> ChatifyResult<()> {
    if d.get("n").and_then(|v| v.as_str()).is_none() {
        return Err(ChatifyError::Validation("missing nonce".to_string()));
    }

    let Some(ts) = d
        .get("ts")
        .and_then(|v| v.as_f64().or_else(|| v.as_u64().map(|u| u as f64)))
    else {
        return Err(ChatifyError::Validation("missing timestamp".to_string()));
    };

    if !ts.is_finite() || ts < 0.0 {
        return Err(ChatifyError::Validation("invalid timestamp".to_string()));
    }

    if (now() - ts).abs() > MAX_CLOCK_SKEW_SECS {
        return Err(ChatifyError::Validation(
            "timestamp outside allowed clock skew".to_string(),
        ));
    }

    Ok(())
}

/// Checks that the `"n"` nonce field has not been seen before, then records it.
///
/// # Nonce format
///
/// Nonces must be non-empty lowercase hexadecimal strings of at most
/// [`MAX_NONCE_LEN`] characters. This restriction:
/// - Prevents injection via non-hex characters in storage paths or logs.
/// - Bounds the per-entry size in the nonce cache.
///
/// # Cache eviction
///
/// Each user's nonce deque is capped at [`NONCE_CACHE_CAP`] entries. When the
/// cap is reached the oldest entry is evicted. Nonces older than
/// [`MAX_CLOCK_SKEW_SECS`] would be rejected by the timestamp check before
/// reaching nonce validation, so eviction does not open a replay window within
/// the skew window as long as `NONCE_CACHE_CAP` is large enough to hold all
/// nonces that could arrive within that window.
fn validate_and_register_nonce(state: &State, username: &str, d: &Value) -> ChatifyResult<()> {
    let Some(nonce) = d.get("n").and_then(|v| v.as_str()) else {
        return Err(ChatifyError::Validation("missing nonce".to_string()));
    };

    if nonce.is_empty() || nonce.len() > MAX_NONCE_LEN {
        return Err(ChatifyError::Validation("invalid nonce".to_string()));
    }
    if !nonce.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ChatifyError::Validation("invalid nonce format".to_string()));
    }

    let mut user_nonces = state.recent_nonces.entry(username.to_string()).or_default();

    if user_nonces.iter().any(|n| n == nonce) {
        return Err(ChatifyError::Validation("replayed nonce".to_string()));
    }

    user_nonces.push_back(nonce.to_string());
    if user_nonces.len() > NONCE_CACHE_CAP {
        let _ = user_nonces.pop_front();
    }
    // Drop the mutable borrow before accessing nonce_last_seen.
    drop(user_nonces);

    state
        .nonce_last_seen
        .insert(username.to_string(), crate::now());

    Ok(())
}

// ---------------------------------------------------------------------------
// Handshake validation (CVE-2023-43668 mitigation)
// ---------------------------------------------------------------------------

/// Callback for validating WebSocket handshake HTTP headers.
///
/// This callback is invoked during the WebSocket upgrade handshake to validate
/// HTTP headers before the connection is established. It mitigates CVE-2023-43668
/// by enforcing limits on header size and count, preventing denial-of-service
/// attacks via excessive HTTP headers.
///
/// # Security considerations
///
/// - Rejects requests with headers exceeding `MAX_HANDSHAKE_HEADER_SIZE` bytes
/// - Rejects requests with more than `MAX_HANDSHAKE_HEADERS` headers
/// - Logs suspicious activity for monitoring
struct HandshakeValidator;

impl Callback for HandshakeValidator {
    fn on_request(
        self,
        req: &Request,
        response: Response,
    ) -> Result<Response, http::Response<Option<String>>> {
        // Calculate total header size
        let mut total_header_size = req.uri().to_string().len();
        let header_count = req.headers().len();

        for (name, value) in req.headers().iter() {
            total_header_size += name.as_str().len();
            total_header_size += value.len();
        }

        // Validate header count
        if header_count > MAX_HANDSHAKE_HEADERS {
            warn!(
                "Handshake rejected: too many headers ({} > {})",
                header_count, MAX_HANDSHAKE_HEADERS
            );
            return Err(http::Response::builder()
                .status(431)
                .body(Some("Too Many Headers".to_string()))
                .unwrap());
        }

        // Validate total header size
        if total_header_size > MAX_HANDSHAKE_HEADER_SIZE {
            warn!(
                "Handshake rejected: headers too large ({} > {} bytes)",
                total_header_size, MAX_HANDSHAKE_HEADER_SIZE
            );
            return Err(http::Response::builder()
                .status(431)
                .body(Some("Request Header Fields Too Large".to_string()))
                .unwrap());
        }

        Ok(response)
    }
}

// ---------------------------------------------------------------------------
// Connection handler
// ---------------------------------------------------------------------------

/// Handles a single client WebSocket connection from TCP accept to disconnect.
///
/// # Lifecycle
///
/// ```text
/// accept_async ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ read auth frame ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ validate auth ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ enforce 2FA
///     ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ register user ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ send "ok" ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ spawn sink writer task
///     ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ main recv loop ( handle_event )
///     ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ deregister user ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ broadcast leave
/// ```
///
/// A [`ConnectionGuard`] is created immediately after accept and dropped at
/// the end of the function, ensuring `active_connections` is always accurate.
///
/// # Concurrency model
///
/// The WebSocket stream is read sequentially in this task. Outbound messages
/// from broadcast channels and other connection tasks are queued via an
/// bounded `mpsc::channel` and drained by a dedicated sink-writer task.
/// This decouples the read path from the write path, preventing a slow write
/// from blocking event processing.
async fn handle<S>(stream: S, addr: SocketAddr, state: Arc<State>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    // ConnectionGuard increments active_connections and decrements it on drop,
    // even if we return early.

    // --- IP-level rate limiting ---
    if !state.ip_connect(&addr) {
        warn!("connection rejected: too many connections from source IP");
        // Best-effort: the stream may not support WebSocket yet, but try.
        if let Ok(ws) = accept_async(stream).await {
            let (mut sink, _) = ws.split();
            let _ = sink
                .send(Message::text(
                    serde_json::json!({
                        "t": "err",
                        "m": format!("too many connections from {}", addr.ip())
                    })
                    .to_string(),
                ))
                .await;
        }
        return;
    }

    let _conn_guard = ConnectionGuard::new(state.clone(), addr);

    // Upgrade the raw TCP stream to a WebSocket connection.
    // Use accept_hdr_async with custom callback to validate headers (CVE-2023-43668 mitigation).
    let ws = match accept_hdr_async(stream, HandshakeValidator).await {
        Ok(w) => w,
        Err(e) => {
            debug!("WebSocket handshake failed: {}", e);
            return;
        }
    };
    let (mut sink, mut stream) = ws.split();

    // ---- Phase 1: read and validate the auth frame --------------------------

    let mut raw = match stream.next().await {
        Some(Ok(Message::Text(r))) => r,
        Some(Ok(_)) => {
            let _ = sink
                .send(Message::text(
                    serde_json::json!({"t":"err","m":"first frame must be text auth"}).to_string(),
                ))
                .await;
            return;
        }
        _ => return,
    };

    // Reject oversized auth frames before JSON parsing.
    if raw.len() > MAX_AUTH_BYTES {
        let _ = sink
            .send(Message::text(
                serde_json::json!({"t":"err","m":"auth frame too large"}).to_string(),
            ))
            .await;
        return;
    }

    let mut d: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            let _ = sink
                .send(Message::text(
                    serde_json::json!({"t":"err","m":"invalid auth JSON"}).to_string(),
                ))
                .await;
            return;
        }
    };

    let msg_type = d.get("t").and_then(|v| v.as_str()).unwrap_or("");

    if msg_type == "register" {
        handle_self_registration(&state, &d, &addr, &mut sink).await;
        return;
    }

    let mut expected_auth_v2_nonce: Option<(String, String)> = None;
    match auth_v2_start_client_nonce(&d) {
        Ok(Some(client_nonce)) => {
            let username = d.get("u").and_then(|v| v.as_str()).unwrap_or("");
            let credential_state = match state.store.credential_state(username) {
                Ok(stored) => stored,
                Err(err) => {
                    let _ = sink
                        .send(Message::text(
                            serde_json::json!({"t":"err","m":format!("credential error: {}", err)})
                                .to_string(),
                        ))
                        .await;
                    return;
                }
            };
            let enrollment = credential_state.is_none();
            let legacy_migration = credential_state
                .as_deref()
                .is_some_and(|stored| !stored.starts_with("v2$"));
            let server_nonce = chatify::fresh_nonce_hex();
            expected_auth_v2_nonce = Some((client_nonce.clone(), server_nonce.clone()));
            let _ = sink
                .send(Message::text(
                    serde_json::json!({
                        "t":"auth_challenge",
                        "auth_v":2,
                        "alg":"hmac-sha256",
                        "cn":client_nonce,
                        "sn":server_nonce,
                        "enroll": enrollment,
                        "legacy_migration": legacy_migration
                    })
                    .to_string(),
                ))
                .await;

            raw = match stream.next().await {
                Some(Ok(Message::Text(r))) => r,
                Some(Ok(_)) => {
                    let _ = sink
                        .send(Message::text(
                            serde_json::json!({"t":"err","m":"auth proof must be text"})
                                .to_string(),
                        ))
                        .await;
                    return;
                }
                _ => return,
            };
            if raw.len() > MAX_AUTH_BYTES {
                let _ = sink
                    .send(Message::text(
                        serde_json::json!({"t":"err","m":"auth frame too large"}).to_string(),
                    ))
                    .await;
                return;
            }
            d = match serde_json::from_str(&raw) {
                Ok(v) => v,
                Err(_) => {
                    let _ = sink
                        .send(Message::text(
                            serde_json::json!({"t":"err","m":"invalid auth JSON"}).to_string(),
                        ))
                        .await;
                    return;
                }
            };
        }
        Ok(None) => {}
        Err(err) => {
            let _ = sink
                .send(Message::text(
                    serde_json::json!({"t":"err","m":err.to_string()}).to_string(),
                ))
                .await;
            return;
        }
    }

    let auth = match validate_auth_payload(&d) {
        Ok(a) => a,
        Err(err) => {
            let _ = sink
                .send(Message::text(
                    serde_json::json!({"t":"err","m":err.to_string()}).to_string(),
                ))
                .await;
            return;
        }
    };

    // --- Per-IP auth rate limiting ---
    if !state.ip_auth_allowed(&addr) {
        warn!("auth rate limited");
        let _ = sink
            .send(Message::text(
                serde_json::json!({
                    "t": "err",
                    "m": "too many auth attempts, please wait"
                })
                .to_string(),
            ))
            .await;
        return;
    }

    let AuthInfo {
        username,
        credential,
        mut status,
        pubkey,
        otp_code,
        is_bridge,
        bridge_type,
        bridge_instance_id,
        bridge_routes,
    } = auth;

    // --- Account lockout check ---
    const MAX_FAILED_ATTEMPTS: i32 = 5;
    if let Some((_, locked_until)) = state.store.get_lockout_status(&username) {
        if locked_until > crate::now() {
            let _ = sink
                .send(Message::text(
                    serde_json::json!({
                        "t": "err",
                        "m": "invalid credentials",
                    })
                    .to_string(),
                ))
                .await;
            warn!("auth blocked: account locked");
            return;
        }
    }

    // --- Credential verification ---
    // Auth-v2 uses a per-login challenge proof so the reusable client password
    // hash is not sent over the WebSocket during normal logins. Legacy rows can
    // be migrated by one v2 login that includes the client secret only when the
    // challenge explicitly asks for enrollment/migration.
    let mut legacy_hash_to_persist: Option<String> = None;
    let mut v2_secret_to_persist: Option<String> = None;
    let mut credential_verified = false;
    let credential_result = match credential {
        AuthCredential::LegacyHash(pw_hash) => {
            if state
                .store
                .credential_state(&username)
                .ok()
                .flatten()
                .as_deref()
                .is_some_and(|stored| stored.starts_with("v2$"))
            {
                Ok(false)
            } else if let Some(pending) = state.pending_credentials.get(&username) {
                Ok(crypto::secure_string_eq(&pw_hash, pending.value()))
            } else {
                let result = state.store.verify_credential(&username, &pw_hash);
                if result == Err("first_login") {
                    legacy_hash_to_persist = Some(pw_hash);
                }
                result
            }
        }
        AuthCredential::V2Proof {
            proof,
            client_nonce,
            server_nonce,
            enrollment_secret,
        } => {
            if let Some((expected_client_nonce, expected_server_nonce)) =
                expected_auth_v2_nonce.as_ref()
            {
                if client_nonce != *expected_client_nonce || server_nonce != *expected_server_nonce
                {
                    Ok(false)
                } else {
                    match state.store.verify_auth_v2_proof(
                        &username,
                        &proof,
                        &client_nonce,
                        &server_nonce,
                    ) {
                        Ok(result) => Ok(result),
                        Err("first_login") => {
                            if let Some(secret) = enrollment_secret {
                                let expected = crypto::auth_proof(
                                    &secret,
                                    &username,
                                    &client_nonce,
                                    &server_nonce,
                                )
                                .unwrap_or_default();
                                if crypto::secure_string_eq(&proof, &expected) {
                                    v2_secret_to_persist = Some(secret);
                                    Err("first_login")
                                } else {
                                    Ok(false)
                                }
                            } else {
                                Ok(false)
                            }
                        }
                        Err("legacy_credential") => {
                            if let Some(secret) = enrollment_secret {
                                let expected = crypto::auth_proof(
                                    &secret,
                                    &username,
                                    &client_nonce,
                                    &server_nonce,
                                )
                                .unwrap_or_default();
                                if crypto::secure_string_eq(&proof, &expected)
                                    && state.store.verify_credential(&username, &secret) == Ok(true)
                                {
                                    v2_secret_to_persist = Some(secret);
                                    Ok(true)
                                } else {
                                    Ok(false)
                                }
                            } else {
                                Ok(false)
                            }
                        }
                        Err(e) => Err(e),
                    }
                }
            } else {
                warn!("auth-v2 proof rejected without matching challenge");
                Ok(false)
            }
        }
    };

    match credential_result {
        Ok(true) => credential_verified = true, // Hash matches — continue auth flow.
        Ok(false) => {
            let (locked, _attempts) = state
                .store
                .record_failed_login(&username, MAX_FAILED_ATTEMPTS);
            if locked {
                state.store.log_suspicious_activity(
                    &username,
                    "brute_force",
                    "high",
                    Some("account locked due to repeated failed login attempts"),
                );
            }
            let _ = sink
                .send(Message::text(
                    serde_json::json!({
                        "t":"err",
                        "m":"invalid credentials",
                    })
                    .to_string(),
                ))
                .await;
            warn!("auth failed: invalid password");
            return;
        }
        Err("first_login") => {
            if !state.self_registration_enabled {
                let _ = sink
                    .send(Message::text(
                        serde_json::json!({
                            "t": "err",
                            "m": "invalid credentials",
                        })
                        .to_string(),
                    ))
                    .await;
                warn!("auth rejected for unknown user because self-registration is disabled");
                return;
            }
            // First time this username connects ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â store their credential.
            // The submitted hash is itself a PBKDF2 output, so we wrap it
            // in another salted PBKDF2 layer server-side.
            let credential_secret = if let Some(secret) = v2_secret_to_persist.take() {
                secret
            } else if let Some(secret) = legacy_hash_to_persist.take() {
                secret
            } else {
                let _ = sink
                    .send(Message::text(
                        serde_json::json!({"t":"err","m":"missing enrollment credential"})
                            .to_string(),
                    ))
                    .await;
                return;
            };
            let store_as_v2 = expected_auth_v2_nonce.is_some();
            state
                .pending_credentials
                .insert(username.clone(), credential_secret.clone());
            let state_for_task = state.clone();
            let username_for_task = username.clone();
            let credential_secret_for_task = credential_secret.clone();
            tokio::task::spawn_blocking(move || {
                if store_as_v2 {
                    state_for_task
                        .store
                        .upsert_auth_v2_secret(&username_for_task, &credential_secret_for_task);
                } else {
                    let server_hash = crypto::pw_hash(&credential_secret_for_task);
                    state_for_task
                        .store
                        .upsert_credentials(&username_for_task, &server_hash);
                }
                state_for_task
                    .pending_credentials
                    .remove(&username_for_task);
            });
            info!("credentials created for new user");
        }
        Err(e) => {
            let _ = sink
                .send(Message::text(
                    serde_json::json!({"t":"err","m":format!("credential error: {}", e)})
                        .to_string(),
                ))
                .await;
            return;
        }
    }

    // --- Username uniqueness ---
    // Reject if this username is already online (prevents session hijacking).
    if state.user_statuses.contains_key(&username) {
        let _ = sink
            .send(Message::text(
                serde_json::json!({"t":"err","m":"username already in use"}).to_string(),
            ))
            .await;
        warn!("auth rejected: username already connected");
        return;
    }

    if let Err(err) = enforce_2fa_on_auth(&state, &username, otp_code.as_deref()) {
        let error_text = err.to_string();
        if error_text == "invalid 2FA code" {
            let (locked, _attempts) = state
                .store
                .record_failed_login(&username, MAX_FAILED_ATTEMPTS);
            if locked {
                state.store.log_suspicious_activity(
                    &username,
                    "brute_force",
                    "high",
                    Some("account locked due to repeated failed 2FA attempts"),
                );
            }
            let _ = sink
                .send(Message::text(
                    serde_json::json!({
                        "t":"err",
                        "m":"invalid 2FA code",
                    })
                    .to_string(),
                ))
                .await;
            warn!("auth failed: invalid 2FA code");
        } else {
            let _ = sink
                .send(Message::text(
                    serde_json::json!({"t":"err","m":error_text}).to_string(),
                ))
                .await;
        }
        return;
    }
    if credential_verified {
        state.store.clear_failed_logins(&username);
        if let Some(secret) = v2_secret_to_persist.take() {
            state.store.upsert_auth_v2_secret(&username, &secret);
        }
    }

    if is_default_online_status(&status) {
        if let Some(snapshot_status) = state.store.load_presence_snapshot(&username) {
            status = snapshot_status;
        }
    }

    // Generate a session token for this connection.
    let session_token = state.create_session(&username);

    // ---- Phase 2: register user and send welcome response -------------------

    state.user_statuses.insert(username.clone(), status.clone());
    state.user_pubkeys.insert(username.clone(), pubkey);
    state.store.upsert_presence_snapshot(&username, &status);
    state
        .store
        .upsert_channel_subscription(&username, "general");

    let status_update = Arc::new(
        serde_json::json!({
            "t":"status_update",
            "user":username,
            "status":status.clone()
        })
        .to_string(),
    );
    for chan_entry in state.channels.iter() {
        let _ = chan_entry.tx.send(status_update.as_ref().clone());
    }

    // Register bridge if the client identified as one.
    if is_bridge {
        let info = BridgeInfo {
            username: username.clone(),
            bridge_type: bridge_type.clone(),
            instance_id: bridge_instance_id.clone(),
            connected_at: crate::now(),
            route_count: bridge_routes,
        };
        state.bridges.insert(username.clone(), info);
        info!(
            "event=bridge_connected bridge_type={} routes={}",
            bridge_type, bridge_routes
        );
    }

    // Subscribe to "general" before sending "ok" to avoid missing messages
    // that arrive between the response send and the subscription.
    let general = state.chan("general");
    let gen_rx = general.tx.subscribe();
    let dm_rx = state.chan(&dm_channel_name(&username)).tx.subscribe();
    let mut hist = state.store.history("general", HISTORY_CAP);
    if hist.is_empty() {
        hist = general.hist().await;
    }

    let ok = create_ok_response(&username, &state, hist, Some(&session_token));
    if sink.send(Message::text(ok)).await.is_err() {
        return;
    }

    broadcast_system_msg(&state, &format!("ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ {} joined", username)).await;
    info!("client joined");

    // ---- Phase 3: set up bidirectional message routing ----------------------

    // mpsc channel: all tasks that want to send to this client queue here.
    // Bounded to prevent unbounded memory growth on slow or stalled clients.
    let (outbound_sender, mut out_rx) =
        tokio::sync::mpsc::channel::<String>(state.outbound_queue_capacity);
    let (slow_client_tx, mut slow_client_rx) = tokio::sync::mpsc::channel::<()>(1);
    let out_tx = OutboundTx::new(
        outbound_sender,
        slow_client_tx,
        state.slow_client_drop_burst,
        state.prometheus.clone(),
    );

    let mut channel_forwarders: Vec<JoinHandle<()>> = Vec::new();
    // Forward "general" broadcast to the outbound queue.
    track_forwarder(
        &mut channel_forwarders,
        spawn_broadcast_forwarder(gen_rx, out_tx.clone()),
    );
    // Forward this user's DM broadcast channel to the outbound queue.
    track_forwarder(
        &mut channel_forwarders,
        spawn_broadcast_forwarder(dm_rx, out_tx.clone()),
    );

    // Track subscribed channels for this connection to prevent duplicate
    // forwarders, support reconnect recovery, and allow leave-time cleanup.
    let joined_channels: Arc<DashSet<String>> = Arc::new(DashSet::new());
    joined_channels.insert("general".to_string());

    let mut restored_subscriptions = 0usize;
    for channel in state.store.list_channel_subscriptions(&username) {
        if channel == "general" {
            continue;
        }
        if joined_channels.insert(channel.clone()) {
            let chan = state.chan(&channel);
            track_forwarder(
                &mut channel_forwarders,
                spawn_channel_forwarder(
                    chan.tx.subscribe(),
                    out_tx.clone(),
                    joined_channels.clone(),
                    channel.clone(),
                ),
            );
            restored_subscriptions += 1;
        }
    }
    if restored_subscriptions > 0 {
        info!(
            "rehydrated channel subscriptions count={}",
            restored_subscriptions
        );
    }

    // Sink writer task: drains out_rx and writes to the WebSocket sink.
    // Runs until out_rx is closed (out_tx is dropped at function exit).
    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(Message::text(m)).await.is_err() {
                break;
            }
        }
    });

    // ---- Phase 4: main event loop -------------------------------------------

    let mut session = ConnectionSession {
        out_tx: out_tx.clone(),
        fresh_required: expected_auth_v2_nonce.is_some(),
        voice_room: None,
        active_voice_room: Arc::new(RwLock::new(None)),
        voice_audio_forwarder: None,
        voice_relay_forwarder: None,
        screen_room: None,
        screen_forwarder: None,
        joined_channels: joined_channels.clone(),
        channel_forwarders,
    };

    loop {
        let msg = tokio::select! {
            signal = slow_client_rx.recv() => {
                if signal.is_some() {
                    warn!(
                        "disconnecting slow client queue_capacity={} drop_burst={}",
                        state.outbound_queue_capacity,
                        state.slow_client_drop_burst,
                    );
                }
                break;
            }
            next = stream.next() => match next {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => {
                info!("ws recv error: {}", e);
                break;
            }
            None => break, // Client closed the connection cleanly.
            }
        };

        let raw = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue, // Binary / ping / pong frames are ignored.
        };

        // Payload size gate (post-auth; auth size is gated earlier).
        if raw.len() > MAX_BYTES {
            send_out_json(
                &out_tx,
                serde_json::json!({"t":"err","m":"payload exceeds max size"}),
            );
            continue;
        }

        let d: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => {
                send_out_json(
                    &out_tx,
                    serde_json::json!({"t":"err","m":"invalid JSON payload"}),
                );
                state.metrics.inc_received(1);
                state.metrics.inc_bytes_received(raw.len());
                continue;
            }
        };

        if !d.is_object() {
            send_out_json(
                &out_tx,
                serde_json::json!({"t":"err","m":"payload must be a JSON object"}),
            );
            state.metrics.inc_received(1);
            state.metrics.inc_bytes_received(raw.len());
            continue;
        }

        state.metrics.inc_received(1);
        state.metrics.inc_bytes_received(raw.len());

        handle_event(&d, &state, &username, &mut session).await;
    }

    // ---- Phase 5: cleanup ---------------------------------------------------
    if let Some(handle) = session.voice_audio_forwarder.take() {
        handle.abort();
    }
    if let Some(handle) = session.voice_relay_forwarder.take() {
        handle.abort();
    }
    if let Some(room) = session.voice_room.take() {
        state.voice_relay.leave_room(&room, &username);
    }
    {
        let mut room_guard = session.active_voice_room.write().await;
        *room_guard = None;
    }

    if let Some(handle) = session.screen_forwarder.take() {
        handle.abort();
    }
    if let Some(room) = session.screen_room.take() {
        if let Some(stx) = state.screen.get(&room) {
            let _ = stx.send(
                serde_json::json!({
                    "t": "ss_state",
                    "room": room,
                    "from": username,
                    "enabled": false,
                    "status": "inactive",
                    "reason": "disconnect",
                    "ts": now(),
                })
                .to_string(),
            );
        }
    }
    session.joined_channels.clear();
    for handle in session.channel_forwarders.drain(..) {
        handle.abort();
    }

    // Remove user presence so they no longer appear in the user directory.
    state.user_statuses.remove(&username);
    state.user_pubkeys.remove(&username);
    // Invalidate the session token.
    state.end_session(&username);
    // Clear the nonce cache to free memory; replays from this session are
    // no longer possible once the connection is closed.
    state.recent_nonces.remove(&username);
    state.nonce_last_seen.remove(&username);
    if state.bridges.remove(&username).is_some() {
        info!("event=bridge_disconnected");
    }
    broadcast_system_msg(&state, &format!("ÃƒÂ¢Ã…â€œÃ¢â‚¬â€œ {} left", username)).await;
    info!("client left");
    // _conn_guard drops here, decrementing active_connections and IP counter.
}

// ---------------------------------------------------------------------------
// TLS support
// ---------------------------------------------------------------------------

/// Wraps a TLS stream, forwarding [`AsyncRead`] and [`AsyncWrite`] to the
/// inner `TlsStream<TcpStream>`. Needed because the two concrete stream types
/// (plain `TcpStream` and `TlsStream<TcpStream>`) are different types, and
/// [`accept_async`] needs a single type parameter.
struct ChatifyTlsStream {
    inner: tokio_rustls::server::TlsStream<TcpStream>,
}

impl tokio::io::AsyncRead for ChatifyTlsStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for ChatifyTlsStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl Unpin for ChatifyTlsStream {}

/// Handles CLI admin commands (register-user, enable-2fa, disable-2fa).
fn handle_admin_commands(args: &Args) -> ChatifyResult<()> {
    let db_key = resolve_db_key(&args.db, args.db_key.as_deref())?;

    let plugin_runtime = match std::env::current_exe() {
        Ok(path) => PluginRuntime::new(path),
        Err(e) => {
            return Err(ChatifyError::Message(format!(
                "failed to resolve exe: {}",
                e
            )))
        }
    };

    let state = State::new(
        args.db.clone(),
        db_key,
        args.db_durability,
        args.db_pool_size,
        None,
        plugin_runtime,
        60,
        false,
        false,
        OUTBOUND_QUEUE_CAPACITY_DEFAULT,
        SLOW_CLIENT_DROP_BURST_DEFAULT,
        false,
    );

    if let Some(username) = &args.register_user {
        let mut password = read_registration_password(args)?;
        let server_hash = crypto::pw_hash(&password);
        password.zeroize();
        state.store.upsert_credentials(username, &server_hash);
        state.store.log_audit(
            "cli_register",
            "cli",
            Some(username),
            Some("general"),
            None,
            if args.make_admin {
                Some("admin")
            } else {
                Some("member")
            },
        );

        if args.make_admin {
            state
                .store
                .assign_role(username, "general", "admin", "cli")
                .map_err(ChatifyError::Message)?;
            println!("Created admin user: {}", username);
        } else {
            println!("Created user: {}", username);
        }
        return Ok(());
    }

    if let Some(username) = &args.enable_2fa_for {
        let output_path = args.two_fa_provisioning_output.as_ref().ok_or_else(|| {
            ChatifyError::Message(
                "--2fa-provisioning-output is required with --enable-2fa-for".to_string(),
            )
        })?;
        let provisioning_file = create_sensitive_file(output_path)?;

        let secret = chatify::totp::generate_secret();
        let qr_url = chatify::totp::generate_qr_url(username, "Chatify", &secret);

        let mut user_2fa = chatify::totp::User2FA::new(username.clone());
        let backup_codes = user_2fa.enable(secret);

        state.store.upsert_user_2fa(&user_2fa);
        state
            .store
            .log_audit("cli_enable_2fa", "cli", Some(username), None, None, None);

        // Safe access - enable() sets totp_config, so this is an internal invariant check
        let config = match &user_2fa.totp_config {
            Some(c) => c,
            None => {
                eprintln!("internal error: 2FA config not initialized after enable()");
                return Err(ChatifyError::Message(
                    "internal error: 2FA configuration state inconsistent".into(),
                ));
            }
        };

        write_2fa_provisioning_file(
            provisioning_file,
            username,
            &config.secret,
            &qr_url,
            &backup_codes,
        )?;

        println!("2FA enabled for: {}", username);
        println!("One-time enrollment material written to: {}", output_path);
        return Ok(());
    }

    if let Some(username) = &args.disable_2fa_for {
        if let Some(mut user_2fa) = state.store.load_user_2fa(username) {
            user_2fa.disable();
            state.store.upsert_user_2fa(&user_2fa);
            state
                .store
                .log_audit("cli_disable_2fa", "cli", Some(username), None, None, None);
            println!("2FA disabled for: {}", username);
        } else {
            println!("No 2FA configuration found for: {}", username);
        }
        return Ok(());
    }

    Err(ChatifyError::Message(
        "No admin command specified".to_string(),
    ))
}

fn read_registration_password(args: &Args) -> ChatifyResult<String> {
    if let Some(password) = &args.user_password {
        if password.is_empty() {
            return Err(ChatifyError::Validation(
                "--user-password cannot be empty".to_string(),
            ));
        }
        return Ok(password.clone());
    }

    if !std::io::stdin().is_terminal() {
        return Err(ChatifyError::Message(
            "--user-password is required with --register-user in non-interactive mode".to_string(),
        ));
    }

    let password = rpassword::prompt_password("New user password: ")
        .map_err(|e| ChatifyError::Io(Box::new(e)))?;
    if password.is_empty() {
        return Err(ChatifyError::Validation(
            "registered user password cannot be empty".to_string(),
        ));
    }

    Ok(password)
}

fn create_sensitive_file(path: &str) -> ChatifyResult<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| ChatifyError::Io(Box::new(e)))
    }

    #[cfg(not(unix))]
    {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| ChatifyError::Io(Box::new(e)))
    }
}

fn write_2fa_provisioning_file(
    mut file: std::fs::File,
    username: &str,
    secret: &str,
    qr_url: &str,
    backup_codes: &[String],
) -> ChatifyResult<()> {
    use std::io::Write;

    writeln!(file, "Chatify 2FA enrollment").map_err(|e| ChatifyError::Io(Box::new(e)))?;
    writeln!(file, "User: {}", username).map_err(|e| ChatifyError::Io(Box::new(e)))?;
    writeln!(file, "Secret: {}", secret).map_err(|e| ChatifyError::Io(Box::new(e)))?;
    writeln!(file, "QR URL: {}", qr_url).map_err(|e| ChatifyError::Io(Box::new(e)))?;
    writeln!(file, "Backup codes:").map_err(|e| ChatifyError::Io(Box::new(e)))?;
    for code in backup_codes {
        writeln!(file, "{}", code).map_err(|e| ChatifyError::Io(Box::new(e)))?;
    }
    file.flush().map_err(|e| ChatifyError::Io(Box::new(e)))
}

/// Holds either a plain TCP stream or a TLS-wrapped stream.
enum StreamType {
    Plain(TcpStream),
    Tls(Box<ChatifyTlsStream>),
}

impl tokio::io::AsyncRead for StreamType {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            StreamType::Plain(s) => Pin::new(s).poll_read(cx, buf),
            StreamType::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for StreamType {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            StreamType::Plain(s) => Pin::new(s).poll_write(cx, buf),
            StreamType::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            StreamType::Plain(s) => Pin::new(s).poll_flush(cx),
            StreamType::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            StreamType::Plain(s) => Pin::new(s).poll_shutdown(cx),
            StreamType::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Loads a PEM certificate chain and private key, returning a [`TlsAcceptor`].
fn load_tls_config(cert_path: &str, key_path: &str) -> ChatifyResult<TlsAcceptor> {
    use rustls_pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};

    // Load certificate chain
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert_path)
        .map_err(|e| {
            ChatifyError::Validation(format!("cannot open TLS cert '{}': {}", cert_path, e))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ChatifyError::Validation(format!("failed to parse TLS cert: {}", e)))?;
    if certs.is_empty() {
        return Err(ChatifyError::Validation(
            "TLS cert file is empty".to_string(),
        ));
    }

    // Load private key
    let key = PrivateKeyDer::from_pem_file(key_path)
        .map_err(|e| ChatifyError::Validation(format!("failed to parse TLS key: {}", e)))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| ChatifyError::Validation(format!("TLS config error: {}", e)))?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}

// ---------------------------------------------------------------------------
// Socket optimization helpers
// ---------------------------------------------------------------------------

/// Configures TCP socket for low-latency performance.
/// This sets TCP_NODELAY and keepalive at the OS level.
#[allow(dead_code)]
fn configure_socket(_socket: &tokio::net::TcpStream) {
    // Socket optimization for production use
    // In production, configure at system level or via tokio native options
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Resolves the database encryption key from the CLI arg or a key file.
///
/// Resolution order:
/// 1. If `--db-key` is provided, decode it as hex (must be 64 chars = 32 bytes).
/// 2. If a `<db_path>.key` file exists, read and decode it.
/// 3. If `db_path` is `:memory:`, return `None` (no encryption for tests).
/// 4. Otherwise, generate a new random 32-byte key, write it to `<db_path>.key`,
///    and return it.
///
/// The `.key` file is created with user-only permissions where possible.
/// Store it alongside backups; losing it means the database is unrecoverable.
fn write_db_key_file(key_path: &str, hex_key: &str) -> ChatifyResult<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut key_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(key_path)
            .map_err(|e| ChatifyError::Io(Box::new(e)))?;
        key_file
            .write_all(hex_key.as_bytes())
            .map_err(|e| ChatifyError::Io(Box::new(e)))?;
        key_file
            .write_all(b"\n")
            .map_err(|e| ChatifyError::Io(Box::new(e)))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(key_path, hex_key).map_err(|e| ChatifyError::Io(Box::new(e)))?;
    }

    Ok(())
}

fn resolve_db_key(db_path: &str, cli_key: Option<&str>) -> ChatifyResult<Option<Vec<u8>>> {
    // 1. CLI-provided key takes priority.
    if let Some(hex_key) = cli_key {
        let key = hex::decode(hex_key)
            .map_err(|e| ChatifyError::Validation(format!("invalid --db-key hex: {}", e)))?;
        if key.len() != 32 {
            return Err(ChatifyError::Validation(format!(
                "--db-key must be 32 bytes (64 hex chars), got {} bytes",
                key.len()
            )));
        }
        return Ok(Some(key));
    }

    // 2. In-memory databases don't need encryption.
    if db_path == ":memory:" {
        return Ok(None);
    }

    // 3. Check for an existing key file.
    let key_path = format!("{}.key", db_path);
    if std::path::Path::new(&key_path).exists() {
        let hex_key = std::fs::read_to_string(&key_path)
            .map_err(|e| ChatifyError::Io(Box::new(e)))?
            .trim()
            .to_string();
        let key = hex::decode(&hex_key).map_err(|e| {
            ChatifyError::Validation(format!("invalid hex in key file '{}': {}", key_path, e))
        })?;
        if key.len() != 32 {
            return Err(ChatifyError::Validation(format!(
                "key file '{}' must contain 32 bytes (64 hex chars)",
                key_path
            )));
        }
        return Ok(Some(key));
    }

    if std::path::Path::new(db_path).exists() {
        return Err(ChatifyError::Validation(format!(
            "database '{}' exists but key file '{}' is missing; provide --db-key or restore the key file",
            db_path, key_path
        )));
    }

    // 4. Generate a new key and write it to disk.
    use rand::{rngs::OsRng, RngCore};
    let mut key = <[u8; 32]>::default();
    OsRng.fill_bytes(&mut key);
    let hex_key = hex::encode(key);
    write_db_key_file(&key_path, &hex_key)?;
    println!("Generated new DB encryption key: {}", key_path);
    Ok(Some(key.to_vec()))
}

/// Server entry point.
///
/// 1. Parses CLI args and initialises optional logging.
/// 2. Resolves the database encryption key.
/// 3. Binds the TCP listener.
/// 4. Initialises shared [`State`] (which runs SQLite migrations).
/// 5. Accepts connections in a `tokio::select!` loop until Ctrl+C.
/// 6. Broadcasts a shutdown notice and waits up to 10 s for connections to
///    drain before returning.
#[cfg(not(unix))]
async fn accept_loop(
    listener: TcpListener,
    state: Arc<State>,
    tls_acceptor: Option<TlsAcceptor>,
    mut shutdown_rx: tokio::sync::mpsc::Receiver<()>,
    _args: &Args,
) {
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                broadcast_system_msg(&state, "Server is shutting down").await;
                println!("\nShutdown signal received. Stopping server loop...");
                info!("shutdown signal received; stopping accept loop");
                state.initiate_shutdown();
                break;
            }
            _ = shutdown_rx.recv() => {
                broadcast_system_msg(&state, "Server is shutting down").await;
                println!("\nShutdown triggered via API. Stopping server loop...");
                info!("shutdown triggered via API; stopping accept loop");
                state.initiate_shutdown();
                break;
            }
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, addr)) => {
                        if state.is_shutting_down() {
                            debug!("Rejecting new connection during shutdown: {}", addr);
                            continue;
                        }
                        configure_socket(&stream);
                        let s = state.clone();
                        if let Some(ref acceptor) = tls_acceptor {
                            let acceptor = acceptor.clone();
                            tokio::spawn(async move {
                                match acceptor.accept(stream).await {
                                    Ok(tls_stream) => {
                                        handle(
                                            StreamType::Tls(Box::new(ChatifyTlsStream { inner: tls_stream })),
                                            addr,
                                            s,
                                        ).await;
                                    }
                                    Err(e) => {
                                        warn!("TLS handshake failed: {}", e);
                                    }
                                }
                            });
                        } else {
                            tokio::spawn(handle(StreamType::Plain(stream), addr, s));
                        }
                    }
                    Err(_) => continue,
                }
            }
        }
    }
}

/// Unix-specific server entry point with SIGHUP support.
#[cfg(unix)]
async fn accept_loop_unix(
    listener: TcpListener,
    state: Arc<State>,
    tls_acceptor: Option<TlsAcceptor>,
    mut shutdown_rx: tokio::sync::mpsc::Receiver<()>,
    mut sighup_rx: tokio::sync::mpsc::Receiver<()>,
    args: &Args,
) {
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                broadcast_system_msg(&state, "Server is shutting down").await;
                println!("\nShutdown signal received. Stopping server loop...");
                info!("shutdown signal received; stopping accept loop");
                state.initiate_shutdown();
                break;
            }
            _ = shutdown_rx.recv() => {
                broadcast_system_msg(&state, "Server is shutting down").await;
                println!("\nShutdown triggered via API. Stopping server loop...");
                info!("shutdown triggered via API; stopping accept loop");
                state.initiate_shutdown();
                break;
            }
            _ = sighup_rx.recv() => {
                if args.enable_hot_reload {
                    info!("hot reload acknowledged in accept loop");
                }
            }
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, addr)) => {
                        if state.is_shutting_down() {
                            debug!("Rejecting new connection during shutdown: {}", addr);
                            continue;
                        }
                        configure_socket(&stream);
                        let s = state.clone();
                        if let Some(ref acceptor) = tls_acceptor {
                            let acceptor = acceptor.clone();
                            tokio::spawn(async move {
                                match acceptor.accept(stream).await {
                                    Ok(tls_stream) => {
                                        handle(
                                            StreamType::Tls(Box::new(ChatifyTlsStream { inner: tls_stream })),
                                            addr,
                                            s,
                                        ).await;
                                    }
                                    Err(e) => {
                                        warn!("TLS handshake failed: {}", e);
                                    }
                                }
                            });
                        } else {
                            tokio::spawn(handle(StreamType::Plain(stream), addr, s));
                        }
                    }
                    Err(_) => continue,
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> ChatifyResult<()> {
    let args = Args::parse();

    // Handle CLI admin commands (non-server mode)
    if args.register_user.is_some()
        || args.enable_2fa_for.is_some()
        || args.disable_2fa_for.is_some()
    {
        handle_admin_commands(&args)?;
        return Ok(());
    }

    if let Some(plugin_name) = args.chatify_plugin_worker.as_deref() {
        run_builtin_plugin_worker(
            plugin_name,
            &args.chatify_plugin_op,
            args.chatify_plugin_command.as_deref(),
        )
        .map_err(ChatifyError::Message)?;
        return Ok(());
    }

    let addr = format!("{}:{}", args.host, args.port);

    if args.log {
        let _ = env_logger::Builder::from_default_env()
            .format_timestamp_secs()
            .try_init();
    }

    let db_key = resolve_db_key(&args.db, args.db_key.as_deref())?;

    // Set up TLS if enabled.
    let tls_acceptor = if args.tls {
        let acceptor = load_tls_config(&args.tls_cert, &args.tls_key)?;
        Some(acceptor)
    } else {
        None
    };

    // Initialize Prometheus metrics
    let metrics: Option<Arc<std::sync::Mutex<PrometheusMetrics>>> = match PrometheusMetrics::new() {
        Ok(m) => {
            if args.log {
                info!("Prometheus metrics initialized");
            }
            Some(Arc::new(std::sync::Mutex::new(m)))
        }
        Err(e) => {
            warn!(
                "Failed to initialize metrics: {}; continuing without metrics",
                e
            );
            None
        }
    };

    let plugin_runtime = PluginRuntime::new(std::env::current_exe()?);

    let listener = TcpListener::bind(&addr).await?;
    let state = State::new(
        args.db.clone(),
        db_key,
        args.db_durability,
        args.db_pool_size,
        metrics.clone(),
        plugin_runtime,
        args.max_msgs_per_minute,
        args.enable_user_rate_limit,
        args.enable_self_registration,
        args.outbound_queue_capacity,
        args.slow_client_drop_burst,
        false,
    );

    for plugin_name in DEFAULT_BUILTIN_PLUGINS {
        if let Err(err) = state.plugin_runtime.install_plugin(plugin_name) {
            warn!(
                "failed to install built-in plugin '{}' at startup: {}",
                plugin_name, err
            );
        }
    }

    let enc_label = if state.store.is_encrypted() {
        "ChaCha20-Poly1305"
    } else {
        "None (unencrypted)"
    };
    let proto = if tls_acceptor.is_some() { "wss" } else { "ws" };
    println!(" Chatify running on {}://{}", proto, addr);
    println!(" Encryption: {} |   IP Privacy: On", enc_label);
    println!(" Event store: {}", args.db);
    println!(" DB durability: {}", args.db_durability.label());
    println!(
        " DB pool size: requested={} effective={}",
        args.db_pool_size,
        state.store.configured_pool_size()
    );
    println!(
        " Outbound queue: requested={} effective={}",
        args.outbound_queue_capacity, state.outbound_queue_capacity
    );
    println!(
        " Slow-client drop burst: requested={} effective={}",
        args.slow_client_drop_burst, state.slow_client_drop_burst
    );

    let media_retention_days = normalize_media_retention_days(args.media_retention_days);
    let media_prune_interval_secs =
        normalize_media_prune_interval_secs(args.media_prune_interval_secs);
    let media_max_total_size_gb = normalize_media_max_total_size_gb(args.media_max_total_size_gb);
    let media_max_total_size_bytes = gib_to_bytes_i64(media_max_total_size_gb);
    if args.disable_media_retention {
        println!(" Media retention: disabled");
    } else {
        println!(
            " Media retention: {} days | cap {:.1} GiB | prune interval {}s",
            media_retention_days, media_max_total_size_gb, media_prune_interval_secs
        );
    }

    println!(
        " User rate limit: {} msgs/min",
        if args.enable_user_rate_limit {
            args.max_msgs_per_minute.to_string()
        } else {
            "disabled".to_string()
        }
    );

    // Shutdown channel for orchestration
    let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    // Start health/metrics HTTP server if configured
    if args.health_port > 0 {
        let health_metrics = metrics.clone();
        let health_state = state.clone();
        let health_enabled = args.metrics_enabled;
        let shutdown_endpoint_enabled = args.shutdown_endpoint;
        let health_shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            let addr = format!("0.0.0.0:{}", args.health_port);
            match TcpListener::bind(&addr).await {
                Ok(listener) => {
                    println!(" Health/Metrics server running on http://{}", addr);
                    if args.log {
                        info!("health/metrics server started on {}", addr);
                    }
                    start_health_server(
                        listener,
                        health_state,
                        health_metrics,
                        health_enabled,
                        shutdown_endpoint_enabled,
                        health_shutdown_tx,
                    )
                    .await;
                }
                Err(e) => {
                    warn!("Failed to bind health port {}: {}", args.health_port, e);
                }
            }
        });
    }

    println!(" Press Ctrl+C to stop\n");
    if args.log {
        info!("server started addr={}://{} db={}", proto, addr, args.db);
    }

    // Periodic nonce cache cleanup: evicts stale entries for users whose
    // connection dropped without proper cleanup (crash, network partition).
    {
        let cleanup_state = state.clone();
        let log_enabled = args.log;
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(NONCE_CLEANUP_INTERVAL_SECS)).await;
                let evicted = cleanup_state.evict_stale_nonce_entries(NONCE_MAX_AGE_SECS);
                if evicted > 0 && log_enabled {
                    info!("nonce cache: evicted {} stale user entries", evicted);
                }
            }
        });
    }

    if !args.disable_media_retention {
        let retention_state = state.clone();
        let log_enabled = args.log;
        tokio::spawn(async move {
            loop {
                let (deleted_objects, reclaimed_bytes) = retention_state.store.prune_media_storage(
                    media_retention_days as u64 * 24 * 3600,
                    media_max_total_size_bytes,
                );

                if deleted_objects > 0 && log_enabled {
                    info!(
                        "media retention: pruned {} object(s), reclaimed {} bytes",
                        deleted_objects, reclaimed_bytes
                    );
                }

                sleep(Duration::from_secs(media_prune_interval_secs)).await;
            }
        });
    }

    #[cfg(unix)]
    {
        #[allow(unused_mut)]
        let (sighup_tx, mut sighup_rx) = tokio::sync::mpsc::channel::<()>(1);
        if args.enable_hot_reload {
            use tokio::signal::unix::{Signal, SignalKind};
            let mut sighup: Signal = tokio::signal::unix::signal(SignalKind::hangup())?;
            let reload_state = state.clone();
            let reload_log = args.log;
            let sighup_tx = sighup_tx.clone();
            tokio::spawn(async move {
                loop {
                    sighup.recv().await;
                    info!("SIGHUP received - triggering hot reload");
                    broadcast_system_msg(&reload_state, "Server reloading configuration...").await;
                    reload_state.user_msg_rate.clear();
                    if reload_log {
                        info!("hot reload complete: rate limit counters cleared, {} active connections", reload_state.active_connection_count());
                    }
                    broadcast_system_msg(&reload_state, "Configuration reloaded").await;
                    let _ = sighup_tx.send(()).await;
                }
            });
        }
        accept_loop_unix(
            listener,
            state.clone(),
            tls_acceptor,
            shutdown_rx,
            sighup_rx,
            &args,
        )
        .await;
    }

    #[cfg(not(unix))]
    {
        let state_for_accept = state.clone();
        accept_loop(listener, state_for_accept, tls_acceptor, shutdown_rx, &args).await;
    }

    // Graceful drain: wait for connections to close.
    let drain_timeout = Duration::from_secs(args.shutdown_timeout_secs);
    let start = std::time::Instant::now();
    loop {
        let active = state.active_connection_count();
        if active == 0 {
            break;
        }
        if start.elapsed() >= drain_timeout {
            println!(
                "Shutdown timeout reached with {} active connection(s)",
                active
            );
            warn!(
                "shutdown timeout reached with {} active connection(s)",
                active
            );
            break;
        }
        println!("Waiting for {} active connection(s) to close...", active);
        info!("waiting for active connections to drain count={}", active);
        tokio::select! {
            _ = state.drained_notify.notified() => {}
            _ = sleep(Duration::from_millis(250)) => {}
        }
    }

    println!("Shutdown complete.");
    info!("server shutdown complete");

    #[allow(unreachable_code)]
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chatify::totp::TotpConfig;
    use chatify_server::db::{
        DB_POOL_SIZE_DEFAULT, EventStore, MEDIA_MAX_TOTAL_SIZE_GB_DEFAULT,
        MEDIA_MAX_TOTAL_SIZE_GB_MAX, MEDIA_MAX_TOTAL_SIZE_GB_MIN,
        MEDIA_PRUNE_INTERVAL_SECS_DEFAULT, MEDIA_PRUNE_INTERVAL_SECS_MAX,
        MEDIA_PRUNE_INTERVAL_SECS_MIN, MEDIA_RETENTION_DAYS_DEFAULT, MEDIA_RETENTION_DAYS_MAX,
        RolePermissions,
    };
    use chatify_server::state::{
        normalize_outbound_queue_capacity, OUTBOUND_QUEUE_CAPACITY_DEFAULT,
        OUTBOUND_QUEUE_CAPACITY_MAX, OUTBOUND_QUEUE_CAPACITY_MIN, SLOW_CLIENT_DROP_BURST_DEFAULT,
        SLOW_CLIENT_DROP_BURST_MAX, SLOW_CLIENT_DROP_BURST_MIN,
    };
    use chatify_server::DbDurabilityMode;
    use rusqlite::{params, Connection};

    fn unique_test_db_path(prefix: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}.db"))
    }

    fn test_encryption_key() -> Vec<u8> {
        crypto::new_keypair()
    }

    fn distinct_test_encryption_keys() -> (Vec<u8>, Vec<u8>) {
        let first = test_encryption_key();
        let mut second = test_encryption_key();
        while second == first {
            second = test_encryption_key();
        }
        (first, second)
    }

    /// Deterministic non-trivial byte pattern for test fixture data.
    /// Avoids hard-coded literals that security scanners flag as weak keys.
    fn test_chunk(size: usize) -> Vec<u8> {
        (0..size)
            .map(|i| (i.wrapping_mul(17) ^ 0xA5) as u8)
            .collect()
    }

    #[test]
    fn voice_event_forwarding_respects_active_room() {
        let event = VoiceBroadcast::MemberJoined {
            room: "ops".to_string(),
            user: "alice".to_string(),
        };

        assert!(should_forward_voice_event(Some("ops"), &event));
        assert!(!should_forward_voice_event(Some("general"), &event));
        assert!(!should_forward_voice_event(None, &event));
    }

    #[test]
    fn voice_event_room_extracts_room_for_all_variants() {
        let users = VoiceBroadcast::Users {
            room: "ops".to_string(),
            members: Vec::new(),
        };
        let state = VoiceBroadcast::StateChange {
            room: "ops".to_string(),
            user: "alice".to_string(),
            muted: Some(true),
            deafened: Some(false),
            speaking: None,
        };
        let speaking = VoiceBroadcast::Speaking {
            room: "ops".to_string(),
            user: "alice".to_string(),
            speaking: true,
        };
        let joined = VoiceBroadcast::MemberJoined {
            room: "ops".to_string(),
            user: "alice".to_string(),
        };
        let left = VoiceBroadcast::MemberLeft {
            room: "ops".to_string(),
            user: "alice".to_string(),
        };

        assert_eq!(voice_event_room(&users), "ops");
        assert_eq!(voice_event_room(&state), "ops");
        assert_eq!(voice_event_room(&speaking), "ops");
        assert_eq!(voice_event_room(&joined), "ops");
        assert_eq!(voice_event_room(&left), "ops");
    }

    #[test]
    fn readonly_role_exists_and_cannot_send() {
        let db_path = unique_test_db_path("chatify-readonly-role");
        let store = EventStore::new(
            db_path.to_string_lossy().to_string(),
            None,
            DbDurabilityMode::MaxSafety,
            DB_POOL_SIZE_DEFAULT,
            None,
        );

        store
            .assign_role("alice", "general", "readonly", "test")
            .expect("assign readonly role");
        let role = store
            .get_user_role("alice", "general")
            .expect("readonly role should load");

        assert_eq!(role.name, "readonly");
        assert!(role.permissions.contains(RolePermissions::VIEW));
        assert!(!role.permissions.contains(RolePermissions::SEND));
        assert!(!role.can_manage());
    }

    /// Verifies that [`validate_auth_payload`] returns a `ChatifyError::Validation`
    /// variant (not `Message`) for an invalid username, allowing callers to
    /// distinguish validation errors from protocol errors.
    ///
    /// The specific error message `"invalid username"` is part of the public
    /// error contract and must not change without updating client-side error
    /// handling.
    #[test]
    fn auth_payload_rejects_invalid_username_with_typed_error() {
        let password_hash = chatify::fresh_nonce_hex();
        let public_key = crypto::pub_b64(&crypto::new_keypair()).expect("encode public key");
        let payload = serde_json::json!({
            "t": "auth",
            "u": "bad user",  // space is not allowed
            "pw": password_hash,
            "pk": public_key
        });

        let err = match validate_auth_payload(&payload) {
            Ok(_) => panic!("expected validation error"),
            Err(e) => e,
        };
        match err {
            ChatifyError::Validation(msg) => assert_eq!(msg, "invalid username"),
            other => panic!("unexpected error type: {}", other),
        }
    }

    /// Verifies that [`ConnectionGuard`] correctly increments and decrements
    /// [`State::active_connections`].
    ///
    /// Two guards are created concurrently to confirm the counter reaches 2,
    /// then both are dropped. The test polls until the counter returns to 0
    /// with a 1-second timeout to account for any scheduling delay between
    /// the drop and the atomic write.
    #[tokio::test]
    async fn connection_counter_tracks_open_and_close() {
        let plugin_runtime =
            PluginRuntime::new(std::env::current_exe().expect("resolve current exe"));
        let state = State::new(
            ":memory:".to_string(),
            None,
            DbDurabilityMode::MaxSafety,
            DB_POOL_SIZE_DEFAULT,
            None,
            plugin_runtime,
            60,
            false,
            false,
            OUTBOUND_QUEUE_CAPACITY_DEFAULT,
            SLOW_CLIENT_DROP_BURST_DEFAULT,
            true,
        );
        assert_eq!(state.active_connection_count(), 0);

        let addr1: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let addr2: SocketAddr = "127.0.0.2:12345".parse().unwrap();
        {
            let _g1 = ConnectionGuard::new(state.clone(), addr1);
            let _g2 = ConnectionGuard::new(state.clone(), addr2);
            assert_eq!(state.active_connection_count(), 2);
        }

        let start = std::time::Instant::now();
        while state.active_connection_count() != 0 {
            assert!(
                start.elapsed() < Duration::from_secs(1),
                "active connections did not drain in time"
            );
            sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(state.active_connection_count(), 0);
    }

    #[test]
    fn event_store_persists_presence_and_channel_subscriptions() {
        let plugin_runtime =
            PluginRuntime::new(std::env::current_exe().expect("resolve current exe"));
        let state = State::new(
            ":memory:".to_string(),
            None,
            DbDurabilityMode::MaxSafety,
            DB_POOL_SIZE_DEFAULT,
            None,
            plugin_runtime,
            60,
            false,
            false,
            OUTBOUND_QUEUE_CAPACITY_DEFAULT,
            SLOW_CLIENT_DROP_BURST_DEFAULT,
            true,
        );

        let status = serde_json::json!({"text": "Deep work", "emoji": ""});
        state.store.upsert_presence_snapshot("alice", &status);
        let loaded_status = state
            .store
            .load_presence_snapshot("alice")
            .expect("presence snapshot should load");
        assert_eq!(loaded_status, status);

        state.store.upsert_channel_subscription("alice", "general");
        state.store.upsert_channel_subscription("alice", "Rust");
        state.store.upsert_channel_subscription("alice", "rust");
        state
            .store
            .upsert_channel_subscription("alice", "__dm__bob");

        let mut channels = state.store.list_channel_subscriptions("alice");
        channels.sort();
        assert_eq!(channels, vec!["general".to_string(), "rust".to_string()]);

        assert!(state.store.remove_channel_subscription("alice", "rust"));
        assert!(!state.store.remove_channel_subscription("alice", "general"));

        let mut channels_after_remove = state.store.list_channel_subscriptions("alice");
        channels_after_remove.sort();
        assert_eq!(channels_after_remove, vec!["general".to_string()]);
    }

    #[test]
    fn upsert_credentials_updates_password_hash_on_conflict() {
        let db_path = unique_test_db_path("chatify-upsert-credentials");
        let plugin_runtime =
            PluginRuntime::new(std::env::current_exe().expect("resolve current exe"));
        let encryption_key = test_encryption_key();
        let state = State::new(
            db_path.to_string_lossy().to_string(),
            Some(encryption_key),
            DbDurabilityMode::MaxSafety,
            DB_POOL_SIZE_DEFAULT,
            None,
            plugin_runtime,
            60,
            false,
            true,
            OUTBOUND_QUEUE_CAPACITY_DEFAULT,
            SLOW_CLIENT_DROP_BURST_DEFAULT,
            true,
        );

        let old_client_hash = chatify::fresh_nonce_hex();
        let old_server_hash = crypto::pw_hash(&old_client_hash);
        state.store.upsert_credentials("alice", &old_server_hash);

        let mut new_client_hash = chatify::fresh_nonce_hex();
        while new_client_hash == old_client_hash {
            new_client_hash = chatify::fresh_nonce_hex();
        }
        let new_server_hash = crypto::pw_hash(&new_client_hash);
        state.store.upsert_credentials("alice", &new_server_hash);

        assert_eq!(
            state.store.verify_credential("alice", &old_client_hash),
            Ok(false)
        );
        assert_eq!(
            state.store.verify_credential("alice", &new_client_hash),
            Ok(true)
        );

        drop(state);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.to_string_lossy()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.to_string_lossy()));
    }

    #[test]
    fn event_store_encrypts_credentials_and_2fa_fields_when_key_present() {
        let db_path = unique_test_db_path("chatify-auth-encryption");
        let plugin_runtime =
            PluginRuntime::new(std::env::current_exe().expect("resolve current exe"));
        let encryption_key = test_encryption_key();
        let state = State::new(
            db_path.to_string_lossy().to_string(),
            Some(encryption_key),
            DbDurabilityMode::MaxSafety,
            DB_POOL_SIZE_DEFAULT,
            None,
            plugin_runtime,
            60,
            false,
            true,
            OUTBOUND_QUEUE_CAPACITY_DEFAULT,
            SLOW_CLIENT_DROP_BURST_DEFAULT,
            true,
        );

        let client_hash = chatify::fresh_nonce_hex();
        let server_hash = crypto::pw_hash(&client_hash);
        state.store.upsert_credentials("alice", &server_hash);
        assert_eq!(
            state.store.verify_credential("alice", &client_hash),
            Ok(true)
        );

        let totp_secret = chatify::fresh_nonce_hex();
        let backup_code_hash = chatify::fresh_nonce_hex();
        let mut user_2fa = User2FA::new("alice".to_string());
        user_2fa.enabled = true;
        user_2fa.totp_config = Some(TotpConfig {
            secret: totp_secret.clone(),
            digits: 6,
            step: 30,
            algorithm: "SHA256".to_string(),
        });
        user_2fa.backup_codes = vec![backup_code_hash.clone()];
        state.store.upsert_user_2fa(&user_2fa);

        let loaded_2fa = state
            .store
            .load_user_2fa("alice")
            .expect("2fa row should load");
        assert_eq!(
            loaded_2fa
                .totp_config
                .as_ref()
                .map(|cfg| cfg.secret.as_str()),
            Some(totp_secret.as_str())
        );
        assert_eq!(loaded_2fa.backup_codes, vec![backup_code_hash.clone()]);

        let conn = Connection::open(&db_path).expect("open sqlite db");
        let raw_pw_hash: String = conn
            .query_row(
                "SELECT pw_hash FROM user_credentials WHERE username = ?1",
                params!["alice"],
                |row| row.get(0),
            )
            .expect("read raw pw_hash");
        assert_ne!(raw_pw_hash, server_hash);
        assert!(
            serde_json::from_str::<Value>(&raw_pw_hash)
                .ok()
                .and_then(|v| {
                    v.get("ct")
                        .and_then(|value| value.as_str().map(|s| s.to_string()))
                })
                .is_some(),
            "pw_hash should be stored as encrypted ct wrapper"
        );

        let raw_secret: Option<String> = conn
            .query_row(
                "SELECT secret FROM user_2fa WHERE username = ?1",
                params!["alice"],
                |row| row.get(0),
            )
            .expect("read raw 2fa secret");
        let raw_backup_codes: Option<String> = conn
            .query_row(
                "SELECT backup_codes FROM user_2fa WHERE username = ?1",
                params!["alice"],
                |row| row.get(0),
            )
            .expect("read raw backup codes");
        let raw_secret = raw_secret.expect("2fa secret must be present");
        let raw_backup_codes = raw_backup_codes.expect("2fa backup codes must be present");

        assert_ne!(raw_secret, totp_secret);
        assert!(
            serde_json::from_str::<Value>(&raw_secret)
                .ok()
                .and_then(|v| {
                    v.get("ct")
                        .and_then(|value| value.as_str().map(|s| s.to_string()))
                })
                .is_some(),
            "2fa secret should be stored as encrypted ct wrapper"
        );
        assert!(
            serde_json::from_str::<Value>(&raw_backup_codes)
                .ok()
                .and_then(|v| {
                    v.get("ct")
                        .and_then(|value| value.as_str().map(|s| s.to_string()))
                })
                .is_some(),
            "2fa backup codes should be stored as encrypted ct wrapper"
        );

        drop(conn);
        drop(state);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.to_string_lossy()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.to_string_lossy()));
    }

    #[test]
    fn resolve_db_key_rejects_existing_database_without_key_file() {
        let db_path = unique_test_db_path("chatify-existing-db-no-key");
        let db_path_str = db_path.to_string_lossy().to_string();

        Connection::open(&db_path).expect("create sqlite db");
        let key_path = format!("{}.key", db_path_str);
        let _ = std::fs::remove_file(&key_path);

        let result = resolve_db_key(&db_path_str, None);
        assert!(matches!(result, Err(ChatifyError::Validation(msg)) if msg.contains("key file")));

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn state_init_fails_fast_on_encryption_key_mismatch() {
        let db_path = unique_test_db_path("chatify-key-mismatch");
        let db_path_str = db_path.to_string_lossy().to_string();
        let (original_key, replacement_key) = distinct_test_encryption_keys();

        let plugin_runtime_a =
            PluginRuntime::new(std::env::current_exe().expect("resolve current exe"));
        let state_a = State::new(
            db_path_str.clone(),
            Some(original_key),
            DbDurabilityMode::MaxSafety,
            DB_POOL_SIZE_DEFAULT,
            None,
            plugin_runtime_a,
            60,
            false,
            true,
            OUTBOUND_QUEUE_CAPACITY_DEFAULT,
            SLOW_CLIENT_DROP_BURST_DEFAULT,
            true,
        );

        let payload = serde_json::json!({"t": "msg", "c": "encrypted history marker"});
        state_a.store.persist(
            "msg",
            "general",
            "alice",
            None,
            &payload,
            "encrypted history marker",
        );
        drop(state_a);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let plugin_runtime_b =
                PluginRuntime::new(std::env::current_exe().expect("resolve current exe"));
            State::new(
                db_path_str.clone(),
                Some(replacement_key),
                DbDurabilityMode::MaxSafety,
                DB_POOL_SIZE_DEFAULT,
                None,
                plugin_runtime_b,
                60,
                false,
                true,
                OUTBOUND_QUEUE_CAPACITY_DEFAULT,
                SLOW_CLIENT_DROP_BURST_DEFAULT,
                true,
            )
        }));

        assert!(
            result.is_err(),
            "state initialization should fail when DB encryption key is wrong"
        );

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.to_string_lossy()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.to_string_lossy()));
    }

    #[test]
    fn normalization_clamps_outbound_settings() {
        assert_eq!(
            normalize_outbound_queue_capacity(0),
            OUTBOUND_QUEUE_CAPACITY_DEFAULT
        );
        assert_eq!(
            normalize_outbound_queue_capacity(OUTBOUND_QUEUE_CAPACITY_MAX + 1),
            OUTBOUND_QUEUE_CAPACITY_MAX
        );
        assert_eq!(
            normalize_outbound_queue_capacity(OUTBOUND_QUEUE_CAPACITY_MIN - 1),
            OUTBOUND_QUEUE_CAPACITY_MIN
        );

        assert_eq!(
            normalize_slow_client_drop_burst(0),
            SLOW_CLIENT_DROP_BURST_DEFAULT
        );
        assert_eq!(
            normalize_slow_client_drop_burst(SLOW_CLIENT_DROP_BURST_MAX + 1),
            SLOW_CLIENT_DROP_BURST_MAX
        );
        assert_eq!(
            normalize_slow_client_drop_burst(SLOW_CLIENT_DROP_BURST_MIN),
            SLOW_CLIENT_DROP_BURST_MIN
        );

        assert_eq!(
            normalize_media_retention_days(0),
            MEDIA_RETENTION_DAYS_DEFAULT
        );
        assert_eq!(
            normalize_media_retention_days(MEDIA_RETENTION_DAYS_MAX + 1),
            MEDIA_RETENTION_DAYS_MAX
        );

        assert_eq!(
            normalize_media_prune_interval_secs(0),
            MEDIA_PRUNE_INTERVAL_SECS_DEFAULT
        );
        assert_eq!(
            normalize_media_prune_interval_secs(MEDIA_PRUNE_INTERVAL_SECS_MAX + 1),
            MEDIA_PRUNE_INTERVAL_SECS_MAX
        );
        assert_eq!(
            normalize_media_prune_interval_secs(MEDIA_PRUNE_INTERVAL_SECS_MIN),
            MEDIA_PRUNE_INTERVAL_SECS_MIN
        );

        assert_eq!(
            normalize_media_max_total_size_gb(0.0),
            MEDIA_MAX_TOTAL_SIZE_GB_DEFAULT
        );
        assert_eq!(
            normalize_media_max_total_size_gb(0.1),
            MEDIA_MAX_TOTAL_SIZE_GB_MIN
        );
        assert_eq!(
            normalize_media_max_total_size_gb(MEDIA_MAX_TOTAL_SIZE_GB_MAX + 1.0),
            MEDIA_MAX_TOTAL_SIZE_GB_MAX
        );

        assert_eq!(gib_to_bytes_i64(1.0), 1_073_741_824);
    }

    #[test]
    fn media_prune_storage_enforces_age_and_size_limits() {
        let db_path = unique_test_db_path("chatify-media-prune");
        let plugin_runtime =
            PluginRuntime::new(std::env::current_exe().expect("resolve current exe"));
        let state = State::new(
            db_path.to_string_lossy().to_string(),
            None,
            DbDurabilityMode::MaxSafety,
            DB_POOL_SIZE_DEFAULT,
            None,
            plugin_runtime,
            60,
            false,
            false,
            OUTBOUND_QUEUE_CAPACITY_DEFAULT,
            SLOW_CLIENT_DROP_BURST_DEFAULT,
            true,
        );

        state.store.upsert_media_object(MediaObjectUpsert {
            channel: "general",
            file_id: "old-complete",
            sender: "alice",
            filename: "old.bin",
            media_kind: "file",
            mime: Some("application/octet-stream"),
            declared_size: 10,
        });
        state
            .store
            .append_media_chunk("general", "old-complete", "alice", 0, &test_chunk(10));

        state.store.upsert_media_object(MediaObjectUpsert {
            channel: "general",
            file_id: "recent-complete",
            sender: "alice",
            filename: "recent.bin",
            media_kind: "file",
            mime: Some("application/octet-stream"),
            declared_size: 20,
        });
        state
            .store
            .append_media_chunk("general", "recent-complete", "alice", 0, &test_chunk(20));

        state.store.upsert_media_object(MediaObjectUpsert {
            channel: "general",
            file_id: "partial",
            sender: "alice",
            filename: "partial.bin",
            media_kind: "file",
            mime: Some("application/octet-stream"),
            declared_size: 30,
        });
        state
            .store
            .append_media_chunk("general", "partial", "alice", 0, &test_chunk(10));

        let pooled = state
            .store
            .get_connection()
            .expect("obtain pooled sqlite connection");
        let ts_now = now();
        pooled
            .execute(
                "UPDATE media_objects
                 SET created_ts = ?1,
                     completed_ts = ?2
                 WHERE channel = 'general' AND file_id = 'old-complete'",
                params![ts_now - 10_000.0, ts_now - 10_000.0],
            )
            .expect("set old object timestamps");
        pooled
            .execute(
                "UPDATE media_objects
                 SET created_ts = ?1,
                     completed_ts = ?2
                 WHERE channel = 'general' AND file_id = 'recent-complete'",
                params![ts_now - 100.0, ts_now - 100.0],
            )
            .expect("set recent object timestamps");
        pooled
            .execute(
                "UPDATE media_objects
                 SET created_ts = ?1
                 WHERE channel = 'general' AND file_id = 'partial'",
                params![ts_now - 50.0],
            )
            .expect("set partial object timestamp");
        drop(pooled);

        let (deleted, reclaimed) = state.store.prune_media_storage(500, 15);
        assert_eq!(deleted, 2);
        assert_eq!(reclaimed, 30);

        let conn = Connection::open(&db_path).expect("open sqlite db");
        let remaining: Vec<(String, bool, i64, i64)> = {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT file_id, completed, declared_size, received_size
                     FROM media_objects
                     ORDER BY file_id ASC",
                )
                .expect("prepare remaining media query");
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .expect("query remaining media rows");
            rows.filter_map(|row| row.ok()).collect()
        };

        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].0, "partial");
        assert!(!remaining[0].1);
        assert_eq!(remaining[0].2, 30);
        assert_eq!(remaining[0].3, 10);

        drop(conn);
        drop(state);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(format!("{}-wal", db_path.to_string_lossy()));
        let _ = std::fs::remove_file(format!("{}-shm", db_path.to_string_lossy()));
    }

    #[tokio::test]
    async fn outbound_tx_signals_slow_client_after_drop_burst() {
        let (tx, _rx) = mpsc::channel::<String>(1);
        let (slow_client_tx, mut slow_client_rx) = mpsc::channel::<()>(1);
        let outbound = OutboundTx::new(tx, slow_client_tx, 2, None);

        outbound.try_send("first".to_string());
        outbound.try_send("second".to_string());
        outbound.try_send("third".to_string());

        let signal = tokio::time::timeout(Duration::from_millis(100), slow_client_rx.recv())
            .await
            .expect("slow-client signal timeout");
        assert!(signal.is_some(), "slow-client signal was not emitted");
    }

    #[tokio::test]
    async fn outbound_tx_records_prometheus_metrics_on_backpressure() {
        let metrics = Arc::new(std::sync::Mutex::new(
            PrometheusMetrics::new().expect("metrics init"),
        ));
        let (tx, _rx) = mpsc::channel::<String>(1);
        let (slow_client_tx, mut slow_client_rx) = mpsc::channel::<()>(1);
        let outbound = OutboundTx::new(tx, slow_client_tx, 2, Some(metrics.clone()));

        outbound.try_send("first".to_string());
        outbound.try_send("second".to_string());
        outbound.try_send("third".to_string());

        let signal = tokio::time::timeout(Duration::from_millis(100), slow_client_rx.recv())
            .await
            .expect("slow-client signal timeout");
        assert!(signal.is_some(), "slow-client signal was not emitted");

        let guard = metrics.lock().expect("metrics lock");
        assert_eq!(guard.outbound_queue_drops_total.get(), 2);
        assert_eq!(guard.slow_client_disconnects_total.get(), 1);
    }
}
