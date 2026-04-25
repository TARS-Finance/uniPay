export type PersonaType = 'customer' | 'merchant' | null;
export type QuoteMode = 'exact-in' | 'exact-out';

export type CustomerPage = 'pay' | 'history';
export type MerchantPage = 'overview' | 'pools' | 'activity' | 'earn';
export type AnyPage = CustomerPage | MerchantPage;

export interface PaymentInvoice {
  version: '1';
  recipient: string;
  destChain: string;
  destTokenId: string;
  destAmount: string;
  invoiceId?: string;
}

export interface PreparedPayment {
  quoteMode: QuoteMode;
  sourceAmountRaw: string;
  sourceAmountDisplay: string;
  destinationAmountRaw: string;
  destinationAmountDisplay: string;
  sourceAsset: string;
  destinationAsset: string;
  strategyId: string;
}

export interface Token {
  sym: string;
  name: string;
  klass: string;
  price: number;
}

export interface Chain {
  id: string;
  name: string;
  klass: string;
  short: string;
  explorer?: string;      // tx URL prefix
  addrExplorer?: string;  // address URL prefix
}

export type StepStatus = 'pending' | 'active' | 'done';

export interface HistoryTx {
  id: string;
  amount: number;
  token: string;
  chain: string;
  destChain?: string;
  destToken: string;
  status: 'Settled' | 'Pending' | 'Refunded';
  ts: number;
  initAmount: number;
  srcHash: string;
  initHash: string | null;
  swapId: string;   // deposit address for Bitcoin, swap_id otherwise

  // Transaction detail — optional fields populated from the matched-order payload.
  orderId?: string;
  srcInitiator?: string;
  srcRedeemer?: string;
  srcInitiateHash?: string;
  srcRedeemHash?: string;
  srcRefundHash?: string;
  dstInitiator?: string;
  dstRedeemer?: string;
  dstInitiateHash?: string;
  dstRedeemHash?: string;
  dstRefundHash?: string;
  refundAddr?: string;
  destinationAddr?: string;
  settleSecs?: number;
  source?: string;
  srcPrice?: number;
  dstPrice?: number;
  srcUnitPrice?: number;
  dstUnitPrice?: number;
}

export interface Settlement {
  id: string;
  amount: number;
  token: string;
  srcChain: string;
  ts: number;
  staked: boolean;
}

export interface Pool {
  id: string;
  name: string;
  tokens: [string, string];
  staked: number;
  apy: number;
  earned: number;
  chain: string;
  tvl: string;
}
