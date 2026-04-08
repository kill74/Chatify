//! Database layer - placeholder for incremental refactoring.

#[derive(Clone)]
pub struct DbPool {
    pub path: String,
    pub encryption_key: Option<Vec<u8>>,
    pub pool: (),
}

pub struct PooledConnection {
    pub path: String,
    pub encryption_key: Option<Vec<u8>>,
}

impl PooledConnection {
    pub fn new(path: String, encryption_key: Option<Vec<u8>>) -> Self {
        Self {
            path,
            encryption_key,
        }
    }
}

impl r2d2::ManageConnection for PooledConnection {
    type Connection = ();
    type Error = std::convert::Infallible;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        Ok(())
    }

    fn is_valid(&self, _conn: &mut Self::Connection) -> Result<(), Self::Error> {
        Ok(())
    }

    fn has_broken(&self, _conn: &mut Self::Connection) -> bool {
        false
    }
}

#[derive(Clone)]
pub struct EventStore {
    pub pool: DbPool,
}

impl EventStore {
    pub fn new(_path: String, _encryption_key: Option<Vec<u8>>) -> Self {
        Self {
            pool: DbPool {
                path: String::new(),
                encryption_key: None,
                pool: (),
            },
        }
    }

    pub fn is_encrypted(&self) -> bool {
        false
    }

    pub fn health_check(&self) -> bool {
        true
    }

    pub fn history(&self, _channel: &str, _limit: usize) -> Vec<serde_json::Value> {
        Vec::new()
    }
}
