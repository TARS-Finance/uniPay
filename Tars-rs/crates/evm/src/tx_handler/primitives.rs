#[derive(Debug, PartialEq)]
pub enum TransactionState {
    Status(TransactionStatus),
    ReplacementNeeded,
}

#[derive(Debug, PartialEq)]
pub enum TransactionStatus {
    Confirmed,
    Reverted,
    NotFound,
    Pending,
}
