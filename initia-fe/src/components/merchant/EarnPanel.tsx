import { useEffect, useState } from 'react'
import { useInterwovenKit } from '@initia/interwovenkit-react'
import { formatUnits } from 'viem'
import { useMerchantPosition } from '../../lib/stacker'
import { useOpinitExecutorStatus } from '../../lib/opinit'
import { useRollupUsdcBalance } from '../../lib/usdc'
import { L1_TX_EXPLORER_URL } from '../../lib/config'
import { useWithdrawalTracker, type WithdrawalRecord } from '../../lib/withdrawal-tracker'

const ROLLUP_CHAIN_ID = import.meta.env.VITE_CHAIN_ID ?? 'tars-1'
const ROLLUP_USDC_DENOM = import.meta.env.VITE_USDC_ROLLUP_DENOM ?? 'uusdc'
const L1_CHAIN_ID = import.meta.env.VITE_L1_CHAIN_ID ?? 'initiation-2'
const L1_USDC_DENOM = import.meta.env.VITE_L1_USDC_DENOM ?? 'uusdc'
const ROLLUP_TX_EXPLORER_URL = import.meta.env.VITE_ROLLUP_TX_EXPLORER_URL
const MAX_SAFE_OUTPUT_LAG = 20
const DECIMALS = 6
const FALLBACK_FINALIZATION_MS = 5 * 60 * 1000

function formatUsdc(value: string | bigint) {
  return formatUnits(BigInt(value), DECIMALS).replace(/\.?0+$/, '') || '0'
}

function formatLag(value: number | null) {
  if (value === null) return null
  return value.toLocaleString('en-US')
}

function normalizeTxHash(hash: string) {
  return hash.replace(/^0x/i, '').toUpperCase()
}

function getRollupTxUrl(txHash: string) {
  if (ROLLUP_TX_EXPLORER_URL) {
    return `${ROLLUP_TX_EXPLORER_URL.replace(/\/$/, '')}/${txHash}`
  }
  if (ROLLUP_CHAIN_ID === 'utars-chain-1') {
    return `http://localhost:1317/cosmos/tx/v1beta1/txs/${txHash}`
  }
  return `https://scan.testnet.initia.xyz/${ROLLUP_CHAIN_ID}/txs/${txHash}`
}

function getL1TxUrl(txHash: string) {
  return `${L1_TX_EXPLORER_URL.replace(/\/$/, '')}/${txHash}`
}

function shortHash(hash: string) {
  if (hash.length <= 14) return hash
  return `${hash.slice(0, 8)}…${hash.slice(-5)}`
}

function elapsedLabel(submittedAt: number) {
  const seconds = Math.max(0, Math.floor((Date.now() - submittedAt) / 1000))
  if (seconds < 60) return `${seconds}s ago`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ago`
  const hours = Math.floor(minutes / 60)
  return `${hours}h ${minutes % 60}m ago`
}

function computeProgress(record: WithdrawalRecord): { pct: number; etaLabel: string } {
  if (record.status === 'finalized') return { pct: 100, etaLabel: '✓' }
  if (record.status === 'failed') return { pct: 0, etaLabel: '—' }

  const period = record.finalizationPeriodMs ?? FALLBACK_FINALIZATION_MS
  if (record.outputProposedAt) {
    const elapsed = Date.now() - record.outputProposedAt
    const ratio = Math.max(0, Math.min(1, elapsed / period))
    const remaining = Math.max(0, period - elapsed)
    return {
      pct: 35 + ratio * 60,
      etaLabel: remaining > 60_000 ? `~${Math.ceil(remaining / 60_000)} min` : '<1 min',
    }
  }
  if (record.l2Sequence) return { pct: 35, etaLabel: '~5 min' }
  return { pct: 12, etaLabel: 'soon' }
}

export function EarnPanel() {
  const { initiaAddress, address, requestTxBlock } = useInterwovenKit()
  const [withdrawalError, setWithdrawalError] = useState<string | null>(null)
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [helpOpen, setHelpOpen] = useState(false)
  const [copiedHash, setCopiedHash] = useState(false)

  const {
    balance,
    formatted: rollupBalance,
    isLoading: isBalanceLoading,
  } = useRollupUsdcBalance(address as `0x${string}` | undefined)
  const {
    data: position,
    isLoading: isPositionLoading,
    error: positionError,
  } = useMerchantPosition(initiaAddress ?? undefined)
  const {
    data: executorStatus,
    isLoading: isExecutorLoading,
    error: executorError,
  } = useOpinitExecutorStatus(!!initiaAddress)
  const { records, addRecord, removeRecord } = useWithdrawalTracker(initiaAddress ?? null)

  const hasRollupUsdc = balance > 0n
  const outputLag = executorStatus
    ? Math.max(
        executorStatus.child.node.last_block_height -
          executorStatus.host.last_proposed_output_l2_block_number,
        0,
      )
    : null
  const isExecutorLagging =
    !!executorStatus &&
    (executorStatus.host.node.syncing ||
      executorStatus.child.node.syncing ||
      (outputLag !== null && outputLag > MAX_SAFE_OUTPUT_LAG))
  const canWithdraw = hasRollupUsdc && !!initiaAddress && !isExecutorLagging && !isSubmitting

  const apyLabel = position ? `${(position.apy_bps / 100).toFixed(2)}% APY` : '— APY'
  const targetApyLabel = position ? `${(position.apy_bps / 100).toFixed(2)}%` : '—'

  const latestSubmitted = records[0]
  const pendingRecords = records.filter((r) => r.status !== 'finalized' && r.status !== 'failed')
  const finalizedRecords = records.filter((r) => r.status === 'finalized')
  const pendingCount = pendingRecords.length
  const mintedCount = finalizedRecords.length

  const isSynced = !isExecutorLagging && !executorError && !isExecutorLoading

  async function handleWithdraw() {
    if (!canWithdraw) return

    setIsSubmitting(true)
    setWithdrawalError(null)
    const amount = balance.toString()

    try {
      const { transactionHash } = await requestTxBlock({
        messages: [
          {
            typeUrl: '/opinit.opchild.v1.MsgInitiateTokenWithdrawal',
            value: {
              sender: initiaAddress!,
              to: initiaAddress!,
              amount: { denom: ROLLUP_USDC_DENOM, amount },
            },
          },
        ],
      })

      if (!transactionHash) throw new Error('Missing transaction hash from wallet')

      addRecord({
        l2TxHash: normalizeTxHash(transactionHash),
        amount,
        denom: ROLLUP_USDC_DENOM,
        submittedAt: Date.now(),
        status: 'pending',
      })
    } catch (error) {
      const message = error instanceof Error ? error.message : 'Withdrawal request failed'
      setWithdrawalError(message)
    } finally {
      setIsSubmitting(false)
    }
  }

  function copyLastHash() {
    if (!latestSubmitted) return
    void navigator.clipboard?.writeText(latestSubmitted.l2TxHash)
    setCopiedHash(true)
    setTimeout(() => setCopiedHash(false), 1200)
  }

  let ctaLabel: string
  if (!hasRollupUsdc) ctaLabel = 'No balance to bridge'
  else if (isExecutorLagging) ctaLabel = 'Bridge catching up…'
  else if (isSubmitting) ctaLabel = 'Submitting withdrawal…'
  else ctaLabel = 'Bridge to L1 & Earn'

  let statusHint: string
  if (isExecutorLagging) {
    statusHint = `Local OPInit bot is catching up (${formatLag(executorStatus?.host.last_proposed_output_l2_block_number ?? null)} / ${formatLag(executorStatus?.child.node.last_block_height ?? null)} L2 blocks).`
  } else if (executorError) {
    statusHint = 'Executor status unavailable, but you can still sign the direct withdrawal.'
  } else if (isExecutorLoading) {
    statusHint = 'Checking bridge status…'
  } else {
    statusHint = `Direct ${ROLLUP_CHAIN_ID} → ${L1_CHAIN_ID} ${L1_USDC_DENOM} withdrawal · ~5 min finalization`
  }

  return (
    <section className="bridge-stage">
      <nav className="bridge-crumbs" aria-label="Breadcrumb">
        <span>Earn</span>
        <span className="sep">›</span>
        <span className="here">Bridge</span>
      </nav>

      <header className="bridge-h1-row">
        <div>
          <h1 className="bridge-h1">Bridge to L1</h1>
          <p className="bridge-h1-sub">
            Move settled USDC from your rollup back to L1 — auto-staked when the keeper is online.
          </p>
        </div>
        <div>
          <button
            type="button"
            className="bridge-help-toggle"
            aria-expanded={helpOpen}
            onClick={() => setHelpOpen((v) => !v)}
          >
            <HelpIcon />
            How it works
          </button>
        </div>
      </header>

      {helpOpen ? (
        <section className="bridge-help-panel">
          Withdrawals are canonical OPInit txs. After signing, the entry appears once the output
          proposal catches up and the finalization window clears (~5 min). If the direct staker is
          online, idle <span className="k">{ROLLUP_USDC_DENOM}</span> moves into staking the moment
          it lands on L1.
        </section>
      ) : null}

      <div className="bridge-grid">
        {/* Rollup balance + bridge action */}
        <div className="bridge-card">
          <div className="bridge-card-title-row">
            <div className="bridge-card-title">Rollup balance</div>
            <span className={`bridge-status-pill ${isSynced ? 'done' : 'pending'}`}>
              <span className="dot" />
              {isSynced ? 'Synced' : isExecutorLagging ? 'Catching up' : 'Checking…'}
            </span>
          </div>

          <div className="balance-hero">
            <span className="num">{isBalanceLoading ? '…' : rollupBalance}</span>
            <span className="sym">USDC</span>
          </div>
          <div className="balance-meta">
            <span>Available to bridge</span>
            <span className="sep">·</span>
            <span>~5 min finalization</span>
          </div>

          <button
            type="button"
            className={`bridge-cta${canWithdraw ? '' : ' is-disabled'}`}
            disabled={!canWithdraw}
            onClick={() => void handleWithdraw()}
          >
            <BridgeIcon />
            {ctaLabel}
          </button>

          {withdrawalError ? (
            <p style={{ margin: '10px 0 0', color: '#ff8a7a', fontSize: 13 }}>{withdrawalError}</p>
          ) : null}

          <p style={{ margin: '10px 0 0', color: 'var(--text-3)', fontSize: 12 }}>{statusHint}</p>

          {latestSubmitted ? (
            <div className="bridge-last-tx" role="group" aria-label="Last withdrawal">
              <span className="label">Last withdrawal</span>
              <span className="hash">{shortHash(latestSubmitted.l2TxHash)}</span>
              <span className="grow" />
              <button type="button" className="linklike" onClick={copyLastHash} title="Copy">
                {copiedHash ? <CheckIcon /> : <CopyIcon />}
                {copiedHash ? 'Copied' : 'Copy'}
              </button>
              <a
                href={getRollupTxUrl(latestSubmitted.l2TxHash)}
                target="_blank"
                rel="noreferrer"
                title="View"
              >
                <ExternalIcon />
                View
              </a>
            </div>
          ) : null}
        </div>

        {/* Staked on L1 */}
        <div className="bridge-card">
          <div className="bridge-card-title-row">
            <div className="bridge-card-title">Staked on L1</div>
            <span className="bridge-apy-pill">{apyLabel}</span>
          </div>

          {positionError ? (
            <p style={{ margin: 0, color: 'var(--text-2)', fontSize: 13 }}>
              Waiting for the staker API to report balances.
            </p>
          ) : (
            <div className="bridge-stake-list">
              <div className="bridge-stake-row">
                <span className="bridge-stake-label">Principal</span>
                <span className="bridge-stake-val">
                  {isPositionLoading || !position ? '…' : formatUsdc(position.principal_staked)}{' '}
                  <span className="unit">USDC</span>
                </span>
              </div>
              <div className="bridge-stake-row">
                <span className="bridge-stake-label">Available</span>
                <span className="bridge-stake-val muted">
                  {isPositionLoading || !position ? '…' : formatUsdc(position.principal_available)}{' '}
                  <span>USDC</span>
                </span>
              </div>
              <div className="bridge-stake-divider" />
              <div className="bridge-stake-row">
                <span className="bridge-stake-label">Yield earned</span>
                <span className="bridge-stake-val accent">
                  +{isPositionLoading || !position ? '…' : formatUsdc(position.yield_earned)}{' '}
                  <span className="unit">USDC</span>
                </span>
              </div>
            </div>
          )}

          <div className="bridge-target-row">
            <span className="lhs">Target APY</span>
            <span className="rhs">{targetApyLabel}</span>
          </div>
        </div>
      </div>

      {records.length > 0 ? (
        <div className="bridge-card bridge-queue-card">
          <div className="bridge-queue-head">
            <div className="bridge-queue-head-left">
              <div className="bridge-card-title" style={{ margin: 0 }}>
                Bridge queue
              </div>
              <span className="bridge-queue-count">
                <span className="num">{pendingCount}</span> bridging
                <span className="muted">·</span>
                <span className="muted">{mintedCount} minted</span>
              </span>
            </div>
            <span className="bridge-queue-hint">Click a finalized row to open the L1 mint tx</span>
          </div>

          <div>
            {records.map((r) => (
              <WithdrawalRow key={r.l2TxHash} record={r} onForget={() => removeRecord(r.l2TxHash)} />
            ))}
          </div>
        </div>
      ) : null}
    </section>
  )
}

function WithdrawalRow({
  record,
  onForget,
}: {
  record: WithdrawalRecord
  onForget: () => void
}) {
  const [, force] = useState(0)
  useEffect(() => {
    if (record.status === 'finalized' || record.status === 'failed') return
    const id = setInterval(() => force((n) => n + 1), 5_000)
    return () => clearInterval(id)
  }, [record.status])

  const isFinalized = record.status === 'finalized'
  const href = record.l1TxHash
    ? getL1TxUrl(record.l1TxHash)
    : getRollupTxUrl(record.l2TxHash)
  const { pct, etaLabel } = computeProgress(record)
  const statusLabel = isFinalized
    ? 'Minted on L1'
    : record.l2Sequence
      ? 'Awaiting L1 finalization'
      : 'Indexing L2 receipt'

  return (
    <a href={href} target="_blank" rel="noreferrer" className="bridge-queue-row" tabIndex={0}>
      <div className="amount">
        <span className="n">{formatUsdc(record.amount)}</span>
        <span className="s">USDC</span>
      </div>
      <div className="meta">
        {record.l2Sequence ? (
          <>
            <span className="seq">seq #{record.l2Sequence}</span>
            <span className="dot" />
          </>
        ) : null}
        <span>{elapsedLabel(record.submittedAt)}</span>
        <span className="dot" />
        <span className="seq">L2 {shortHash(record.l2TxHash)}</span>
        {record.l1TxHash ? (
          <>
            <span className="dot" />
            <span className="seq">L1 {shortHash(record.l1TxHash)}</span>
          </>
        ) : null}
      </div>
      <div className="bridge-progress">
        <span className={`bridge-status-pill ${isFinalized ? 'done' : 'pending'}`}>
          <span className="dot" />
          {statusLabel}
        </span>
        <div className="bp-track">
          <div
            className={`bp-fill${isFinalized ? '' : ' is-pending'}`}
            style={{ ['--pct' as never]: `${pct}%` }}
          />
        </div>
        <span className="bp-eta">{etaLabel}</span>
      </div>
      <button
        type="button"
        className="bridge-row-action"
        title="Forget"
        aria-label="Forget"
        onClick={(e) => {
          e.preventDefault()
          e.stopPropagation()
          onForget()
        }}
      >
        <CloseIcon />
      </button>
    </a>
  )
}

function HelpIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="9" />
      <path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.7.4-1 .9-1 1.7" />
      <circle cx="12" cy="17" r="0.6" fill="currentColor" />
    </svg>
  )
}

function BridgeIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M12 4v12" />
      <path d="m6 10 6 6 6-6" />
      <path d="M5 20h14" />
    </svg>
  )
}

function CopyIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <path d="M5 15V5a2 2 0 0 1 2-2h10" />
    </svg>
  )
}

function CheckIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round">
      <path d="m5 12 5 5L20 7" />
    </svg>
  )
}

function ExternalIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M7 17 17 7" />
      <path d="M8 7h9v9" />
    </svg>
  )
}

function CloseIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M6 6 18 18" />
      <path d="M18 6 6 18" />
    </svg>
  )
}
