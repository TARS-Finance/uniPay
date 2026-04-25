#[derive(Debug, thiserror::Error)]
pub enum ChainError {
    #[error("{component} lock poisoned")]
    LockPoisoned { component: &'static str },
    #[error("chain {0} not registered")]
    NotRegistered(String),
    #[error("decode failed: {0}")]
    DecodeFailed(String),
    #[error("contract revert: {0}")]
    ContractRevert(String),
    #[error("simulation failed: {revert_data}")]
    SimulationFailed { revert_data: String },
    #[error("rpc error: {0}")]
    Rpc(String),
    #[error("tx timeout")]
    TxTimeout,
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("validation failed: {0}")]
    ValidationFailed(String),
    #[error("worker channel: {0}")]
    WorkerChannel(String),
    #[error("chain error: {0}")]
    Other(String),
}

#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("query error: {0}")]
    Query(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("connection error: {0}")]
    Connection(String),
    #[error("data corruption: {0}")]
    DataCorruption(String),
}

#[derive(Debug, thiserror::Error)]
pub enum VenueError {
    #[error("venue error: {0}")]
    Other(String),
}

pub trait Retryable {
    fn is_transient(&self) -> bool;
}

impl Retryable for ChainError {
    fn is_transient(&self) -> bool {
        matches!(
            self,
            ChainError::Rpc(_) | ChainError::TxTimeout | ChainError::WorkerChannel(_)
        )
    }
}

impl Retryable for PersistenceError {
    fn is_transient(&self) -> bool {
        matches!(self, PersistenceError::Connection(_))
    }
}

impl Retryable for VenueError {
    fn is_transient(&self) -> bool {
        false
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("chain error: {0}")]
    Chain(#[from] ChainError),
    #[error("persistence error: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("venue error: {0}")]
    Venue(#[from] VenueError),
    #[error("domain error: {0}")]
    Domain(String),
}

impl Retryable for ExecutorError {
    fn is_transient(&self) -> bool {
        match self {
            ExecutorError::Chain(error) => error.is_transient(),
            ExecutorError::Persistence(error) => error.is_transient(),
            ExecutorError::Venue(error) => error.is_transient(),
            ExecutorError::Domain(_) => false,
        }
    }
}
