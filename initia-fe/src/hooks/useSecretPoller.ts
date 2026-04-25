import { useEffect } from 'react';
import { loadSecret, clearSecret, revealSecret } from '../lib/htlc';
import { QUOTE_API } from '../lib/config';

const SECRET_KEY_PREFIX = 'htlc.secret.';

function getStoredOrderIds(): string[] {
  return Object.keys(localStorage)
    .filter((k) => k.startsWith(SECRET_KEY_PREFIX))
    .map((k) => k.slice(SECRET_KEY_PREFIX.length));
}

async function pollOnce() {
  const orderIds = getStoredOrderIds();
  if (orderIds.length === 0) return;

  await Promise.allSettled(
    orderIds.map(async (oid) => {
      try {
        const res = await fetch(`${QUOTE_API}/orders/${oid}`);
        if (!res.ok) return;
        const json = await res.json();
        const s = json.data ?? json;

        const src = s.source_swap ?? {};
        const dst = s.destination_swap ?? {};

        const isDone =
          !!src.refund_tx_hash || !!dst.refund_tx_hash || !!src.redeem_tx_hash;

        if (isDone) {
          clearSecret(oid);
          return;
        }

        // cobi has locked on destination — reveal our secret
        // Do NOT clear here; wait until src.redeem_tx_hash confirms (isDone above)
        if (dst.initiate_tx_hash && !src.redeem_tx_hash) {
          const secretHex = loadSecret(oid);
          if (!secretHex) return;
          await revealSecret(oid, secretHex);
        }
      } catch {
        // transient; retry next tick
      }
    }),
  );
}

export function useSecretPoller() {
  useEffect(() => {
    pollOnce();
    const id = setInterval(pollOnce, 10_000);
    return () => clearInterval(id);
  }, []);
}
