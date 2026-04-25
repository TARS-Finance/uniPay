// Event signatures match the deployed HTLC contract exactly.
// Both refund() and instantRefund() emit Refunded — there is no separate InstantRefunded event.
alloy::sol! {
    event Initiated(bytes32 indexed orderID, bytes32 indexed secretHash, uint256 indexed amount);
    event Redeemed(bytes32 indexed orderID, bytes32 indexed secretHash, bytes secret);
    event Refunded(bytes32 indexed orderID);
}
