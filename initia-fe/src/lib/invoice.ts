import { bech32m } from '@scure/base';
import type { PaymentInvoice } from '../types';

const INVOICE_VERSION: '1' = '1';
const INVOICE_HRP = 'unipay';
const BECH32_MAX_CHARS = 1800;

function randomHex(size = 4): string {
  const buffer = new Uint8Array(size);
  if (typeof window !== 'undefined' && window.crypto?.getRandomValues) {
    window.crypto.getRandomValues(buffer);
  } else {
    for (let i = 0; i < size; i += 1) {
      buffer[i] = Math.floor(Math.random() * 256);
    }
  }
  return Array.from(buffer).map((value) => value.toString(16).padStart(2, '0')).join('');
}

type InvoicePayload = {
  v: string;
  r: string;
  c: string;
  t: string;
  a: string;
  i?: string;
};

function toNormalizedInvoice(payload: {
  version: string;
  recipient: string;
  destChain: string;
  destTokenId: string;
  destAmount: string;
  invoiceId?: string;
}): PaymentInvoice | null {
  const recipient = payload.recipient.trim();
  const destChain = payload.destChain.trim();
  const destTokenId = payload.destTokenId.trim();
  const destAmount = normalizeInvoiceAmount(payload.destAmount);

  if (payload.version !== INVOICE_VERSION) return null;
  if (!recipient || !destChain || !destTokenId || !destAmount) return null;

  return {
    version: INVOICE_VERSION,
    recipient,
    destChain,
    destTokenId,
    destAmount,
    invoiceId: payload.invoiceId?.trim(),
  };
}

function toCanonicalPayload(raw: unknown): InvoicePayload | null {
  if (!raw || typeof raw !== 'object') return null;

  const typed = raw as Record<string, unknown>;
  const version = typed.version ?? typed.v;
  const recipient = typed.recipient ?? typed.r;
  const destChain = typed.destChain ?? typed.c;
  const destTokenId = typed.destTokenId ?? typed.t;
  const destAmount = typed.destAmount ?? typed.a;
  const invoiceId = typed.invoiceId ?? typed.i;

  if (typeof version !== 'string' || typeof recipient !== 'string' || typeof destChain !== 'string'
    || typeof destTokenId !== 'string' || (typeof destAmount !== 'string' && typeof destAmount !== 'number')
    || (typeof invoiceId !== 'undefined' && typeof invoiceId !== 'string')) {
    return null;
  }

  return {
    v: version,
    r: recipient,
    c: destChain,
    t: destTokenId,
    a: String(destAmount),
    i: invoiceId,
  };
}

function serializePayload(payload: InvoicePayload): string {
  const normalized = JSON.stringify(payload);
  const bytes = new TextEncoder().encode(normalized);
  const words = bech32m.toWords(bytes);
  return bech32m.encode(INVOICE_HRP, words, BECH32_MAX_CHARS);
}

function deserializePayload(invoiceId: string): PaymentInvoice | null {
  try {
    const lower = invoiceId.toLowerCase();
    if (!lower.includes('1')) return null;
    const { prefix, words } = bech32m.decode(lower as `${string}1${string}`, BECH32_MAX_CHARS);
    if (prefix !== INVOICE_HRP) return null;

    const bytes = new Uint8Array(bech32m.fromWords(words));
    const decoded = new TextDecoder().decode(bytes);
    const raw = JSON.parse(decoded);
    const canonical = toCanonicalPayload(raw);
    if (!canonical) return null;
    return toNormalizedInvoice({
      version: canonical.v,
      recipient: canonical.r,
      destChain: canonical.c,
      destTokenId: canonical.t,
      destAmount: canonical.a,
      invoiceId: canonical.i,
    });
  } catch {
    return null;
  }
}

function normalizeInvoiceAmount(amount: string): string {
  const trimmed = amount.trim();
  const parsed = Number(trimmed);
  if (!trimmed || !Number.isFinite(parsed) || parsed <= 0) return '';
  return trimmed;
}

export function generateInvoiceId(): string {
  return `inv_${Date.now().toString(36)}_${randomHex(4)}${randomHex(4)}`;
}

export function parsePaymentInvoice(search: string): PaymentInvoice | null {
  const params = new URLSearchParams(search);
  const invoiceParam = params.get('invoice')?.trim();
  if (invoiceParam && invoiceParam !== INVOICE_VERSION) {
    const decoded = parsePaymentInvoiceCode(invoiceParam);
    if (decoded) return decoded;
  }

  const recipient = params.get('recipient')?.trim() ?? '';
  const destChain = params.get('destChain')?.trim() ?? '';
  const destTokenId = (params.get('destToken')?.trim() ?? params.get('destTokenId')?.trim() ?? '').trim();
  const destAmount = normalizeInvoiceAmount(params.get('destAmount') ?? '');
  const hasLegacyShape = Boolean(recipient || destChain || destTokenId || destAmount);

  if (!hasLegacyShape) return null;

  if (invoiceParam && invoiceParam !== INVOICE_VERSION) {
    return null;
  }

  const normalized = toNormalizedInvoice({
    version: INVOICE_VERSION,
    recipient,
    destChain,
    destTokenId,
    destAmount,
  });
  if (!normalized) return null;

  return normalized;
}

export function parsePaymentInvoiceCode(invoiceId: string): PaymentInvoice | null {
  const trimmed = invoiceId.trim().toLowerCase();
  if (!trimmed) return null;
  return deserializePayload(trimmed);
}

export function parsePaymentInvoiceFromInput(value: string): PaymentInvoice | null {
  const trimmed = value.trim();
  if (!trimmed) return null;

  if (trimmed.includes('?')) {
    const queryStart = trimmed.indexOf('?');
    const parsed = parsePaymentInvoice(trimmed.slice(queryStart));
    if (parsed) return parsed;
  }

  if (trimmed.includes('=')) {
    const parsed = parsePaymentInvoice(`?${trimmed}`);
    if (parsed) return parsed;
  }

  return parsePaymentInvoiceCode(trimmed);
}

export function buildPaymentInvoiceCode(invoice: PaymentInvoice): string {
  const normalized = toNormalizedInvoice(invoice);
  if (!normalized) throw new Error('Invalid invoice');

  const payload: InvoicePayload = {
    v: INVOICE_VERSION,
    r: normalized.recipient,
    c: normalized.destChain,
    t: normalized.destTokenId,
    a: normalized.destAmount,
    i: normalized.invoiceId || generateInvoiceId(),
  };

  return serializePayload(payload);
}

export function buildPaymentInvoiceUrl(invoice: PaymentInvoice): string {
  const baseUrl = (
    import.meta.env.VITE_PUBLIC_APP_URL
    ?? (typeof window !== 'undefined' ? window.location.href : '')
  ).trim();
  const url = new URL(baseUrl || 'http://localhost:5173');
  const invoiceCode = buildPaymentInvoiceCode(invoice);

  url.search = '';
  url.hash = '';
  url.searchParams.set('invoice', invoiceCode);

  return url.toString();
}
