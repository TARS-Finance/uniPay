CREATE TABLE bitcoin_wallet_requests (
    scope TEXT NOT NULL,
    dedupe_key TEXT NOT NULL,
    kind TEXT NOT NULL
        CHECK (kind IN ('send', 'spend')),
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'inflight', 'confirmed', 'dropped')),
    lineage_id TEXT,
    batch_txid TEXT,
    txid_history JSONB NOT NULL DEFAULT '[]'::jsonb,
    chain_anchor JSONB,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (scope, dedupe_key)
);

CREATE INDEX idx_bw_requests_status
    ON bitcoin_wallet_requests (scope, status);

CREATE INDEX idx_bw_requests_batch
    ON bitcoin_wallet_requests (scope, batch_txid)
    WHERE batch_txid IS NOT NULL;

CREATE INDEX idx_bw_requests_lineage
    ON bitcoin_wallet_requests (scope, lineage_id)
    WHERE lineage_id IS NOT NULL;

CREATE INDEX idx_bw_requests_chain_anchor
    ON bitcoin_wallet_requests (scope)
    WHERE chain_anchor IS NOT NULL AND status = 'pending';
