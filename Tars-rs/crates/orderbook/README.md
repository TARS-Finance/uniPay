# Orderbook

Contains common data models and helper functions used in the unipay

## Available Functions

### Order Queries

-   `get_swap(order_id: &str, chain: SwapChain) -> Option<SingleSwap>`

    -   Retrieves swap details for a specific order and chain (source/destination)

-   `get_matched_order(create_id: &str) -> Option<MatchedOrderVerbose>`

    -   Retrieves a matched order by its creation ID

-   `get_unmatched_order(create_id: &str) -> Option<CreateOrder>`
    -   Retrieves an unmatched order by its creation ID

### Pagination Queries

-   `get_all_matched_orders(page_data: PageData) -> PaginatedData<MatchedOrderVerbose>`

    -   Retrieves all matched orders with pagination

-   `get_all_unmatched_orders(page_data: PageData) -> PaginatedData<CreateOrder>`

    -   Retrieves all unmatched orders with pagination

-   `get_matched_orders(user: &str, status: OrderStatus, page_data: PageData) -> PaginatedData<MatchedOrderVerbose>`

    -   Retrieves matched orders for a specific user with status (all, pending, fulfilled) filter and pagination

-   `get_unmatched_orders(user: &str, page_data: PageData) -> PaginatedData<CreateOrder>`
    -   Retrieves unmatched orders for a specific user with pagination

### Status Queries

-   `exists(secret_hash: &str) -> bool`

    -   Checks if an order exists with the given secret hash

-   `get_order_count(user: &str) -> i64`

    -   Gets total order count for a user

-   `get_total_locked_amount(addr: &str, chain: &str) -> BigDecimal`

    -   Gets total locked amount for an address on a specific chain

-   `get_solver_committed_funds(addr: &str, chain: &str, asset: &str,) -> BigDecimal`

    -   Gets the total amount yest to be initiated by solver

-   `wait_until_action(create_id: &str, action: MatchedOrderAction) -> MatchedOrderVerbose`
    -   Waits till an action on the order and Returns the matched order

### Pending & Refund Operations

-   `get_user_refundable_swaps() -> Vec<SingleSwap>`

    -   Retrieves all refundable swaps for users

-   `get_pending_orders() -> Vec<MatchedOrderVerbose>`
    -   Retrieves all pending orders that need processing

### SACP Operations

-   `add_instant_refund_sacp(order_id: &str, instant_refund_tx_bytes: &str)`

    -   Adds instant refund SACP data to an order

-   `add_redeem_sacp(order_id: &str, redeem_tx_bytes: &str, redeem_tx_id: &str, secret: &str)`
    -   Adds redeem SACP data and updates destination swap information

## Type References

For detailed type definitions, refer to `src/primitives.rs`

## Usage

```rust
use unipay::orderbook::{OrderbookProvider, primitives::{SwapChain, PageData}};

#[tokio::main]
async fn main() -> Result<()> {
    let provider = OrderbookProvider::from_db_url("postgres://...").await?;

    // Get a specific swap
    let swap = provider.get_swap("order_id", SwapChain::Source).await?;

    // Get paginated matched orders
    let page_data = PageData::new(1, 10)?;
    let matched_orders = provider.get_all_matched_orders(page_data).await?;
}
```
