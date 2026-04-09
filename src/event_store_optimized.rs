// EventStore — SQLite persistence layer with connection pooling
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
    path: String,
    encryption_key: Option<Vec<u8>>,
}

impl PooledConnection {
    fn new(path: String, encryption_key: Option<Vec<u8>>) -> rusqlite::Result<Self> {
        Ok(Self {
            path,
            encryption_key,
        })
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

    fn has_broken_connection(&self, _conn: &mut Self::Connection) -> bool {
        false
    }

    fn timeout(&self) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_secs(5))
    }
}

#[derive(Clone)]
struct DbPool {
    pool: r2d2::Pool<PooledConnection>,
    path: String,
    encryption_key: Option<Vec<u8>>,
}

impl DbPool {
    fn new(path: String, encryption_key: Option<Vec<u8>>) -> Result<Self, r2d2::Error> {
        let manager = PooledConnection::new(path.clone(), encryption_key.clone())?;
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
}

impl EventStore {
    fn new(path: String, encryption_key: Option<Vec<u8>>) -> Self {
        let pool = DbPool::new(path.clone(), encryption_key.clone())
            .expect("failed to create database pool");
        let store = Self { pool };
        store.init().expect("failed to initialise event store — check database path, permissions, and encryption key");
        store
    }

    fn is_encrypted(&self) -> bool {
        self.pool.encryption_key.is_some()
    }

    fn init(&self) -> rusqlite::Result<()> {
        let conn = self.get_connection()?;
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

        if version > CURRENT_SCHEMA_VERSION {
            warn!(
                "Database schema version {} is newer than supported version {}",
                version, CURRENT_SCHEMA_VERSION
            );
        }

        Ok(())
    }

    fn encrypt_field(&self, plaintext: &str) -> Option<String> {
        if let Some(ref key) = self.pool.encryption_key {
            match crypto::enc_bytes(key, plaintext.as_bytes()) {
                Ok(ct) => Some(serde_json::json!({"ct": hex::encode(ct)}).to_string()),
                Err(e) => {
                    warn!("encryption failed; dropping persistence write: {}", e);
                    None
                }
            }
        } else {
            Some(plaintext.to_string())
        }
    }

    fn decrypt_field(&self, stored: &str) -> Option<String> {
        if let Some(ref key) = self.pool.encryption_key {
            let val = match serde_json::from_str::<serde_json::Value>(stored) {
                Ok(v) => v,
                Err(_) => {
                    // Backward-compatible read path for legacy plaintext rows.
                    // New writes remain fail-closed in encrypt_field.
                    warn!("legacy plaintext row encountered while encryption is enabled");
                    return Some(stored.to_string());
                }
            };
            let Some(ct_hex) = val.get("ct").and_then(|v| v.as_str()) else {
                warn!("legacy plaintext row encountered while encryption is enabled");
                return Some(stored.to_string());
            };
            let Ok(ct_bytes) = hex::decode(ct_hex) else {
                warn!("encrypted payload has invalid ciphertext encoding; dropping row");
                return None;
            };
            match crypto::dec_bytes(key, &ct_bytes) {
                Ok(pt) => Some(String::from_utf8_lossy(&pt).to_string()),
                Err(e) => {
                    warn!("decryption failed; dropping row: {}", e);
                    None
                }
            }
        } else {
            Some(stored.to_string())
        }
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
            .filter_map(|raw| self.decrypt_field(&raw))
            .collect();

        let mut out = Self::decode_rows(decrypted);
        out.reverse();
        out
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
        let Some(conn) = self.get_connection() else {
            return;
        };
        let payload_json = payload.to_string();
        let Some(enc_payload) = self.encrypt_field(&payload_json) else {
            warn!(
                "event persist dropped: type={} channel={} sender={} reason=payload_encrypt_failed",
                event_type, channel, sender
            );
            return;
        };
        let Some(enc_search) = self.encrypt_field(&search_text.to_lowercase()) else {
            warn!(
                "event persist dropped: type={} channel={} sender={} reason=search_encrypt_failed",
                event_type, channel, sender
            );
            return;
        };
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

    fn history(&self, channel: &str, limit: usize) -> Vec<Value> {
        self.query_events(
            "SELECT payload FROM events
             WHERE channel = ?1
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
            "SELECT payload FROM (
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?1 AND target = ?2
                 UNION ALL
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?2 AND target = ?1
             )
             ORDER BY ts DESC
             LIMIT ?3",
            params![username, peer, limit as i64],
        )
    }

    fn dm_rewind(&self, username: &str, peer: &str, seconds: u64, limit: usize) -> Vec<Value> {
        let cutoff = (now() - seconds as f64).max(0.0);
        self.query_events(
            "SELECT payload FROM (
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?1 AND target = ?2 AND ts >= ?3
                 UNION ALL
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?2 AND target = ?1 AND ts >= ?3
             )
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
            "SELECT payload FROM (
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?1 AND target = ?2 AND ts >= ?3
                 UNION ALL
                 SELECT ts, payload FROM events
                 WHERE event_type = 'dm' AND sender = ?2 AND target = ?1 AND ts >= ?3
             )
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
            let Some(search_text) = self.decrypt_field(&enc_search) else {
                continue;
            };
            if search_text.contains(query_lower) {
                let Some(decrypted) = self.decrypt_field(&enc_payload) else {
                    continue;
                };
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
}
