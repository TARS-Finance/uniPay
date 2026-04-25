import { useState, useEffect, useRef } from 'react';
import { QUOTE_API } from '../lib/config';
import type { QuoteMode } from '../types';

export interface QuoteResult {
  sourceDisplay: string;        // e.g. "10.01"
  sourceAmount: string;         // raw string amount
  destinationDisplay: string;   // e.g. "119.12"
  destinationAmount: string;    // raw string amount
  feePercent: number;           // e.g. 30 (bips)
  estimatedTime: number;        // seconds
  inputTokenPrice: number;
  outputTokenPrice: number;
  strategyId: string;
  loading: boolean;
  error: string | null;
}

interface UseQuoteParams {
  from: string;
  to: string;
  mode?: QuoteMode;
  fromAmount?: string;
  toAmount?: string;
}

const EMPTY_QUOTE: QuoteResult = {
  sourceDisplay: '',
  sourceAmount: '',
  destinationDisplay: '',
  destinationAmount: '',
  feePercent: 0,
  estimatedTime: 0,
  inputTokenPrice: 0,
  outputTokenPrice: 0,
  strategyId: '',
  loading: false,
  error: null,
};

export function useQuote({
  from,
  to,
  mode = 'exact-in',
  fromAmount = '',
  toAmount = '',
}: UseQuoteParams): QuoteResult {
  const [result, setResult] = useState<QuoteResult>(EMPTY_QUOTE);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    // Clear any pending debounce
    if (debounceRef.current !== null) {
      clearTimeout(debounceRef.current);
    }

    const requestedAmount = mode === 'exact-out' ? toAmount : fromAmount;

    if (!from || !to) {
      setResult(EMPTY_QUOTE);
      return;
    }

    // Skip fetch for empty / zero amounts
    if (!requestedAmount || requestedAmount === '0') {
      setResult(EMPTY_QUOTE);
      return;
    }

    // Show loading immediately so the UI can respond
    setResult((prev) => ({ ...prev, loading: true, error: null }));

    debounceRef.current = setTimeout(async () => {
      try {
        const url = new URL(`${QUOTE_API}/quote`);
        url.searchParams.set('from', from);
        url.searchParams.set('to', to);
        url.searchParams.set(mode === 'exact-out' ? 'to_amount' : 'from_amount', requestedAmount);
        const res = await fetch(url);
        const json = await res.json();
        if (!res.ok) throw new Error(json?.error ?? `${res.status} ${res.statusText}`);

        if (!json.ok || !json.data?.best) {
          throw new Error('Invalid quote response');
        }

        const best = json.data.best;
        setResult({
          sourceDisplay: best.source?.display ?? '',
          sourceAmount: best.source?.amount ?? '',
          destinationDisplay: best.destination?.display ?? '',
          destinationAmount: best.destination?.amount ?? '',
          feePercent: best.fee ?? 0,
          estimatedTime: best.estimated_time ?? 0,
          inputTokenPrice: json.data.input_token_price ?? 0,
          outputTokenPrice: json.data.output_token_price ?? 0,
          strategyId: best.strategy_id ?? '',
          loading: false,
          error: null,
        });
      } catch (e) {
        setResult({
          ...EMPTY_QUOTE,
          loading: false,
          error: e instanceof Error ? e.message : String(e),
        });
      }
    }, 400);

    return () => {
      if (debounceRef.current !== null) {
        clearTimeout(debounceRef.current);
      }
    };
  }, [from, to, mode, fromAmount, toAmount]);

  return result;
}
