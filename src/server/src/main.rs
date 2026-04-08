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
//!
//! ## Protocol
//!
//! All frames are UTF-8 JSON objects with a mandatory `"t"` (type) field.
//! Binary frames are silently ignored. The first frame **must** be an `auth`
//! frame; any other type causes an immediate `err` response and disconnection.
//!
//! See [`validate_auth_payload`] for the full auth contract and
//! [`handle_event`] for the complete set of post-auth event types.

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use base64::{engine::general_purpose, Engine as _};
use clap::Parser;
use clifford::crypto;
use clifford::error::{ChatifyError, ChatifyResult};
use clifford::metrics::PrometheusMetrics;
use clifford::performance::{Metrics as PerfMetrics, VecCache};
use clifford::totp::{generate_qr_url, generate_secret, TotpConfig, User2FA};
use clifford::voice::{relay::VoiceBroadcast, VoiceRelay};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use log::{debug, info, warn};
use prometheus::Encoder;
use rusqlite::{params, Connection, Error as SqlError, OptionalExtension};
use serde_json::Value;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Notify, RwLock};
use tokio::time::{sleep, Duration};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::{
    accept_async, accept_hdr_async,
    tungstenite::{
        handshake::server::{Callback, Request, Response},
        http, Message,
    },
};

// ---------------------------------------------------------------------------
// CLI configuration
// ---------------------------------------------------------------------------

/// Command-line arguments for `clicord-server`.
///
/// Parsed once at startup by [clap]; the fields are consumed into [`State`]
/// and the bind address, and are not referenced again after `main` returns
/// from its setup phase.
#[derive(Parser)]
#[command(name = "clicord-server")]
struct Args {
    /// IP address the server will bind to. Use `127.0.0.1` to restrict to
    /// loopback (useful for testing behind a reverse proxy).
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    /// TCP port to listen on.
    #[arg(long, default_value_t = 8765)]
    port: u16,

    /// Enable structured logging via `env_logger`. When off, the server is
    /// completely silent except for the startup banner.
    #[arg(long)]
    log: bool,

    /// Path to the SQLite database file. The file is created automatically on
    /// first run and migrated to the current schema. Use `:memory:` for
    /// ephemeral in-process storage in tests.
    #[arg(long, default_value = "chatify.db")]
    db: String,

    /// Hex-encoded encryption key for the SQLite database (SQLCipher).
    /// Must be exactly 64 hex characters (32 bytes).
    /// If omitted, the server looks for a `<db>.key` file. If neither
    /// exists, a new random key is generated and saved to `<db>.key`.
    /// Use `:memory:` databases (tests) to skip encryption entirely.
    #[arg(long)]
    db_key: Option<String>,

    /// Enable TLS for WebSocket connections (wss://).
    /// Requires `--tls-cert` and `--tls-key`.
    #[arg(long)]
    tls: bool,

    /// Path to the TLS certificate file (PEM format).
    /// Used when `--tls` is enabled.
    #[arg(long, default_value = "cert.pem")]
    tls_cert: String,

    /// Path to the TLS private key file (PEM format).
    /// Used when `--tls` is enabled.
    #[arg(long, default_value = "key.pem")]
    tls_key: String,

    /// TCP port for health check and metrics endpoints.
    /// Set to 0 to disable the HTTP server.
    #[arg(long, default_value_t = 8080)]
    health_port: u16,

    /// Enable Prometheus metrics endpoint.
    #[arg(long, default_value_t = true)]
    metrics_enabled: bool,

    /// Maximum time in seconds to wait for connections to drain during shutdown.
    #[arg(long, default_value_t = 30)]
    shutdown_timeout_secs: u64,

    /// Enable handling SIGHUP for config hot-reload.
    #[arg(long)]
    enable_hot_reload: bool,

    /// Enable shutdown HTTP endpoint for orchestration (POST /shutdown).
    #[arg(long)]
    shutdown_endpoint: bool,

    /// Maximum messages per user per minute (0 = unlimited).
    #[arg(long, default_value_t = 60)]
    max_msgs_per_minute: u32,

    /// Enable per-user rate limiting.
    #[arg(long)]
    enable_user_rate_limit: bool,
}

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

/// Maximum number of messages kept in the per-channel in-memory ring buffer.
/// Older entries are evicted when the buffer is full. Persistent history is
/// unbounded in SQLite; this cap only affects the in-process hot cache.
const HISTORY_CAP: usize = 50;

/// Maximum byte length for any post-auth WebSocket frame.
/// Frames exceeding this are rejected before JSON parsing to limit memory
/// pressure from a single misbehaving or malicious client.
const MAX_BYTES: usize = 16_000;

/// Maximum byte length for the initial auth frame.
/// Tighter than [`MAX_BYTES`] because the auth payload is parsed before the
/// client is known/trusted, making it a potential amplification target.
const MAX_AUTH_BYTES: usize = 4_096;

/// Maximum HTTP header size during WebSocket handshake (CVE-2023-43668 mitigation).
/// Limits the total size of HTTP headers during the WebSocket upgrade handshake
/// to prevent denial-of-service attacks via excessive header length.
/// Tungstenite 0.21.0+ includes this fix, but we enforce it explicitly.
const MAX_HANDSHAKE_HEADER_SIZE: usize = 8192;

/// Maximum number of HTTP headers during WebSocket handshake.
/// Limits the number of individual headers to prevent resource exhaustion.
const MAX_HANDSHAKE_HEADERS: usize = 64;

/// SQLite schema version this binary was built against.
/// The migration path in [`EventStore::migrate`] upgrades from any lower
/// version to this one. If the stored version is *higher*, the server logs a
/// warning and continues without modifying the schema (no downgrade).
const CURRENT_SCHEMA_VERSION: i64 = 6;

/// Default number of events returned by a `history` request.
const DEFAULT_HISTORY_LIMIT: usize = 50;

/// Default number of events returned by a `search` request.
const DEFAULT_SEARCH_LIMIT: usize = 30;

/// Default look-back window in seconds for a `rewind` request (1 hour).
const DEFAULT_REWIND_SECONDS: u64 = 3600;

/// Default number of events returned by a `rewind` request.
const DEFAULT_REWIND_LIMIT: usize = 100;

/// DM channel name prefix - avoids repeated format!() calls.
const DM_CHANNEL_PREFIX: &str = "__dm__";

/// Creates a DM channel name for a given user.
fn dm_channel_name(user: &str) -> String {
    format!("{}{}", DM_CHANNEL_PREFIX, user)
}

/// WebSocket sub-protocol version advertised in the `"ok"` auth response.
/// Clients can use this to detect incompatible server versions.
const PROTOCOL_VERSION: u64 = 1;

/// Maximum allowed username length in characters (ASCII only).
const MAX_USERNAME_LEN: usize = 32;

/// Maximum allowed length for the `"pw"` (password hash) field in the auth
/// frame. This covers SHA-256 hex strings (64 chars) with generous headroom
/// for other hash schemes.
const MAX_PASSWORD_FIELD_LEN: usize = 256;

/// Maximum allowed length for the `"pk"` (public key) base64 field.
const MAX_PUBLIC_KEY_FIELD_LEN: usize = 256;

/// Allowed clock skew in seconds between the client-supplied `"ts"` timestamp
/// and the server's wall clock. A window of Ãƒâ€šÃ‚Â±300 s accommodates clients with
/// moderately drifted clocks while still preventing stale-message replay.
const MAX_CLOCK_SKEW_SECS: f64 = 300.0;

/// Maximum length of a nonce string in the `"n"` field. This bounds the
/// nonce-cache entry size and prevents artificially long strings from being
/// used as a timing oracle.
const MAX_NONCE_LEN: usize = 64;

/// Maximum number of recently seen nonces remembered per user.
/// Once the deque is full the oldest entry is evicted. Choosing a cap large
/// enough to cover the clock-skew window prevents replay within that window
/// while bounding per-user memory to `NONCE_CACHE_CAP * MAX_NONCE_LEN` bytes.
const NONCE_CACHE_CAP: usize = 256;

/// How often the nonce cleanup task runs (seconds).
/// The sweep is cheap ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â it only iterates `nonce_last_seen`, which has at most
/// one entry per connected user.
const NONCE_CLEANUP_INTERVAL_SECS: u64 = 60;

/// Age threshold for nonce eviction (seconds).
/// Users whose nonce cache has not been updated within this window are removed.
/// Set to 2ÃƒÆ’Ã¢â‚¬â€ the clock-skew window: any frame older than `MAX_CLOCK_SKEW_SECS`
/// is already rejected by `validate_timestamp_skew`, so the extra margin is
/// pure safety against clock jitter.
const NONCE_MAX_AGE_SECS: f64 = MAX_CLOCK_SKEW_SECS * 2.0;

/// Maximum number of concurrent connections from a single IP address.
/// Connections beyond this are rejected with a rate-limit error.
const MAX_CONNECTIONS_PER_IP: usize = 5;

/// Minimum interval in seconds between auth attempts from the same IP.
/// Attempts within this window are rejected to slow brute-force attacks.
/// Set to 0.5s ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â enough to throttle automated tools while allowing
/// rapid legitimate connections (e.g. from integration tests).
const AUTH_RATE_LIMIT_SECS: f64 = 0.5;

/// Maximum file transfer size in bytes (100 MB).
const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;

/// Maximum filename length accepted in `file_meta` announcements.
const MAX_FILE_NAME_LEN: usize = 256;

/// Maximum transfer identifier length accepted in `file_meta` announcements.
const MAX_FILE_ID_LEN: usize = 128;

/// Maximum MIME type length accepted in `file_meta` announcements.
const MAX_MEDIA_MIME_LEN: usize = 128;

/// Maximum allowed length for a status text field.
const MAX_STATUS_TEXT_LEN: usize = 128;

/// Maximum allowed length for a status emoji field.
const MAX_STATUS_EMOJI_LEN: usize = 16;

/// Maximum allowed length for server-issued message IDs.
const MAX_MSG_ID_LEN: usize = 64;

/// Maximum allowed length for reaction emoji payload.
const MAX_REACTION_EMOJI_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Validated, strongly-typed representation of a successful auth frame parse.
///
/// Created by [`validate_auth_payload`] after all field-level validation
/// passes. Using a typed struct here ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â rather than passing `&Value` through
/// downstream functions ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â makes it impossible to accidentally skip validation
/// or misread a field name.
struct AuthInfo {
    /// Validated username (ASCII alphanumeric / `-` / `_`, ÃƒÂ¢Ã¢â‚¬Â°Ã‚Â¤ 32 chars).
    username: String,

    /// Password hash submitted by the client (non-empty, ÃƒÂ¢Ã¢â‚¬Â°Ã‚Â¤ 256 chars).
    /// Used for credential verification against the stored hash.
    pw_hash: String,

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
// Write Queue Ã¢â‚¬â€ Batched DB writes for high-throughput scenarios
// Only used when `batch-writes` feature is enabled
// ---------------------------------------------------------------------------

#[cfg(feature = "batch-writes")]
use std::sync::Arc as StdArc;

#[cfg(feature = "batch-writes")]
const BATCH_SIZE: usize = 256;
#[cfg(feature = "batch-writes")]
const BATCH_FLUSH_MS: u64 = 250;

#[cfg(feature = "batch-writes")]
struct EventRow {
    event_type: String,
    channel: String,
    sender: String,
    target: Option<String>,
    payload: String,
    search_text: String,
    ts: f64,
}

#[cfg(feature = "batch-writes")]
struct WriteQueue {
    pool: DbPool,
    rows: parking_lot::Mutex<Vec<EventRow>>,
    flush_count: AtomicUsize,
}

#[cfg(feature = "batch-writes")]
impl WriteQueue {
    fn new(pool: DbPool) -> Self {
        Self {
            pool,
            rows: parking_lot::Mutex::new(Vec::with_capacity(BATCH_SIZE)),
            flush_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn push(&self, row: EventRow) {
        let should_flush = {
            let mut rows = self.rows.lock();
            rows.push(row);
            rows.len() >= BATCH_SIZE
        };
        if should_flush {
            self.flush();
        }
    }

    fn flush(&self) {
        let rows: Vec<EventRow> = {
            let mut rows_guard = self.rows.lock();
            if rows_guard.is_empty() {
                return;
            }
            std::mem::take(&mut *rows_guard)
        };

        if let Err(failed_rows) = self.flush_rows(rows) {
            let mut pending = self.rows.lock();
            if pending.is_empty() {
                *pending = failed_rows;
            } else {
                let mut merged = failed_rows;
                merged.append(&mut *pending);
                *pending = merged;
            }
        }
    }

    fn flush_rows(&self, rows: Vec<EventRow>) -> Result<(), Vec<EventRow>> {
        if rows.is_empty() {
            return Ok(());
        }

        let mut conn = match self.pool.pool.get() {
            Ok(conn) => conn,
            Err(e) => {
                warn!("batch flush failed to acquire db connection: {}", e);
                return Err(rows);
            }
        };

        let tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(e) => {
                warn!("batch flush failed to start transaction: {}", e);
                return Err(rows);
            }
        };

        {
            let mut stmt = match tx.prepare_cached(
                "INSERT INTO events(ts, event_type, channel, sender, target, payload, search_text)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            ) {
                Ok(stmt) => stmt,
                Err(e) => {
                    warn!("batch flush failed to prepare statement: {}", e);
                    return Err(rows);
                }
            };

            for row in &rows {
                if let Err(e) = stmt.execute(params![
                    row.ts,
                    row.event_type,
                    row.channel,
                    row.sender,
                    row.target,
                    row.payload,
                    row.search_text
                ]) {
                    warn!("batch flush insert failed: {}", e);
                    return Err(rows);
                }
            }
        }

        if let Err(e) = tx.commit() {
            warn!("batch flush commit failed: {}", e);
            return Err(rows);
        }

        self.flush_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn start_worker(queue: &StdArc<Self>) {
        let weak_queue = StdArc::downgrade(queue);
        let spawn_result = std::thread::Builder::new()
            .name("chatify-batch-writer".to_string())
            .spawn(move || {
                let interval = std::time::Duration::from_millis(BATCH_FLUSH_MS);
                loop {
                    std::thread::sleep(interval);
                    let Some(queue) = weak_queue.upgrade() else {
                        break;
                    };
                    queue.flush();
                }
            });

        if let Err(e) = spawn_result {
            warn!("failed to spawn batch write worker: {}", e);
        }
    }
}

#[cfg(feature = "batch-writes")]
impl Drop for WriteQueue {
    fn drop(&mut self) {
        self.flush();
    }
}

// ---------------------------------------------------------------------------
// Roles & Permissions System
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    #[derive(Clone, Debug, Default)]
    pub struct RolePermissions: u32 {
        const NONE      = 0;
        const VIEW       = 1 << 0;
        const SEND       = 1 << 1;
        const KICK       = 1 << 2;
        const BAN        = 1 << 3;
        const MUTE       = 1 << 4;
        const MANAGE     = 1 << 5;
        const PIN        = 1 << 6;
    }
}

impl RolePermissions {
    pub fn from_db_row(
        can_kick: bool,
        can_ban: bool,
        can_mute: bool,
        can_manage: bool,
        can_pin: bool,
    ) -> Self {
        let mut perms = Self::NONE;
        perms |= Self::VIEW | Self::SEND;
        if can_kick {
            perms |= Self::KICK;
        }
        if can_ban {
            perms |= Self::BAN;
        }
        if can_mute {
            perms |= Self::MUTE;
        }
        if can_manage {
            perms |= Self::MANAGE;
        }
        if can_pin {
            perms |= Self::PIN;
        }
        perms
    }
}

#[derive(Clone, Debug)]
pub struct Role {
    pub id: i64,
    pub name: String,
    pub level: i32,
    pub permissions: RolePermissions,
}

impl Role {
    pub fn is_admin(&self) -> bool {
        self.level >= 100
    }

    pub fn can_kick(&self) -> bool {
        self.permissions.contains(RolePermissions::KICK)
    }

    pub fn can_ban(&self) -> bool {
        self.permissions.contains(RolePermissions::BAN)
    }

    pub fn can_mute(&self) -> bool {
        self.permissions.contains(RolePermissions::MUTE)
    }

    pub fn can_manage(&self) -> bool {
        self.permissions.contains(RolePermissions::MANAGE)
    }
}

#[derive(Clone, Debug)]
pub struct Ban {
    pub username: String,
    pub channel: String,
    pub banned_by: String,
    pub reason: Option<String>,
    pub banned_at: f64,
    pub expires_at: Option<f64>,
}

impl Ban {
    pub fn is_active(&self) -> bool {
        if let Some(expires) = self.expires_at {
            crate::now() < expires
        } else {
            true
        }
    }
}

#[derive(Clone, Debug)]
pub struct Mute {
    pub username: String,
    pub channel: String,
    pub muted_by: String,
    pub reason: Option<String>,
    pub muted_at: f64,
    pub expires_at: Option<f64>,
}

impl Mute {
    pub fn is_active(&self) -> bool {
        if let Some(expires) = self.expires_at {
            crate::now() < expires
        } else {
            true
        }
    }
}

// ---------------------------------------------------------------------------
// EventStore Ã¢â‚¬â€ SQLite persistence layer with connection pooling
// ---------------------------------------------------------------------------

const DB_POOL_SIZE: u32 = 8;
const DB_POOL_MIN_IDLE: u32 = 2;
const DB_POOL_IDLE_TIMEOUT_SECS: u64 = 60;
const DB_PRAGMAS: &str = "
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = NORMAL;
    PRAGMA cache_size = -2000;
    PRAGMA temp_store = MEMORY;
    PRAGMA foreign_keys = ON;
    PRAGMA mmap_size = 268435456;
    PRAGMA page_size = 4096;
";

#[derive(Clone)]
struct PooledConnection {
    #[allow(dead_code)]
    path: String,
    #[allow(dead_code)]
    encryption_key: Option<Vec<u8>>,
}

impl PooledConnection {
    fn new(path: String, encryption_key: Option<Vec<u8>>) -> Self {
        Self {
            path,
            encryption_key,
        }
    }
}

impl r2d2::ManageConnection for PooledConnection {
    type Connection = Connection;
    type Error = rusqlite::Error;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let conn = Connection::open(&self.path)?;
        if self.path != ":memory:" {
            conn.execute_batch(DB_PRAGMAS)?;
        } else {
            conn.execute_batch("PRAGMA foreign_keys = ON")?;
        }
        conn.set_prepared_statement_cache_capacity(100);
        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        conn.execute_batch("SELECT 1")
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        !conn.is_autocommit()
    }
}

#[derive(Clone)]
struct DbPool {
    #[allow(dead_code)]
    path: String,
    #[allow(dead_code)]
    encryption_key: Option<Vec<u8>>,
    pool: r2d2::Pool<PooledConnection>,
}

impl DbPool {
    fn new(path: String, encryption_key: Option<Vec<u8>>) -> Result<Self, r2d2::Error> {
        let manager = PooledConnection::new(path.clone(), encryption_key.clone());
        let pool = r2d2::Pool::builder()
            .max_size(DB_POOL_SIZE)
            .min_idle(Some(DB_POOL_MIN_IDLE))
            .idle_timeout(Some(std::time::Duration::from_secs(
                DB_POOL_IDLE_TIMEOUT_SECS,
            )))
            .connection_timeout(std::time::Duration::from_secs(10))
            .test_on_check_out(true)
            .build(manager)?;
        Ok(Self {
            pool,
            path,
            encryption_key,
        })
    }
}

#[derive(Clone)]
struct EventStore {
    pool: DbPool,
    #[cfg(feature = "batch-writes")]
    write_queue: Option<StdArc<WriteQueue>>,
}

type RoleRow = (i64, String, i32, bool, bool, bool, bool, bool);
type BanMuteRow = (String, String, String, Option<String>, f64, Option<f64>);

impl EventStore {
    fn new(path: String, encryption_key: Option<Vec<u8>>) -> Self {
        let pool = DbPool::new(path.clone(), encryption_key.clone())
            .expect("failed to create database pool");
        #[cfg(feature = "batch-writes")]
        let write_queue = {
            let queue = StdArc::new(WriteQueue::new(pool.clone()));
            WriteQueue::start_worker(&queue);
            Some(queue)
        };
        let store = Self {
            pool,
            #[cfg(feature = "batch-writes")]
            write_queue,
        };
        store.init().expect("failed to initialise event store Ã¢â‚¬â€ check database path, permissions, and encryption key");
        store
    }

    fn is_encrypted(&self) -> bool {
        self.pool.encryption_key.is_some()
    }

    fn health_check(&self) -> bool {
        if let Some(conn) = self.get_connection() {
            conn.query_row("SELECT 1", [], |_| Ok(())).is_ok()
        } else {
            false
        }
    }

    fn init(&self) -> rusqlite::Result<()> {
        let conn = self
            .get_connection()
            .ok_or_else(|| rusqlite::Error::InvalidQuery)?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;
        let version = Self::schema_version(&conn)?;
        self.migrate(&conn, version)?;
        Ok(())
    }

    fn get_connection(&self) -> Option<r2d2::PooledConnection<PooledConnection>> {
        self.pool.pool.get().ok()
    }

    fn get_pool_stats(&self) -> clifford::performance::PoolStats {
        use clifford::performance::PoolStats;
        let state = self.pool.pool.state();
        PoolStats {
            active_connections: (state.connections - state.idle_connections) as usize,
            idle_connections: state.idle_connections as usize,
            total_connections: state.connections as usize,
            wait_count: 0,
            acquisition_count: 0,
            release_count: 0,
        }
    }

    fn schema_version(conn: &Connection) -> rusqlite::Result<i64> {
        let value: rusqlite::Result<String> = conn.query_row(
            "SELECT value FROM schema_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        );
        match value {
            Ok(v) => Ok(v.parse::<i64>().unwrap_or(0)),
            Err(SqlError::QueryReturnedNoRows) => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn set_schema_version(conn: &Connection, version: i64) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT INTO schema_meta(key, value)
             VALUES('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![version.to_string()],
        )?;
        Ok(())
    }

    fn migrate(&self, conn: &Connection, from_version: i64) -> rusqlite::Result<()> {
        let mut version = from_version;

        if version < 1 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS events (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts          REAL    NOT NULL,
                    event_type  TEXT    NOT NULL,
                    channel     TEXT    NOT NULL,
                    sender      TEXT,
                    target      TEXT,
                    payload     TEXT    NOT NULL,
                    search_text TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_events_channel_ts
                    ON events(channel, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_events_search
                    ON events(search_text);
                ",
            )?;
            version = 1;
            Self::set_schema_version(conn, version)?;
        }

        if version < 2 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS user_2fa (
                    username      TEXT    PRIMARY KEY,
                    enabled       BOOLEAN NOT NULL DEFAULT FALSE,
                    secret        TEXT,
                    backup_codes  TEXT,
                    enabled_at    REAL,
                    last_verified REAL
                );
                ",
            )?;
            version = 2;
            Self::set_schema_version(conn, version)?;
        }

        if version < 3 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS user_credentials (
                    username     TEXT PRIMARY KEY,
                    pw_hash      TEXT NOT NULL,
                    created_at   REAL NOT NULL,
                    updated_at   REAL NOT NULL,
                    login_count  INTEGER NOT NULL DEFAULT 0,
                    last_login   REAL
                );
                ",
            )?;
            version = 3;
            Self::set_schema_version(conn, version)?;
        }

        if version < 4 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS events (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts          REAL    NOT NULL,
                    event_type  TEXT    NOT NULL,
                    channel     TEXT    NOT NULL,
                    sender      TEXT,
                    target      TEXT,
                    payload     TEXT    NOT NULL,
                    search_text TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_events_channel_ts
                    ON events(channel, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_events_search
                    ON events(search_text);
                CREATE INDEX IF NOT EXISTS idx_events_dm_route_ts
                    ON events(event_type, sender, target, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_events_channel_search_ts
                    ON events(channel, ts DESC, search_text);

                CREATE TRIGGER IF NOT EXISTS trg_events_append_only_update
                BEFORE UPDATE ON events
                BEGIN
                    SELECT RAISE(ABORT, 'events is append-only');
                END;

                CREATE TRIGGER IF NOT EXISTS trg_events_append_only_delete
                BEFORE DELETE ON events
                BEGIN
                    SELECT RAISE(ABORT, 'events is append-only');
                END;
                ",
            )?;
            version = 4;
            Self::set_schema_version(conn, version)?;
        }

        if version < 5 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS roles (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    name        TEXT NOT NULL UNIQUE,
                    level       INTEGER NOT NULL DEFAULT 0,
                    can_kick    BOOLEAN NOT NULL DEFAULT FALSE,
                    can_ban    BOOLEAN NOT NULL DEFAULT FALSE,
                    can_mute    BOOLEAN NOT NULL DEFAULT FALSE,
                    can_manage  BOOLEAN NOT NULL DEFAULT FALSE,
                    can_pin     BOOLEAN NOT NULL DEFAULT FALSE,
                    created_at  REAL NOT NULL
                );

                CREATE TABLE IF NOT EXISTS user_roles (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    username    TEXT NOT NULL,
                    channel     TEXT NOT NULL,
                    role_id     INTEGER NOT NULL,
                    assigned_by TEXT NOT NULL,
                    assigned_at  REAL NOT NULL,
                    UNIQUE(username, channel),
                    FOREIGN KEY (role_id) REFERENCES roles(id)
                );

                CREATE TABLE IF NOT EXISTS bans (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    username    TEXT NOT NULL,
                    channel     TEXT NOT NULL,
                    banned_by   TEXT NOT NULL,
                    reason      TEXT,
                    banned_at   REAL NOT NULL,
                    expires_at  REAL,
                    UNIQUE(username, channel)
                );

                CREATE TABLE IF NOT EXISTS mutes (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    username    TEXT NOT NULL,
                    channel     TEXT NOT NULL,
                    muted_by    TEXT NOT NULL,
                    reason      TEXT,
                    muted_at    REAL NOT NULL,
                    expires_at  REAL,
                    UNIQUE(username, channel)
                );

                CREATE INDEX IF NOT EXISTS idx_user_roles_lookup
                    ON user_roles(username, channel);
                CREATE INDEX IF NOT EXISTS idx_bans_lookup
                    ON bans(username, channel);
                CREATE INDEX IF NOT EXISTS idx_mutes_lookup
                    ON mutes(username, channel);

                INSERT OR IGNORE INTO roles (name, level, can_kick, can_ban, can_mute, can_manage, can_pin, created_at)
                VALUES ('admin', 100, TRUE, TRUE, TRUE, TRUE, TRUE, ?1);
                INSERT OR IGNORE INTO roles (name, level, can_kick, can_ban, can_mute, can_manage, can_pin, created_at)
                VALUES ('moderator', 50, TRUE, TRUE, TRUE, FALSE, TRUE, ?1);
                INSERT OR IGNORE INTO roles (name, level, can_kick, can_ban, can_mute, can_manage, can_pin, created_at)
                VALUES ('member', 10, FALSE, FALSE, FALSE, FALSE, FALSE, ?1);
                INSERT OR IGNORE INTO roles (name, level, can_kick, can_ban, can_mute, can_manage, can_pin, created_at)
                VALUES ('guest', 1, FALSE, FALSE, FALSE, FALSE, FALSE, ?1);
                ",
            )?;
            version = 5;
            Self::set_schema_version(conn, version)?;
        }

        if version > CURRENT_SCHEMA_VERSION {
            warn!(
                "Database schema version {} is newer than supported version {}",
                version, CURRENT_SCHEMA_VERSION
            );
        }

        if version < 6 {
            conn.execute_batch(
                "
                CREATE TABLE IF NOT EXISTS audit_logs (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    action      TEXT NOT NULL,
                    actor       TEXT NOT NULL,
                    target      TEXT,
                    channel     TEXT,
                    reason      TEXT,
                    metadata    TEXT,
                    ts          REAL NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_audit_logs_action
                    ON audit_logs(action, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_audit_logs_actor
                    ON audit_logs(actor, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_audit_logs_target
                    ON audit_logs(target, ts DESC);
                CREATE INDEX IF NOT EXISTS idx_audit_logs_channel
                    ON audit_logs(channel, ts DESC);

                CREATE TABLE IF NOT EXISTS suspicious_activity (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    target_username TEXT NOT NULL,
                    activity_type   TEXT NOT NULL,
                    severity        TEXT NOT NULL DEFAULT 'low',
                    details         TEXT,
                    resolved        BOOLEAN NOT NULL DEFAULT FALSE,
                    resolved_by     TEXT,
                    resolved_at     REAL,
                    ts              REAL NOT NULL,
                    UNIQUE(target_username, activity_type, ts)
                );

                CREATE INDEX IF NOT EXISTS idx_suspicious_activity_lookup
                    ON suspicious_activity(target_username, activity_type, ts DESC);
                ",
            )?;

            conn.execute(
                "ALTER TABLE user_credentials ADD COLUMN failed_attempts INTEGER NOT NULL DEFAULT 0",
                [],
            ).ok();

            conn.execute(
                "ALTER TABLE user_credentials ADD COLUMN locked_until REAL NOT NULL DEFAULT 0",
                [],
            )
            .ok();

            version = 6;
            Self::set_schema_version(conn, version)?;
        }

        Ok(())
    }

    fn encrypt_field(&self, plaintext: &str) -> String {
        if let Some(ref key) = self.pool.encryption_key {
            match crypto::enc_bytes(key, plaintext.as_bytes()) {
                Ok(ct) => serde_json::json!({"ct": hex::encode(ct)}).to_string(),
                Err(e) => {
                    warn!("encryption failed, storing plaintext: {}", e);
                    plaintext.to_string()
                }
            }
        } else {
            plaintext.to_string()
        }
    }

    fn decrypt_field(&self, stored: &str) -> String {
        if let Some(ref key) = self.pool.encryption_key {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(stored) {
                if let Some(ct_hex) = val.get("ct").and_then(|v| v.as_str()) {
                    if let Ok(ct_bytes) = hex::decode(ct_hex) {
                        match crypto::dec_bytes(key, &ct_bytes) {
                            Ok(pt) => return String::from_utf8_lossy(&pt).to_string(),
                            Err(e) => {
                                warn!("decryption failed: {}", e);
                                return stored.to_string();
                            }
                        }
                    }
                }
            }
        }
        stored.to_string()
    }

    fn decode_rows(rows: Vec<String>) -> Vec<Value> {
        rows.into_iter()
            .filter_map(|payload| serde_json::from_str::<Value>(&payload).ok())
            .collect()
    }

    fn query_events<P>(&self, sql: &str, params: P) -> Vec<Value>
    where
        P: rusqlite::Params,
    {
        let Some(conn) = self.get_connection() else {
            return Vec::new();
        };
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => {
                warn!("event query prepare failed: {}", e);
                return Vec::new();
            }
        };

        let rows = match stmt.query_map(params, |row| row.get::<_, String>(0)) {
            Ok(r) => r,
            Err(e) => {
                warn!("event query execute failed: {}", e);
                return Vec::new();
            }
        };

        let decrypted: Vec<String> = rows
            .filter_map(|r| r.ok())
            .map(|raw| self.decrypt_field(&raw))
            .collect();

        Self::decode_rows(decrypted)
    }

    fn persist(
        &self,
        event_type: &str,
        channel: &str,
        sender: &str,
        target: Option<&str>,
        payload: &Value,
        search_text: &str,
    ) {
        #[cfg(feature = "batch-writes")]
        if let Some(ref queue) = self.write_queue {
            let payload_json = payload.to_string();
            queue.push(EventRow {
                event_type: event_type.to_string(),
                channel: channel.to_string(),
                sender: sender.to_string(),
                target: target.map(String::from),
                payload: self.encrypt_field(&payload_json),
                search_text: self.encrypt_field(&search_text.to_lowercase()),
                ts: now(),
            });
            return;
        }

        #[cfg(not(feature = "batch-writes"))]
        {
            let Some(conn) = self.get_connection() else {
                return;
            };
            let payload_json = payload.to_string();
            let enc_payload = self.encrypt_field(&payload_json);
            let enc_search = self.encrypt_field(&search_text.to_lowercase());
            if let Err(e) = conn.execute(
                "INSERT INTO events(ts, event_type, channel, sender, target, payload, search_text)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    now(),
                    event_type,
                    channel,
                    sender,
                    target,
                    enc_payload,
                    enc_search,
                ],
            ) {
                warn!("event persist failed: {}", e);
            }
        }
    }

    fn history(&self, channel: &str, limit: usize) -> Vec<Value> {
        self.query_events(
            "SELECT payload FROM events
             WHERE channel = ?1
             ORDER BY ts DESC
             LIMIT ?2",
            params![channel, limit as i64],
        )
    }

    fn reaction_events(&self, channel: &str, limit: usize) -> Vec<Value> {
        self.query_events(
            "SELECT payload FROM events
             WHERE channel = ?1 AND event_type = 'reaction'
             ORDER BY ts DESC
             LIMIT ?2",
            params![channel, limit as i64],
        )
    }

    fn history_since(&self, channel: &str, from_ts: f64, limit: usize) -> Vec<Value> {
        self.query_events(
            "SELECT payload FROM events
                         WHERE channel = ?1 AND ts >= ?2
                         ORDER BY ts DESC
                         LIMIT ?3",
            params![channel, from_ts, limit as i64],
        )
    }

    fn dm_history(&self, username: &str, peer: &str, limit: usize) -> Vec<Value> {
        self.query_events(
            "SELECT payload FROM events
             WHERE event_type = 'dm' AND ((sender = ?1 AND target = ?2) OR (sender = ?2 AND target = ?1))
             ORDER BY ts DESC
             LIMIT ?3",
            params![username, peer, limit as i64],
        )
    }

    fn dm_rewind(&self, username: &str, peer: &str, seconds: u64, limit: usize) -> Vec<Value> {
        let cutoff = (now() - seconds as f64).max(0.0);
        self.query_events(
            "SELECT payload FROM events
             WHERE event_type = 'dm' AND ts >= ?3 AND ((sender = ?1 AND target = ?2) OR (sender = ?2 AND target = ?1))
             ORDER BY ts DESC
             LIMIT ?4",
            params![username, peer, cutoff, limit as i64],
        )
    }

    fn dm_history_since(
        &self,
        username: &str,
        peer: &str,
        from_ts: f64,
        limit: usize,
    ) -> Vec<Value> {
        self.query_events(
            "SELECT payload FROM events
             WHERE event_type = 'dm' AND ts >= ?3 AND ((sender = ?1 AND target = ?2) OR (sender = ?2 AND target = ?1))
             ORDER BY ts DESC
             LIMIT ?4",
            params![username, peer, from_ts, limit as i64],
        )
    }

    fn search_encrypted<P>(
        &self,
        sql: &str,
        params: P,
        query_lower: &str,
        limit: usize,
        label: &str,
    ) -> Vec<Value>
    where
        P: rusqlite::Params,
    {
        let Some(conn) = self.get_connection() else {
            return Vec::new();
        };
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => {
                warn!("{} search query prepare failed: {}", label, e);
                return Vec::new();
            }
        };
        let rows: Vec<(String, String)> = stmt
            .query_map(params, |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map(|r| r.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        let mut results = Vec::new();
        for (enc_payload, enc_search) in rows {
            let search_text = self.decrypt_field(&enc_search);
            if search_text.contains(query_lower) {
                let decrypted = self.decrypt_field(&enc_payload);
                if let Ok(val) = serde_json::from_str::<Value>(&decrypted) {
                    results.push(val);
                }
                if results.len() >= limit {
                    break;
                }
            }
        }
        results
    }

    fn like_pattern(query: &str) -> String {
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        format!("%{}%", escaped.to_lowercase())
    }

    fn search(&self, channel: &str, query: &str, limit: usize) -> Vec<Value> {
        let query_lower = query.to_lowercase();

        if self.pool.encryption_key.is_some() {
            self.search_encrypted(
                "SELECT payload, search_text FROM events
                 WHERE channel = ?1
                 ORDER BY ts DESC",
                params![channel],
                &query_lower,
                limit,
                "channel",
            )
        } else {
            let like = Self::like_pattern(query);
            self.query_events(
                "SELECT payload FROM events
                 WHERE channel = ?1 AND search_text LIKE ?2 ESCAPE '\\'
                 ORDER BY ts DESC
                 LIMIT ?3",
                params![channel, like, limit as i64],
            )
        }
    }

    fn dm_search(&self, username: &str, peer: &str, query: &str, limit: usize) -> Vec<Value> {
        let query_lower = query.to_lowercase();

        if self.pool.encryption_key.is_some() {
            self.search_encrypted(
                "SELECT payload, search_text FROM (
                     SELECT ts, payload, search_text FROM events
                     WHERE event_type = 'dm' AND sender = ?1 AND target = ?2
                     UNION ALL
                     SELECT ts, payload, search_text FROM events
                     WHERE event_type = 'dm' AND sender = ?2 AND target = ?1
                 )
                 ORDER BY ts DESC",
                params![username, peer],
                &query_lower,
                limit,
                "dm",
            )
        } else {
            let like = Self::like_pattern(query);
            self.query_events(
                "SELECT payload FROM (
                     SELECT ts, payload FROM events
                     WHERE event_type = 'dm'
                         AND sender = ?1 AND target = ?2
                         AND search_text LIKE ?3 ESCAPE '\\'
                     UNION ALL
                     SELECT ts, payload FROM events
                     WHERE event_type = 'dm'
                         AND sender = ?2 AND target = ?1
                         AND search_text LIKE ?3 ESCAPE '\\'
                 )
                 ORDER BY ts DESC
                 LIMIT ?4",
                params![username, peer, like, limit as i64],
            )
        }
    }

    fn rewind(&self, channel: &str, seconds: u64, limit: usize) -> Vec<Value> {
        let cutoff = (now() - seconds as f64).max(0.0);
        self.query_events(
            "SELECT payload FROM events
             WHERE channel = ?1 AND ts >= ?2
             ORDER BY ts DESC
             LIMIT ?3",
            params![channel, cutoff, limit as i64],
        )
    }

    fn load_user_2fa(&self, username: &str) -> Option<User2FA> {
        let conn = self.get_connection()?;
        let row = conn
            .query_row(
                "SELECT enabled, secret, backup_codes, enabled_at, last_verified
                 FROM user_2fa
                 WHERE username = ?1",
                params![username],
                |row| {
                    Ok((
                        row.get::<_, bool>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                        row.get::<_, Option<f64>>(4)?,
                    ))
                },
            )
            .optional();

        let Ok(Some((enabled, secret, backup_codes_json, enabled_at, last_verified))) = row else {
            return None;
        };

        let backup_codes = backup_codes_json
            .as_deref()
            .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
            .unwrap_or_default();

        let totp_config = secret.map(|secret| TotpConfig {
            secret,
            digits: 6,
            step: 30,
            algorithm: "SHA256".to_string(),
        });

        Some(User2FA {
            username: username.to_string(),
            enabled,
            totp_config,
            backup_codes,
            enabled_at,
            last_verified,
        })
    }

    fn upsert_user_2fa(&self, user: &User2FA) {
        let Some(conn) = self.get_connection() else {
            return;
        };

        let secret = user.totp_config.as_ref().map(|cfg| cfg.secret.clone());
        let backup_codes_json =
            serde_json::to_string(&user.backup_codes).unwrap_or_else(|_| "[]".to_string());

        if let Err(e) = conn.execute(
            "INSERT INTO user_2fa(username, enabled, secret, backup_codes, enabled_at, last_verified)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(username) DO UPDATE SET
                 enabled       = excluded.enabled,
                 secret        = excluded.secret,
                 backup_codes  = excluded.backup_codes,
                 enabled_at    = excluded.enabled_at,
                 last_verified = excluded.last_verified",
            params![
                user.username,
                user.enabled,
                secret,
                backup_codes_json,
                user.enabled_at,
                user.last_verified,
            ],
        ) {
            warn!("2fa upsert failed for user {}: {}", user.username, e);
        }
    }

    fn load_pw_hash(&self, username: &str) -> Result<Option<String>, &'static str> {
        let Some(conn) = self.get_connection() else {
            return Err("store_unavailable");
        };
        conn.query_row(
            "SELECT pw_hash FROM user_credentials WHERE username = ?1",
            params![username],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| {
            if let SqlError::SqliteFailure(_, Some(ref msg)) = e {
                if msg.contains("no such table: user_credentials") {
                    warn!(
                        "credential table missing for user '{}'; allowing compatibility auth path",
                        username
                    );
                    return "credentials_table_missing";
                }
            }
            warn!("credential lookup failed for user '{}': {}", username, e);
            "store_query_failed"
        })
    }

    fn upsert_credentials(&self, username: &str, pw_hash: &str) {
        let Some(conn) = self.get_connection() else {
            return;
        };
        let ts = now();
        if let Err(e) = conn.execute(
            "INSERT INTO user_credentials(username, pw_hash, created_at, updated_at, login_count, last_login)
             VALUES(?1, ?2, ?3, ?3, 1, ?3)
             ON CONFLICT(username) DO UPDATE SET
                 updated_at  = excluded.updated_at,
                 login_count = login_count + 1,
                 last_login  = excluded.last_login",
            params![username, pw_hash, ts],
        ) {
            warn!("credential upsert failed for user {}: {}", username, e);
        }
    }

    fn verify_credential(
        &self,
        username: &str,
        submitted_hash: &str,
    ) -> Result<bool, &'static str> {
        match self.load_pw_hash(username) {
            Ok(None) => Err("first_login"),
            Ok(Some(stored)) => Ok(crypto::pw_verify(submitted_hash, &stored)),
            Err("credentials_table_missing") => Err("first_login"),
            Err(e) => Err(e),
        }
    }

    fn get_user_role(&self, username: &str, channel: &str) -> Option<Role> {
        let conn = self.get_connection()?;
        let result: rusqlite::Result<RoleRow> = conn.query_row(
            "SELECT r.id, r.name, r.level, r.can_kick, r.can_ban, r.can_mute, r.can_manage, r.can_pin
             FROM roles r
             JOIN user_roles ur ON r.id = ur.role_id
             WHERE ur.username = ?1 AND ur.channel = ?2",
            params![username, channel],
            |row| Ok((
                row.get(0)?, row.get(1)?, row.get(2)?,
                row.get::<_, i32>(3)? != 0,
                row.get::<_, i32>(4)? != 0,
                row.get::<_, i32>(5)? != 0,
                row.get::<_, i32>(6)? != 0,
                row.get::<_, i32>(7)? != 0,
            )),
        );

        match result {
            Ok((id, name, level, can_kick, can_ban, can_mute, can_manage, can_pin)) => Some(Role {
                id,
                name,
                level,
                permissions: RolePermissions::from_db_row(
                    can_kick, can_ban, can_mute, can_manage, can_pin,
                ),
            }),
            Err(_) => None,
        }
    }

    fn get_default_role(&self) -> Option<Role> {
        let conn = self.get_connection()?;
        let result: rusqlite::Result<RoleRow> = conn.query_row(
            "SELECT id, name, level, can_kick, can_ban, can_mute, can_manage, can_pin FROM roles WHERE name = 'member'",
            [],
            |row| Ok((
                row.get(0)?, row.get(1)?, row.get(2)?,
                row.get::<_, i32>(3)? != 0,
                row.get::<_, i32>(4)? != 0,
                row.get::<_, i32>(5)? != 0,
                row.get::<_, i32>(6)? != 0,
                row.get::<_, i32>(7)? != 0,
            )),
        );

        match result {
            Ok((id, name, level, can_kick, can_ban, can_mute, can_manage, can_pin)) => Some(Role {
                id,
                name,
                level,
                permissions: RolePermissions::from_db_row(
                    can_kick, can_ban, can_mute, can_manage, can_pin,
                ),
            }),
            Err(_) => None,
        }
    }

    fn assign_role(
        &self,
        username: &str,
        channel: &str,
        role_name: &str,
        assigned_by: &str,
    ) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        let role_id = conn
            .query_row(
                "SELECT id FROM roles WHERE name = ?1",
                params![role_name],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|_| format!("role '{}' not found", role_name))?;

        conn.execute(
            "INSERT INTO user_roles (username, channel, role_id, assigned_by, assigned_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(username, channel) DO UPDATE SET role_id = ?3, assigned_by = ?4, assigned_at = ?5",
            params![username, channel, role_id, assigned_by, crate::now()],
        ).map_err(|e| format!("failed to assign role: {}", e))?;

        Ok(())
    }

    fn remove_user_role(&self, username: &str, channel: &str) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        conn.execute(
            "DELETE FROM user_roles WHERE username = ?1 AND channel = ?2",
            params![username, channel],
        )
        .map_err(|e| format!("failed to remove role: {}", e))?;

        Ok(())
    }

    fn ban_user(
        &self,
        username: &str,
        channel: &str,
        banned_by: &str,
        reason: Option<&str>,
        duration_secs: Option<i64>,
    ) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        let expires_at = duration_secs.map(|secs| crate::now() + secs as f64);

        conn.execute(
            "INSERT INTO bans (username, channel, banned_by, reason, banned_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(username, channel) DO UPDATE SET
                banned_by = ?3, reason = ?4, banned_at = ?5, expires_at = ?6",
            params![
                username,
                channel,
                banned_by,
                reason,
                crate::now(),
                expires_at
            ],
        )
        .map_err(|e| format!("failed to ban user: {}", e))?;

        Ok(())
    }

    fn unban_user(&self, username: &str, channel: &str) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        conn.execute(
            "DELETE FROM bans WHERE username = ?1 AND channel = ?2",
            params![username, channel],
        )
        .map_err(|e| format!("failed to unban user: {}", e))?;

        Ok(())
    }

    fn is_banned(&self, username: &str, channel: &str) -> Option<Ban> {
        let conn = self.get_connection()?;

        let result: rusqlite::Result<BanMuteRow> = conn.query_row(
            "SELECT username, channel, banned_by, reason, banned_at, expires_at
             FROM bans WHERE username = ?1 AND channel = ?2",
            params![username, channel],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        );

        match result {
            Ok((username, channel, banned_by, reason, banned_at, expires_at)) => Some(Ban {
                username,
                channel,
                banned_by,
                reason,
                banned_at,
                expires_at,
            }),
            Err(_) => None,
        }
    }

    fn mute_user(
        &self,
        username: &str,
        channel: &str,
        muted_by: &str,
        reason: Option<&str>,
        duration_secs: Option<i64>,
    ) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        let expires_at = duration_secs.map(|secs| crate::now() + secs as f64);

        conn.execute(
            "INSERT INTO mutes (username, channel, muted_by, reason, muted_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(username, channel) DO UPDATE SET
                muted_by = ?3, reason = ?4, muted_at = ?5, expires_at = ?6",
            params![
                username,
                channel,
                muted_by,
                reason,
                crate::now(),
                expires_at
            ],
        )
        .map_err(|e| format!("failed to mute user: {}", e))?;

        Ok(())
    }

    fn unmute_user(&self, username: &str, channel: &str) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        conn.execute(
            "DELETE FROM mutes WHERE username = ?1 AND channel = ?2",
            params![username, channel],
        )
        .map_err(|e| format!("failed to unmute user: {}", e))?;

        Ok(())
    }

    fn is_muted(&self, username: &str, channel: &str) -> Option<Mute> {
        let conn = self.get_connection()?;

        let result: rusqlite::Result<BanMuteRow> = conn.query_row(
            "SELECT username, channel, muted_by, reason, muted_at, expires_at
             FROM mutes WHERE username = ?1 AND channel = ?2",
            params![username, channel],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        );

        match result {
            Ok((username, channel, muted_by, reason, muted_at, expires_at)) => Some(Mute {
                username,
                channel,
                muted_by,
                reason,
                muted_at,
                expires_at,
            }),
            Err(_) => None,
        }
    }

    // -------------------------------------------------------------------------
    // Audit Logging
    // -------------------------------------------------------------------------

    fn log_audit(
        &self,
        action: &str,
        actor: &str,
        target: Option<&str>,
        channel: Option<&str>,
        reason: Option<&str>,
        metadata: Option<&str>,
    ) {
        if let Some(conn) = self.get_connection() {
            let ts = crate::now();
            if let Err(e) = conn.execute(
                "INSERT INTO audit_logs (action, actor, target, channel, reason, metadata, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![action, actor, target, channel, reason, metadata, ts],
            ) {
                warn!("audit log insert failed: {}", e);
            }
        }
    }

    fn get_audit_logs(
        &self,
        filter: Option<&str>,
        filter_value: Option<&str>,
        limit: i64,
    ) -> Vec<AuditLog> {
        let conn = match self.get_connection() {
            Some(c) => c,
            None => return vec![],
        };

        match (filter, filter_value) {
            (Some("user"), Some(user)) => {
                let mut stmt = match conn.prepare(
                    "SELECT action, actor, target, channel, reason, metadata, ts
                     FROM audit_logs
                     WHERE actor = ?2 OR target = ?2
                     ORDER BY ts DESC
                     LIMIT ?1",
                ) {
                    Ok(s) => s,
                    Err(_) => return vec![],
                };

                stmt.query_map(params![limit, user], row_to_audit_log)
                    .map(|rows| rows.filter_map(|row| row.ok()).collect())
                    .unwrap_or_default()
            }
            (Some("channel"), Some(channel)) => {
                let mut stmt = match conn.prepare(
                    "SELECT action, actor, target, channel, reason, metadata, ts
                     FROM audit_logs
                     WHERE channel = ?2
                     ORDER BY ts DESC
                     LIMIT ?1",
                ) {
                    Ok(s) => s,
                    Err(_) => return vec![],
                };

                stmt.query_map(params![limit, channel], row_to_audit_log)
                    .map(|rows| rows.filter_map(|row| row.ok()).collect())
                    .unwrap_or_default()
            }
            (Some("channel"), None) => {
                let mut stmt = match conn.prepare(
                    "SELECT action, actor, target, channel, reason, metadata, ts
                     FROM audit_logs
                     WHERE channel IS NOT NULL
                     ORDER BY ts DESC
                     LIMIT ?1",
                ) {
                    Ok(s) => s,
                    Err(_) => return vec![],
                };

                stmt.query_map(params![limit], row_to_audit_log)
                    .map(|rows| rows.filter_map(|row| row.ok()).collect())
                    .unwrap_or_default()
            }
            _ => {
                let mut stmt = match conn.prepare(
                    "SELECT action, actor, target, channel, reason, metadata, ts
                     FROM audit_logs
                     ORDER BY ts DESC
                     LIMIT ?1",
                ) {
                    Ok(s) => s,
                    Err(_) => return vec![],
                };

                stmt.query_map(params![limit], row_to_audit_log)
                    .map(|rows| rows.filter_map(|row| row.ok()).collect())
                    .unwrap_or_default()
            }
        }
    }

    // -------------------------------------------------------------------------
    // Account Lockout
    // -------------------------------------------------------------------------

    fn get_lockout_status(&self, username: &str) -> Option<(i32, f64)> {
        let conn = self.get_connection()?;
        let result: rusqlite::Result<(i32, f64)> = conn.query_row(
            "SELECT failed_attempts, locked_until FROM user_credentials WHERE username = ?1",
            params![username],
            |row| Ok((row.get::<_, i32>(0)?, row.get::<_, f64>(1)?)),
        );
        match result {
            Ok((failed, locked)) => Some((failed, locked)),
            Err(_) => None,
        }
    }

    fn record_failed_login(&self, username: &str, max_attempts: i32) -> (bool, i32) {
        let mut conn = match self.get_connection() {
            Some(c) => c,
            None => return (false, 0),
        };

        let tx = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(e) => {
                warn!("failed to start lockout transaction: {}", e);
                return (false, 0);
            }
        };

        let previous_attempts = match tx
            .query_row(
                "SELECT failed_attempts FROM user_credentials WHERE username = ?1",
                params![username],
                |row| row.get::<_, i32>(0),
            )
            .optional()
        {
            Ok(v) => v.unwrap_or(0),
            Err(e) => {
                warn!("failed to load lockout status: {}", e);
                return (false, 0);
            }
        };

        let new_attempts = previous_attempts + 1;
        let locked_until = if new_attempts >= max_attempts {
            crate::now() + 900.0
        } else {
            0.0
        };

        if let Err(e) = tx.execute(
            "UPDATE user_credentials SET failed_attempts = ?1, locked_until = ?2 WHERE username = ?3",
            params![new_attempts, locked_until, username],
        ) {
            warn!("failed to record failed login: {}", e);
            return (false, new_attempts);
        }

        if let Err(e) = tx.commit() {
            warn!("failed to commit lockout transaction: {}", e);
            return (false, new_attempts);
        }

        (locked_until > 0.0, new_attempts)
    }

    fn clear_failed_logins(&self, username: &str) {
        if let Some(conn) = self.get_connection() {
            if let Err(e) = conn.execute(
                "UPDATE user_credentials SET failed_attempts = 0, locked_until = 0 WHERE username = ?1",
                params![username],
            ) {
                warn!("failed to clear failed logins: {}", e);
            }
        }
    }

    fn unlock_account(&self, username: &str) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        conn.execute(
            "UPDATE user_credentials SET failed_attempts = 0, locked_until = 0 WHERE username = ?1",
            params![username],
        )
        .map_err(|e| format!("failed to unlock account: {}", e))?;

        Ok(())
    }

    // -------------------------------------------------------------------------
    // Suspicious Activity
    // -------------------------------------------------------------------------

    fn log_suspicious_activity(
        &self,
        target: &str,
        activity_type: &str,
        severity: &str,
        details: Option<&str>,
    ) {
        if let Some(conn) = self.get_connection() {
            let ts = crate::now();
            if let Err(e) = conn.execute(
                "INSERT INTO suspicious_activity (target_username, activity_type, severity, details, ts)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![target, activity_type, severity, details, ts],
            ) {
                warn!("suspicious activity log failed: {}", e);
            }
        }
    }

    fn get_recent_activity_count(
        &self,
        username: &str,
        activity_type: &str,
        window_secs: f64,
    ) -> i64 {
        let conn = match self.get_connection() {
            Some(c) => c,
            None => return 0,
        };

        let cutoff = crate::now() - window_secs;
        let result: rusqlite::Result<i64> = conn.query_row(
            "SELECT COUNT(*) FROM suspicious_activity WHERE target_username = ?1 AND activity_type = ?2 AND ts > ?3",
            params![username, activity_type, cutoff],
            |row| row.get(0),
        );

        result.unwrap_or(0)
    }

    #[allow(dead_code)]
    fn resolve_suspicious_activity(&self, id: i64, resolved_by: &str) -> Result<(), String> {
        let conn = self
            .get_connection()
            .ok_or("database connection unavailable")?;

        conn.execute(
            "UPDATE suspicious_activity SET resolved = TRUE, resolved_by = ?1, resolved_at = ?2 WHERE id = ?3",
            params![resolved_by, crate::now(), id],
        ).map_err(|e| format!("failed to resolve suspicious activity: {}", e))?;

        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct AuditLog {
    pub action: String,
    pub actor: String,
    pub target: Option<String>,
    pub channel: Option<String>,
    pub reason: Option<String>,
    pub metadata: Option<String>,
    pub ts: f64,
}

// ---------------------------------------------------------------------------
// Channel Ã¢â‚¬â€ in-memory broadcast + history ring buffer
// ---------------------------------------------------------------------------
// Channel ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â in-memory broadcast + history ring buffer
// ---------------------------------------------------------------------------

/// A named chat channel consisting of a bounded in-memory history ring buffer
/// and a [tokio broadcast] channel for real-time fan-out to all subscribers.
///
/// `Channel` is cheap to clone; all clones share the same `Arc`-wrapped
/// history and the same `broadcast::Sender` handle. New subscribers obtain a
/// fresh `Receiver` via `tx.subscribe()`.
#[derive(Clone)]
struct Channel {
    /// In-memory ring buffer of the last [`HISTORY_CAP`] messages.
    /// Wrapped in `Arc<RwLock<ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦>>` so multiple tasks can read concurrently
    /// while writes are exclusive.
    history: Arc<RwLock<VecDeque<Value>>>,

    /// Broadcast sender. The channel capacity (256) is deliberately larger
    /// than [`HISTORY_CAP`] to absorb short bursts without dropping frames.
    tx: broadcast::Sender<String>,
}

impl Channel {
    /// Creates a new, empty channel with a 256-message broadcast buffer.
    fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            history: Arc::new(RwLock::new(VecDeque::with_capacity(HISTORY_CAP))),
            tx,
        }
    }

    /// Appends `entry` to the in-memory history, evicting the oldest entry if
    /// the ring buffer is at capacity.
    async fn push(&self, entry: Value) {
        let mut h = self.history.write().await;
        if h.len() >= HISTORY_CAP {
            h.pop_front();
        }
        h.push_back(entry);
    }

    /// Returns a snapshot of the current history as a `Vec`, oldest first.
    async fn hist(&self) -> Vec<Value> {
        self.history.read().await.iter().cloned().collect()
    }
}

// ---------------------------------------------------------------------------
// Bridge tracking
// ---------------------------------------------------------------------------

/// Metadata for a connected bridge (e.g. Discord ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬Â Chatify).
/// Stored in `State::bridges` for the lifetime of the connection.
#[derive(Clone)]
struct BridgeInfo {
    /// The bridge's username on this server.
    username: String,
    /// Bridge type identifier (e.g. "discord").
    bridge_type: String,
    /// Instance ID for loop prevention (e.g. "discord-bridge:prod-1").
    instance_id: String,
    /// Unix timestamp (seconds) when the bridge connected.
    connected_at: f64,
    /// Number of routes configured on the bridge side.
    route_count: usize,
}

// ---------------------------------------------------------------------------
// State ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â shared, thread-safe server state
// ---------------------------------------------------------------------------

/// Central server state shared by all connection handler tasks via `Arc`.
///
/// Every field uses a lock-free concurrent map ([`DashMap`]) or atomic
/// primitive so that individual operations (insert, remove, lookup) do not
/// require global locking. Per-channel operations that require exclusive
/// history access use `tokio::sync::RwLock` scoped to the specific channel.
struct State {
    /// Named public channels, keyed by sanitised channel name.
    /// DM channels live here too under the `__dm__<username>` naming
    /// convention; they are filtered out when listing channels to clients.
    channels: DashMap<String, Channel>,

    /// Per-room voice broadcast senders, keyed by room name.
    voice: DashMap<String, broadcast::Sender<String>>,

    /// Current status value for each online user
    /// (e.g. `{"text":"Online","emoji":"ÃƒÂ°Ã…Â¸Ã…Â¸Ã‚Â¢"}`).
    /// Presence in this map is the authoritative signal that a user is online.
    user_statuses: DashMap<String, Value>,

    /// Public key (base64) for each online user, used by clients to encrypt
    /// DM payloads without a separate key-exchange round-trip.
    user_pubkeys: DashMap<String, String>,

    /// Per-user ring buffer of recently seen nonce values.
    /// Bounded to [`NONCE_CACHE_CAP`] entries; the oldest entry is evicted
    /// once the cap is reached. See [`validate_and_register_nonce`].
    recent_nonces: DashMap<String, VecDeque<String>>,

    /// Last-seen timestamp for each user's nonce cache entry.
    /// Updated on every nonce validation. Used by the periodic cleanup
    /// task to evict stale entries from `recent_nonces` when a user's
    /// connection drops without proper cleanup (crash, network partition).
    nonce_last_seen: DashMap<String, f64>,

    /// Number of WebSocket connections currently open. Managed via
    /// [`ConnectionGuard`] RAII to guarantee accurate accounting even on
    /// panics.
    active_connections: AtomicUsize,

    /// Notified whenever `active_connections` reaches zero, allowing the
    /// graceful-shutdown loop to wake immediately rather than polling.
    drained_notify: Notify,

    /// SQLite-backed event persistence and 2-FA storage.
    store: EventStore,

    /// Per-IP connection count for rate limiting.
    /// Incremented on TCP accept, decremented on disconnect.
    ip_connections: DashMap<std::net::IpAddr, usize>,

    /// Per-IP last auth attempt timestamp.
    /// Used to enforce a minimum interval between auth attempts.
    ip_last_auth: DashMap<std::net::IpAddr, f64>,

    /// Session tokens keyed by token string ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ username.
    /// Generated at auth time and validated on every post-auth frame.
    session_tokens: DashMap<String, String>,

    /// Connected bridge instances, keyed by username.
    /// Populated during auth when the client sends `"bridge": true`.
    bridges: DashMap<String, BridgeInfo>,

    /// Internal metrics for runtime stats and debugging.
    metrics: PerfMetrics,

    /// Prometheus metrics for export.
    prometheus: Option<Arc<std::sync::Mutex<PrometheusMetrics>>>,

    message_cache: VecCache<Value>,

    /// Voice channel relay for managing voice rooms, members, and state
    voice_relay: VoiceRelay,

    /// Flag to signal graceful shutdown in progress.
    /// When true, server stops accepting new connections.
    shutdown_in_progress: std::sync::atomic::AtomicBool,

    /// Shutdown trigger for external signaling (SIGHUP, shutdown endpoint).
    shutdown_notify: Notify,

    /// Per-user message rate limiting: username -> (count, window_start).
    /// Uses DashMap for concurrent access without locking.
    user_msg_rate: DashMap<String, (u32, f64)>,

    /// Maximum messages per user per minute.
    max_msgs_per_minute: u32,

    /// Whether per-user rate limiting is enabled.
    user_rate_limit_enabled: bool,
}

impl State {
    /// Creates the initial server state, pre-populating the `"general"` channel.
    fn new(
        db_path: String,
        db_key: Option<Vec<u8>>,
        prometheus: Option<Arc<std::sync::Mutex<PrometheusMetrics>>>,
        max_msgs_per_minute: u32,
        user_rate_limit_enabled: bool,
    ) -> Arc<Self> {
        let store = EventStore::new(db_path, db_key);
        let s = Arc::new(Self {
            channels: DashMap::new(),
            voice: DashMap::new(),
            user_statuses: DashMap::new(),
            user_pubkeys: DashMap::new(),
            recent_nonces: DashMap::new(),
            nonce_last_seen: DashMap::new(),
            active_connections: AtomicUsize::new(0),
            drained_notify: Notify::new(),
            store,
            ip_connections: DashMap::new(),
            ip_last_auth: DashMap::new(),
            session_tokens: DashMap::new(),
            bridges: DashMap::new(),
            metrics: PerfMetrics::new(),
            prometheus,
            message_cache: VecCache::new(1000),
            voice_relay: VoiceRelay::new(),
            shutdown_in_progress: std::sync::atomic::AtomicBool::new(false),
            shutdown_notify: Notify::new(),
            user_msg_rate: DashMap::new(),
            max_msgs_per_minute,
            user_rate_limit_enabled,
        });
        s.channels.insert("general".into(), Channel::new());
        s
    }

    /// Removes stale nonce cache entries for users whose last activity is
    /// older than `max_age_secs`.
    ///
    /// This is the safety net for connections that drop without proper cleanup
    /// (crash, kernel panic, network partition). The timestamp check in
    /// `validate_timestamp_skew` already rejects frames older than
    /// `MAX_CLOCK_SKEW_SECS`, so entries beyond that window are unreachable
    /// and safe to evict.
    fn evict_stale_nonce_entries(&self, max_age_secs: f64) -> usize {
        let cutoff = crate::now() - max_age_secs;
        let stale_keys: Vec<String> = self
            .nonce_last_seen
            .iter()
            .filter_map(|entry| {
                if *entry.value() < cutoff {
                    Some(entry.key().clone())
                } else {
                    None
                }
            })
            .collect();

        let count = stale_keys.len();
        for key in &stale_keys {
            self.nonce_last_seen.remove(key);
            self.recent_nonces.remove(key);
        }
        count
    }

    /// Returns the [`Channel`] for `name`, creating it lazily on first access.
    fn chan(&self, name: &str) -> Channel {
        self.channels
            .entry(name.into())
            .or_insert_with(Channel::new)
            .clone()
    }

    /// Returns the voice broadcast sender for `room`, creating it lazily on
    /// first access. The `_` receiver returned by `broadcast::channel` is
    /// immediately dropped ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â active subscribers obtain their own receivers
    /// via `vtx.subscribe()` when they call `"vjoin"`.
    fn voice_tx(&self, room: &str) -> broadcast::Sender<String> {
        self.voice
            .entry(room.into())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(128);
                tx
            })
            .clone()
    }

    /// Returns the number of currently online users (users with an active
    /// WebSocket connection that has completed auth).
    fn online_count(&self) -> usize {
        self.user_statuses.len()
    }

    /// Serialises the list of public (non-DM) channel names as a JSON array.
    fn channels_json(&self) -> Value {
        Value::Array(
            self.channels
                .iter()
                .filter(|e| !e.key().starts_with("__dm__"))
                .map(|e| Value::String(e.key().clone()))
                .collect(),
        )
    }

    /// Serialises the list of online users with their public keys as a JSON
    /// array of `{"u": "...", "pk": "..."}` objects.
    ///
    /// This is included in the `"ok"` auth response so clients can populate
    /// their local key stores without making a separate `"users"` request.
    fn users_with_keys_json(&self) -> Value {
        Value::Array(
            self.user_pubkeys
                .iter()
                .map(|e| serde_json::json!({"u": e.key().clone(), "pk": e.value().clone()}))
                .collect(),
        )
    }

    fn connection_opened(&self) {
        self.active_connections.fetch_add(1, Ordering::SeqCst);
        self.metrics.inc_accepted();
        if let Some(ref m) = self.prometheus {
            if let Ok(mutex_guard) = m.lock() {
                mutex_guard.record_connection_accepted();
            }
        }
    }

    /// Decrements the connection counter. If the counter reaches zero, notifies
    /// the [`drained_notify`](Self::drained_notify) condition variable so the
    /// graceful-shutdown loop can wake immediately.
    fn connection_closed(&self) {
        let prev = self.active_connections.fetch_sub(1, Ordering::SeqCst);
        self.metrics.inc_closed();
        if let Some(ref m) = self.prometheus {
            if let Ok(mutex_guard) = m.lock() {
                mutex_guard.record_connection_closed();
            }
        }
        if prev <= 1 {
            self.drained_notify.notify_waiters();
        }
    }

    fn active_connection_count(&self) -> usize {
        self.active_connections.load(Ordering::SeqCst)
    }

    // -----------------------------------------------------------------------
    // Rate limiting
    // -----------------------------------------------------------------------

    /// Increments the per-IP connection counter. Returns `false` if the
    /// IP has exceeded [`MAX_CONNECTIONS_PER_IP`].
    fn ip_connect(&self, addr: &SocketAddr) -> bool {
        let ip = addr.ip();
        let mut entry = self.ip_connections.entry(ip).or_insert(0);
        if *entry >= MAX_CONNECTIONS_PER_IP {
            return false;
        }
        *entry += 1;
        true
    }

    /// Decrements the per-IP connection counter, removing the entry if it
    /// reaches zero.
    fn ip_disconnect(&self, addr: &SocketAddr) {
        let ip = addr.ip();
        if let Some(mut entry) = self.ip_connections.get_mut(&ip) {
            *entry = entry.saturating_sub(1);
            if *entry == 0 {
                drop(entry);
                self.ip_connections.remove(&ip);
            }
        }
    }

    /// Checks whether an auth attempt from `addr` is allowed under the
    /// per-IP rate limit. If allowed, updates the last-auth timestamp.
    fn ip_auth_allowed(&self, addr: &SocketAddr) -> bool {
        let ip = addr.ip();
        let now = crate::now();
        if let Some(last) = self.ip_last_auth.get(&ip) {
            if now - *last < AUTH_RATE_LIMIT_SECS {
                return false;
            }
        }
        self.ip_last_auth.insert(ip, now);
        true
    }

    /// Check and record per-user message rate limit.
    /// Returns (allowed, remaining, reset_in_secs).
    fn check_user_rate_limit(&self, username: &str) -> (bool, u32, u64) {
        if !self.user_rate_limit_enabled {
            return (true, self.max_msgs_per_minute, 60);
        }

        if self.max_msgs_per_minute == 0 {
            return (true, u32::MAX, 60);
        }

        let now = crate::now();
        let window_secs = 60.0;

        let mut entry = self
            .user_msg_rate
            .entry(username.to_string())
            .or_insert((0, now));

        if now - entry.1 >= window_secs {
            entry.0 = 1;
            entry.1 = now;
            return (true, self.max_msgs_per_minute - 1, 60);
        }

        if entry.0 >= self.max_msgs_per_minute {
            let reset_in = (window_secs - (now - entry.1)) as u64;
            return (false, 0, reset_in);
        }

        entry.0 += 1;
        let remaining = self.max_msgs_per_minute - entry.0;
        let reset_in = (window_secs - (now - entry.1)) as u64;
        (true, remaining, reset_in)
    }

    // -----------------------------------------------------------------------
    // Graceful shutdown
    // -----------------------------------------------------------------------

    /// Signal the server to begin graceful shutdown.
    /// Returns true if shutdown was initiated, false if already shutting down.
    fn initiate_shutdown(&self) -> bool {
        self.shutdown_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Check if server is shutting down.
    fn is_shutting_down(&self) -> bool {
        self.shutdown_in_progress.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    fn notify_shutdown(&self) {
        self.shutdown_notify.notify_waiters();
    }

    // -----------------------------------------------------------------------
    // Session tokens
    // -----------------------------------------------------------------------

    /// Generates a cryptographically random session token and associates it
    /// with `username`. Returns the token string.
    fn create_session(&self, username: &str) -> String {
        use rand::{rngs::OsRng, RngCore};
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let token = hex::encode(bytes);
        self.session_tokens
            .insert(token.clone(), username.to_string());
        token
    }

    /// Removes the session token associated with `username`.
    fn remove_session(&self, username: &str) {
        self.session_tokens.retain(|_, v| v != username);
    }

    /// Check if user can send messages in a channel (checks bans, mutes, and permissions)
    fn can_send(&self, username: &str, channel: &str) -> bool {
        if let Some(ban) = self.store.is_banned(username, channel) {
            if ban.is_active() {
                return false;
            }
        }
        if let Some(mute) = self.store.is_muted(username, channel) {
            if mute.is_active() {
                return false;
            }
        }
        // By default, users can send messages unless they have no role AND permissions are restricted
        let role = self
            .store
            .get_user_role(username, channel)
            .or_else(|| self.store.get_default_role());
        match role {
            Some(r) => r.permissions.contains(RolePermissions::SEND),
            None => true, // Default allow for backwards compatibility
        }
    }

    /// Check if user can kick others in a channel
    fn can_kick(&self, username: &str, channel: &str) -> bool {
        let role = self
            .store
            .get_user_role(username, channel)
            .or_else(|| self.store.get_default_role());

        match role {
            Some(r) => r.can_kick(),
            None => false,
        }
    }

    /// Check if user can ban others in a channel
    fn can_ban(&self, username: &str, channel: &str) -> bool {
        let role = self
            .store
            .get_user_role(username, channel)
            .or_else(|| self.store.get_default_role());

        match role {
            Some(r) => r.can_ban(),
            None => false,
        }
    }

    /// Check if user can mute others in a channel
    fn can_mute(&self, username: &str, channel: &str) -> bool {
        let role = self
            .store
            .get_user_role(username, channel)
            .or_else(|| self.store.get_default_role());

        match role {
            Some(r) => r.can_mute(),
            None => false,
        }
    }

    /// Get user's role in a channel
    fn get_user_role(&self, username: &str, channel: &str) -> Option<String> {
        self.store.get_user_role(username, channel).map(|r| r.name)
    }

    /// Check if user is banned in a channel
    fn is_banned(&self, username: &str, channel: &str) -> bool {
        self.store
            .is_banned(username, channel)
            .map(|b| b.is_active())
            .unwrap_or(false)
    }

    /// Check if user is muted in a channel
    fn is_muted(&self, username: &str, channel: &str) -> bool {
        self.store
            .is_muted(username, channel)
            .map(|m| m.is_active())
            .unwrap_or(false)
    }

    // -----------------------------------------------------------------------
    // Suspicious Activity Detection
    // -----------------------------------------------------------------------

    const SPAM_MSG_THRESHOLD: usize = 30;
    const SPAM_WINDOW_SECS: f64 = 60.0;
    const RAID_JOIN_THRESHOLD: usize = 10;
    const RAID_WINDOW_SECS: f64 = 60.0;

    fn check_and_alert_spam(&self, username: &str, _channel: &str) {
        self.store.log_suspicious_activity(
            username,
            "spam",
            "low",
            Some("message activity recorded"),
        );

        let recent = self
            .store
            .get_recent_activity_count(username, "spam", Self::SPAM_WINDOW_SECS);
        if recent < Self::SPAM_MSG_THRESHOLD as i64 {
            return;
        }

        let already_alerted =
            self.store
                .get_recent_activity_count(username, "spam_alert", Self::SPAM_WINDOW_SECS);
        if already_alerted > 0 {
            return;
        }

        self.store.log_suspicious_activity(
            username,
            "spam_alert",
            "medium",
            Some(&format!(
                "user sent {} messages in {}s window",
                recent,
                Self::SPAM_WINDOW_SECS
            )),
        );
    }

    fn check_and_alert_raid(&self, username: &str, _channel: &str) {
        self.store.log_suspicious_activity(
            username,
            "raid_join",
            "low",
            Some("channel join activity recorded"),
        );

        let recent =
            self.store
                .get_recent_activity_count(username, "raid_join", Self::RAID_WINDOW_SECS);
        if recent < Self::RAID_JOIN_THRESHOLD as i64 {
            return;
        }

        let already_alerted =
            self.store
                .get_recent_activity_count(username, "raid_alert", Self::RAID_WINDOW_SECS);
        if already_alerted > 0 {
            return;
        }

        self.store.log_suspicious_activity(
            username,
            "raid_alert",
            "high",
            Some(&format!(
                "user joined {} channels in {}s window",
                recent,
                Self::RAID_WINDOW_SECS
            )),
        );
        self.broadcast_alert(
            "raid",
            username,
            "high",
            &format!(
                "Possible raid detected: user {} joining multiple channels rapidly",
                username
            ),
        );
    }

    fn broadcast_alert(&self, alert_type: &str, target: &str, severity: &str, message: &str) {
        let alert = serde_json::json!({
            "t": "alert",
            "alert_type": alert_type,
            "target": target,
            "severity": severity,
            "message": message,
            "ts": now()
        })
        .to_string();

        for channel in self.channels.iter() {
            let _ = channel.tx.send(alert.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// RAII connection tracking guard
// ---------------------------------------------------------------------------

/// RAII guard that increments [`State::active_connections`] on construction
/// and decrements it on drop, guaranteeing accurate accounting even when a
/// connection handler panics or returns early.
struct ConnectionGuard {
    state: Arc<State>,
    addr: SocketAddr,
}

impl ConnectionGuard {
    fn new(state: Arc<State>, addr: SocketAddr) -> Self {
        state.connection_opened();
        Self { state, addr }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.state.ip_disconnect(&self.addr);
        self.state.connection_closed();
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Clamps a raw `limit` parameter from a client request to `[1, max]`,
/// substituting `default` when the field is absent.
///
/// This prevents clients from requesting zero or unreasonably large result
/// sets while still allowing the server to apply sensible per-endpoint
/// maximums without duplicating clamping logic in each handler.
fn clamp_limit(raw: Option<u64>, default: usize, max: usize) -> usize {
    raw.map(|v| v as usize).unwrap_or(default).clamp(1, max)
}

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
fn send_out_json(out_tx: &mpsc::UnboundedSender<String>, payload: Value) {
    let _ = out_tx.send(payload.to_string());
}

#[allow(dead_code)]
fn send_out_json_with_metrics(
    out_tx: &mpsc::UnboundedSender<String>,
    payload: Value,
    metrics: &PerfMetrics,
) {
    let serialized = payload.to_string();
    let bytes = serialized.len();
    metrics.inc_sent(1);
    metrics.inc_bytes_sent(bytes);
    let _ = out_tx.send(serialized);
}

fn send_err(
    out_tx: &mpsc::UnboundedSender<String>,
    msg: impl std::fmt::Display,
    metrics: &PerfMetrics,
) {
    let payload = serde_json::json!({"t":"err","m":msg.to_string()});
    let serialized = payload.to_string();
    let bytes = serialized.len();
    metrics.inc_sent(1);
    metrics.inc_bytes_sent(bytes);
    let _ = out_tx.send(serialized);
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
    out_tx: mpsc::UnboundedSender<String>,
) {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(m) => {
                    if out_tx.send(m).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(_) => {} // Lagged: skip and continue
            }
        }
    });
}

fn spawn_voice_relay_forwarder(
    mut rx: broadcast::Receiver<VoiceBroadcast>,
    out_tx: mpsc::UnboundedSender<String>,
) {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let json = serde_json::to_string(&event).unwrap_or_default();
                    if out_tx.send(json).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(_) => {}
            }
        }
    });
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
/// | `ping`        | Heartbeat ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â server replies with `pong`              |
/// | `edit`        | Edit a previously sent message (in-memory only)     |
/// | `file_meta`   | Announce a file transfer to a channel               |
/// | `file_chunk`  | Stream a chunk of a file transfer                   |
/// | `typing`      | Broadcast typing state for channel or DM scope      |
/// | `status`      | Update the caller's presence status                 |
/// | `reaction`    | Add an emoji reaction to a message                  |
/// | `2fa_setup`   | Begin the TOTP enrollment flow                      |
/// | `2fa_enable`  | Finalise TOTP enrollment with a verification code   |
/// | `2fa_disable` | Disable 2-FA for the current user                   |
/// | `2fa_verify`  | Verify a TOTP or backup code post-auth              |
///
/// Unknown event types are silently ignored.
async fn handle_event(
    d: &Value,
    state: &Arc<State>,
    username: &str,
    out_tx: &mpsc::UnboundedSender<String>,
    voice_room: &mut Option<String>,
) {
    let t = d["t"].as_str().unwrap_or("");
    let event_channel = d
        .get("ch")
        .or_else(|| d.get("r"))
        .and_then(|v| v.as_str())
        .map(safe_ch);

    if let Some(ch) = event_channel.as_deref() {
        info!("event user={} type={} channel={}", username, t, ch);
    } else {
        info!("event user={} type={}", username, t);
    }

    // --- Replay protection (timestamp skew + nonce dedup) ------------------
    // Only applied to mutating events (see requires_fresh_protection).
    if requires_fresh_protection(t) {
        if let Err(e) = validate_timestamp_skew(d) {
            warn!(
                "protocol validation failed user={} type={} reason={}",
                username, t, e
            );
            send_err(
                out_tx,
                format!("protocol validation failed: {}", e),
                &state.metrics,
            );
            return;
        }
        if let Err(e) = validate_and_register_nonce(state, username, d) {
            warn!(
                "protocol validation failed user={} type={} reason={}",
                username, t, e
            );
            send_err(
                out_tx,
                format!("protocol validation failed: {}", e),
                &state.metrics,
            );
            return;
        }
    }

    // --- Event dispatch switch ---------------------------------------------
    match t {
        "msg" => {
            // Broadcast an encrypted message to a channel.
            // `"c"` is the ciphertext blob; `"p"` is optional plaintext for
            // the search index only Ã¢â‚¬â€ it is never echoed back to clients.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let c = d["c"].as_str().unwrap_or("");
            let p = d["p"].as_str().unwrap_or("");
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
            let msg_id = clifford::fresh_nonce_hex();
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
            let serialized = entry.to_string();
            let chan = state.chan(&ch);
            chan.push(entry.clone()).await;
            let searchable = if p.is_empty() { c } else { p };
            state
                .store
                .persist("msg", &ch, username, None, &entry, searchable);
            let _ = chan.tx.send(serialized);

            // Update message cache for this channel
            state
                .message_cache
                .push_and_trim(&format!("h:{}", ch), entry, 50);

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
                &ptxt,
            );
            let _ = state.chan(&dm_channel_name(&target)).tx.send(p.clone());
            let _ = state.chan(&dm_channel_name(username)).tx.send(p.clone());

            // Update DM cache for both parties
            state.message_cache.push_and_trim(
                &format!("dm:{}:{}", username, target),
                event.clone(),
                50,
            );
            state
                .message_cache
                .push_and_trim(&format!("dm:{}:{}", target, username), event, 50);
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
            spawn_broadcast_forwarder(chan.tx.subscribe(), out_tx.clone());
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
                                    cached.iter().take(limit).cloned().collect();
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
                                    cached.iter().take(limit).cloned().collect();
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
                "event=bridge_status_requested user={} bridge_count={}",
                username,
                bridges.len()
            );
        }
        "metrics" => {
            let snapshot = state.metrics.snapshot();
            let cache_stats = state.message_cache.stats();
            let pool_stats = state.store.get_pool_stats();
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
                    "ts": now(),
                }),
            );
        }
        "vjoin" => {
            // Subscribe to a voice room's broadcast channel.
            // A system message is posted to the room's text channel to
            // notify other members.
            let room = safe_ch(d["r"].as_str().unwrap_or("general"));

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
                    "room": room,
                    "members": members,
                    "joined": true,
                    "ts": now()
                }),
            );

            let vtx = state.voice_tx(&room);
            spawn_broadcast_forwarder(vtx.subscribe(), out_tx.clone());
            spawn_voice_relay_forwarder(state.voice_relay.subscribe(), out_tx.clone());
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
            if let Some(ref room) = voice_room.take() {
                state.voice_relay.leave_room(room, username);

                let leave_voice = serde_json::json!({
                    "t":"sys",
                    "m":format!("ÃƒÂ°Ã…Â¸Ã…Â½Ã¢â€žÂ¢ {} left voice #{}", username, room),
                    "ts":now()
                });
                state.store.persist(
                    "sys",
                    room,
                    username,
                    None,
                    &leave_voice,
                    &format!("{} voice left", username),
                );
                let _ = state.chan(room).tx.send(leave_voice.to_string());
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
            if a.is_empty() {
                return;
            }
            if let Some(ref room) = voice_room {
                if let Some(vtx) = state.voice.get(room) {
                    let _ = vtx
                        .send(serde_json::json!({"t":"vdata","from":username,"a":a}).to_string());
                }
            }
        }
        "ping" => {
            // Heartbeat ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â keep-alive for clients behind proxies with idle
            // connection timeouts.
            let _ = out_tx.send(r#"{"t":"pong"}"#.into());
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
            let filename = d["filename"]
                .as_str()
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

            let media_kind = match d["media_kind"]
                .as_str()
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

            let mut file_announce = serde_json::json!({
                "t":"file_meta","from":username,"filename":filename,
                "size":size,"file_id":file_id,"ch":ch,
                "media_kind":media_kind,"ts":now()
            });
            if let Some(mime_value) = mime {
                file_announce["mime"] = Value::String(mime_value);
            }
            state.store.persist(
                "file_meta",
                &ch,
                username,
                None,
                &file_announce,
                &format!("{} {}", media_kind, filename),
            );
            let _ = state.chan(&ch).tx.send(file_announce.to_string());
        }
        "file_chunk" => {
            // Relay a single chunk of a file transfer to the channel.
            // Chunks are not persisted ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â they are ephemeral relay frames.
            let ch = safe_ch(d["ch"].as_str().unwrap_or("general"));
            let file_id = d["file_id"].as_str().unwrap_or("").trim().to_string();
            let chunk_data = d["data"].as_str().unwrap_or("").to_string();
            if file_id.is_empty() || chunk_data.is_empty() {
                return;
            }
            let index = d["index"].as_u64().unwrap_or(0);
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
            if let Some(status_val) = d.get("status") {
                state
                    .user_statuses
                    .insert(username.to_string(), status_val.clone());
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
            user_2fa.enable(secret);
            if !user_2fa.verify_totp(&code) {
                send_err(out_tx, "invalid 2FA code", &state.metrics);
                return;
            }

            state.store.upsert_user_2fa(&user_2fa);
            send_out_json(
                out_tx,
                serde_json::json!({
                    "t":"2fa_enabled","enabled":true,
                    "backup_codes":user_2fa.backup_codes,"ts":now()
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
                    if !state.can_kick(username, &channel) {
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
                    if !state.can_kick(username, &channel) {
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

            if !state.can_ban(username, "general") {
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
    clifford::now()
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
    clifford::normalize_channel(raw).unwrap_or_else(|| "general".into())
}

fn row_to_audit_log(row: &rusqlite::Row) -> rusqlite::Result<AuditLog> {
    Ok(AuditLog {
        action: row.get(0)?,
        actor: row.get(1)?,
        target: row.get(2)?,
        channel: row.get(3)?,
        reason: row.get(4)?,
        metadata: row.get(5)?,
        ts: row.get(6)?,
    })
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
                    Ok((mut stream, addr)) => {
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
                                    warn!("Failed to read from health connection {}: {}", addr, e);
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
fn create_ok_response(username: &str, state: &Arc<State>, hist: Vec<Value>) -> String {
    serde_json::json!({
        "t": "ok",
        "u": username,
        "users": state.users_with_keys_json(),
        "channels": state.channels_json(),
        "hist": hist,
        "proto": {
            "v": PROTOCOL_VERSION,
            "max_payload_bytes": MAX_BYTES
        }
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Input validation
// ---------------------------------------------------------------------------

/// Returns `true` if `name` is a valid username.
///
/// Valid usernames are non-empty, at most [`MAX_USERNAME_LEN`] characters,
/// and consist entirely of ASCII alphanumeric characters, `-`, or `_`.
/// Whitespace, punctuation, and Unicode are rejected to keep usernames safe
/// for use as map keys, log fields, and SQL parameters.
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

    let pw = d
        .get("pw")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ChatifyError::Validation("missing password hash".to_string()))?;
    if pw.is_empty() || pw.len() > MAX_PASSWORD_FIELD_LEN {
        return Err(ChatifyError::Validation(
            "invalid password hash".to_string(),
        ));
    }

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
        pw_hash: pw.to_string(),
        status,
        pubkey,
        otp_code,
        is_bridge,
        bridge_type,
        bridge_instance_id,
        bridge_routes,
    })
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
/// `"info"`, `"ping"`) and control events (`"join"`, `"vjoin"`) are excluded
/// because replaying them is either idempotent or harmless.
fn requires_fresh_protection(event_type: &str) -> bool {
    matches!(
        event_type,
        "msg"
            | "img"
            | "dm"
            | "vdata"
            | "edit"
            | "file_meta"
            | "file_chunk"
            | "status"
            | "reaction"
    )
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
        // Legacy clients may omit nonce/timestamp on mutating frames.
        return Ok(());
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
        // Legacy clients may omit nonces.
        return Ok(());
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
/// `mpsc::unbounded_channel` and drained by a dedicated sink-writer task.
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
        warn!(
            "connection rejected: too many connections from {}",
            addr.ip()
        );
        // Best-effort: the stream may not support WebSocket yet, but try.
        if let Ok(ws) = accept_async(stream).await {
            let (mut sink, _) = ws.split();
            let _ = sink
                .send(Message::Text(
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
            debug!("WebSocket handshake failed from {}: {}", addr, e);
            return;
        }
    };
    let (mut sink, mut stream) = ws.split();

    // ---- Phase 1: read and validate the auth frame --------------------------

    let raw = match stream.next().await {
        Some(Ok(Message::Text(r))) => r,
        Some(Ok(_)) => {
            let _ = sink
                .send(Message::Text(
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
            .send(Message::Text(
                serde_json::json!({"t":"err","m":"auth frame too large"}).to_string(),
            ))
            .await;
        return;
    }

    let d: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({"t":"err","m":"invalid auth JSON"}).to_string(),
                ))
                .await;
            return;
        }
    };

    let auth = match validate_auth_payload(&d) {
        Ok(a) => a,
        Err(err) => {
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({"t":"err","m":err.to_string()}).to_string(),
                ))
                .await;
            return;
        }
    };

    // --- Per-IP auth rate limiting ---
    if !state.ip_auth_allowed(&addr) {
        warn!("auth rate limited from {}", addr.ip());
        let _ = sink
            .send(Message::Text(
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
        pw_hash,
        status,
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
            let remaining = (locked_until - crate::now()) as i32;
            let _ = sink
                .send(Message::Text(
                    serde_json::json!({
                        "t": "err",
                        "m": format!("account locked, try again in {} seconds", remaining),
                        "locked": true,
                        "retry_after": remaining
                    })
                    .to_string(),
                ))
                .await;
            warn!("auth blocked: account locked for user={}", username);
            return;
        }
    }

    // --- Credential verification ---
    // The client sends a PBKDF2 hash of their password. The server stores
    // its own PBKDF2 hash of that value (with a random salt). This two-layer
    // approach means the server never sees the raw password, but also never
    // trusts a client-provided hash blindly.
    match state.store.verify_credential(&username, &pw_hash) {
        Ok(true) => {
            state.store.clear_failed_logins(&username);
        } // Hash matches ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â proceed.
        Ok(false) => {
            let (locked, attempts) = state
                .store
                .record_failed_login(&username, MAX_FAILED_ATTEMPTS);
            if locked {
                state.store.log_suspicious_activity(
                    &username,
                    "brute_force",
                    "high",
                    Some(&format!("{} failed login attempts", attempts)),
                );
                let _ = sink
                    .send(Message::Text(
                        serde_json::json!({
                            "t":"err",
                            "m":"account locked due to too many failed attempts",
                            "locked": true,
                            "retry_after": 900
                        })
                        .to_string(),
                    ))
                    .await;
            } else {
                let remaining = MAX_FAILED_ATTEMPTS - attempts;
                let _ = sink
                    .send(Message::Text(
                        serde_json::json!({
                            "t":"err",
                            "m":"invalid credentials",
                            "remaining_attempts": remaining
                        })
                        .to_string(),
                    ))
                    .await;
            }
            warn!(
                "auth failed: invalid password for user={}, attempts={}",
                username, attempts
            );
            return;
        }
        Err("first_login") => {
            // First time this username connects ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â store their credential.
            // The submitted hash is itself a PBKDF2 output, so we wrap it
            // in another salted PBKDF2 layer server-side.
            let server_hash = crypto::pw_hash(&pw_hash);
            state.store.upsert_credentials(&username, &server_hash);
            info!("credentials created for new user={}", username);
        }
        Err(e) => {
            let _ = sink
                .send(Message::Text(
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
            .send(Message::Text(
                serde_json::json!({"t":"err","m":"username already in use"}).to_string(),
            ))
            .await;
        warn!("auth rejected: username '{}' already connected", username);
        return;
    }

    if let Err(err) = enforce_2fa_on_auth(&state, &username, otp_code.as_deref()) {
        let _ = sink
            .send(Message::Text(
                serde_json::json!({"t":"err","m":err.to_string()}).to_string(),
            ))
            .await;
        return;
    }

    // Generate a session token for this connection.
    let _session_token = state.create_session(&username);

    // ---- Phase 2: register user and send welcome response -------------------

    state.user_statuses.insert(username.clone(), status);
    state.user_pubkeys.insert(username.clone(), pubkey);

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
            "event=bridge_connected bridge_type={} instance_id={} user={} routes={}",
            bridge_type, bridge_instance_id, username, bridge_routes
        );
    }

    // Subscribe to "general" before sending "ok" to avoid missing messages
    // that arrive between the response send and the subscription.
    let general = state.chan("general");
    let gen_rx = general.tx.subscribe();
    let mut hist = state.store.history("general", HISTORY_CAP);
    if hist.is_empty() {
        hist = general.hist().await;
    }

    let ok = create_ok_response(&username, &state, hist);
    if sink.send(Message::Text(ok)).await.is_err() {
        return;
    }

    broadcast_system_msg(&state, &format!("ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ {} joined", username)).await;
    info!("+ {}", username);

    // ---- Phase 3: set up bidirectional message routing ----------------------

    // mpsc channel: all tasks that want to send to this client queue here.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Forward "general" broadcast to the outbound queue.
    spawn_broadcast_forwarder(gen_rx, out_tx.clone());

    // Sink writer task: drains out_rx and writes to the WebSocket sink.
    // Runs until out_rx is closed (out_tx is dropped at function exit).
    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(Message::Text(m)).await.is_err() {
                break;
            }
        }
    });

    // ---- Phase 4: main event loop -------------------------------------------

    // Tracks the current voice room so vleave / vdata know which room to act on.
    let mut voice_room: Option<String> = None;

    loop {
        let msg = match stream.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => {
                info!("ws recv error for {}: {}", username, e);
                break;
            }
            None => break, // Client closed the connection cleanly.
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

        handle_event(&d, &state, &username, &out_tx, &mut voice_room).await;
    }

    // ---- Phase 5: cleanup ---------------------------------------------------
    // Remove user presence so they no longer appear in the user directory.
    state.user_statuses.remove(&username);
    state.user_pubkeys.remove(&username);
    // Invalidate the session token.
    state.remove_session(&username);
    // Clear the nonce cache to free memory; replays from this session are
    // no longer possible once the connection is closed.
    state.recent_nonces.remove(&username);
    state.nonce_last_seen.remove(&username);
    if state.bridges.remove(&username).is_some() {
        info!("event=bridge_disconnected user={}", username);
    }
    broadcast_system_msg(&state, &format!("ÃƒÂ¢Ã…â€œÃ¢â‚¬â€œ {} left", username)).await;
    info!("- {}", username);
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
    // Load certificate chain
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

    // Load private key
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

    // 4. Generate a new key and write it to disk.
    use rand::{rngs::OsRng, RngCore};
    let mut key = [0u8; 32];
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
                                        warn!("TLS handshake failed from {}: {}", addr, e);
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
                                        warn!("TLS handshake failed from {}: {}", addr, e);
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

    let listener = TcpListener::bind(&addr).await?;
    let state = State::new(
        args.db.clone(),
        db_key,
        metrics.clone(),
        args.max_msgs_per_minute,
        args.enable_user_rate_limit,
    );

    let enc_label = if state.store.is_encrypted() {
        "ChaCha20-Poly1305"
    } else {
        "None (unencrypted)"
    };
    let proto = if tls_acceptor.is_some() { "wss" } else { "ws" };
    println!(" Chatify running on {}://{}", proto, addr);
    println!(" Encryption: {} |   IP Privacy: On", enc_label);
    println!(" Event store: {}", args.db);
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

    #[cfg(unix)]
    {
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
        accept_loop_unix(listener, state, tls_acceptor, shutdown_rx, sighup_rx, &args).await;
    }

    #[cfg(not(unix))]
    {
        accept_loop(listener, state.clone(), tls_acceptor, shutdown_rx, &args).await;
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

    /// Verifies that [`validate_auth_payload`] returns a `ChatifyError::Validation`
    /// variant (not `Message`) for an invalid username, allowing callers to
    /// distinguish validation errors from protocol errors.
    ///
    /// The specific error message `"invalid username"` is part of the public
    /// error contract and must not change without updating client-side error
    /// handling.
    #[test]
    fn auth_payload_rejects_invalid_username_with_typed_error() {
        let payload = serde_json::json!({
            "t": "auth",
            "u": "bad user",  // space is not allowed
            "pw": "abc123",
            "pk": base64::engine::general_purpose::STANDARD.encode([0u8; 32])
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
        let state = State::new(":memory:".to_string(), None, None, 60, false);
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
}
