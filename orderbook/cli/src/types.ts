export type HexString = string;

export interface ApiCreateRfqRequest {
  idempotency_key: string;
  source_asset: string;
  target_asset: string;
  source_amount: string;
  slippage: "auto" | number;
  refund_address: string;
  destination_address: string;
}

export interface ApiAcceptanceTerms {
  source_chain: string;
  htlc_contract_address: string;
  redeemer_address: string;
  timelock: number;
  source_amount: string;
  source_asset_contract: string | null;
  secret_hash_algorithm: string;
}

export interface ApiCreateRfqResponse {
  rfq_id: string;
  quote_id: string;
  status: "accepted" | "rejected" | string;
  reason?: string;
  reason_detail?: string;
  source_asset: string;
  target_asset: string;
  source_amount: string;
  estimated_target_amount: string;
  min_target_amount: string;
  max_slippage_bps: number;
  auto_slippage: boolean;
  execution_path: string;
  expires_at: string;
  acceptance_terms: ApiAcceptanceTerms;
}

export interface ApiQuoteRequest {
  from: string;
  to: string;
  from_amount?: string;
  to_amount?: string;
  affiliate_fee?: number;
  slippage?: number;
  strategy_id?: string;
}

export interface ApiQuoteAssetView {
  asset: string;
  amount: string;
  display: string;
  value: string;
}

export interface ApiQuoteRoute {
  strategy_id: string;
  source: ApiQuoteAssetView;
  destination: ApiQuoteAssetView;
  solver_id: string;
  estimated_time: number;
  slippage: number;
  fee: number;
  fixed_fee: string;
}

export interface ApiQuoteResponse {
  best: ApiQuoteRoute | null;
  routes: ApiQuoteRoute[];
  input_token_price: number;
  output_token_price: number;
}

export interface ApiCreateOrderRequest {
  from: string;
  to: string;
  from_amount?: string;
  to_amount?: string;
  initiator_source_address: string;
  initiator_destination_address: string;
  secret_hash: string;
  strategy_id?: string;
  affiliate_fee?: number;
  slippage?: number;
  nonce?: string;
  bitcoin_optional_recipient?: string;
  source_delegator?: string;
}

export interface ApiOrderSwap {
  swap_id: string;
  chain: string;
  asset: string;
  htlc_address: string | null;
  token_address: string | null;
  initiator: string;
  redeemer: string;
  timelock: number;
  amount: string;
  initiate_tx_hash: string | null;
}

export interface ApiOrderCreateOrder {
  create_id: string;
  source_chain: string;
  destination_chain: string;
  source_asset: string;
  destination_asset: string;
  source_amount: string;
  destination_amount: string;
  initiator_source_address: string;
  initiator_destination_address: string;
}

export interface ApiMatchedOrderResponse {
  created_at: string;
  updated_at: string;
  deleted_at: string | null;
  source_swap: ApiOrderSwap;
  destination_swap: ApiOrderSwap;
  create_order: ApiOrderCreateOrder;
}

export interface ApiEnvelope<T> {
  ok: boolean;
  data: T;
}

export interface ApiAcceptQuoteRequest {
  idempotency_key: string;
  quote_id: string;
  source_tx_hash: string;
  secret_hash: string;
  source_recipient?: string;
  destination_recipient?: string;
}

export interface ApiAcceptQuoteResponse {
  trade_id: string;
  quote_id: string;
  status: string;
  message: string;
}

export interface HtlcParams {
  initiator_address: string | null;
  redeemer_address: string | null;
  timelock: number | null;
  secret_hash: string | null;
  recipient_address: string | null;
}

export interface ApiHtlcSnapshot {
  chain: string;
  tx_hash: string;
  htlc_id: string | null;
  htlc_contract_address: string | null;
  refund_tx_hash: string | null;
  htlc_params?: HtlcParams | null;
}

export interface ApiCancellation {
  reason: string;
  cancel_signature: string | null;
  signature_type: string;
  issued_at: string | null;
}

export interface ApiTradeStatus {
  trade_id: string;
  quote_id: string;
  status: string;
  source_asset: string;
  target_asset: string;
  source_amount: string;
  final_target_amount: string | null;
  created_at: string;
  updated_at: string;
  inbound_htlc: ApiHtlcSnapshot | null;
  outbound_htlc: ApiHtlcSnapshot | null;
  cancellation: ApiCancellation | null;
  settled_at: string | null;
}

export const DEFAULT_TERMINAL_STATUSES = new Set([
  "settled",
  "cancelled",
  "tx_rejected",
  "abandoned",
  "source_timed_out",
  "destination_timed_out",
  "source_refunded",
  "destination_refunded",
]);

export type ChainKind = "evm" | "bitcoin";

export interface EvmChainConfig {
  type: "evm";
  chain_id: number;
  chain_name?: string;
  rpc_url: string;
  private_key: string;
}

export interface BitcoinChainConfig {
  type: "bitcoin";
  network: "bitcoin" | "bitcoin_testnet" | "bitcoin_signet" | "bitcoin_regtest";
  esplora_url: string;
  private_key: string;
}

export type ChainConfig = EvmChainConfig | BitcoinChainConfig;

export interface TradeConfig {
  source_asset: string;
  target_asset: string;
  source_amount: string;
  slippage: "auto" | number;
  refund_address: string;
  destination_address: string;
  source_recipient?: string;
  destination_recipient?: string;
}

export interface CliConfig {
  api_url: string;
  poll_interval_ms: number;
  poll_timeout_ms: number;
  chains: Record<string, ChainConfig>;
  trade: TradeConfig;
}

export interface CliArgs {
  config: string;
  sourceAsset?: string;
  targetAsset?: string;
  sourceAmount?: string;
  slippage?: "auto" | number;
}
