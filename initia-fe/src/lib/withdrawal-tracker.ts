import { useCallback, useEffect, useRef, useState } from 'react'
import { L1_REST_URL, OPINIT_EXECUTOR_API, ROLLUP_REST_URL } from './config'

export type WithdrawalStatus = 'pending' | 'proven' | 'finalized' | 'failed'

export interface WithdrawalRecord {
  l2TxHash: string
  amount: string
  denom: string
  submittedAt: number
  l2Sequence?: string
  l2Height?: string
  bridgeId?: string
  outputIndex?: string
  outputProposedAt?: number
  finalizationPeriodMs?: number
  status: WithdrawalStatus
  l1FinalizedAt?: number
  l1TxHash?: string
  lastError?: string
}

const STORAGE_PREFIX = 'tars:withdrawals:'
const POLL_INTERVAL_MS = 15_000

function storageKey(owner: string) {
  return `${STORAGE_PREFIX}${owner.toLowerCase()}`
}

function loadRecords(owner: string): WithdrawalRecord[] {
  if (typeof window === 'undefined') return []
  try {
    const raw = window.localStorage.getItem(storageKey(owner))
    if (!raw) return []
    const parsed = JSON.parse(raw) as WithdrawalRecord[]
    return Array.isArray(parsed) ? parsed : []
  } catch {
    return []
  }
}

function saveRecords(owner: string, records: WithdrawalRecord[]) {
  if (typeof window === 'undefined') return
  try {
    window.localStorage.setItem(storageKey(owner), JSON.stringify(records))
  } catch {
    /* quota / disabled storage — ignore */
  }
}

interface CosmosEvent {
  type: string
  attributes: Array<{ key: string; value: string }>
}

interface TxResponse {
  height?: string
  txhash?: string
  events?: CosmosEvent[]
  logs?: Array<{ events?: CosmosEvent[] }>
}

function getAttr(event: CosmosEvent, key: string) {
  return event.attributes.find((a) => a.key === key)?.value
}

function flattenEvents(tx: TxResponse): CosmosEvent[] {
  const events: CosmosEvent[] = []
  if (Array.isArray(tx.events)) events.push(...tx.events)
  if (Array.isArray(tx.logs)) {
    for (const log of tx.logs) {
      if (Array.isArray(log.events)) events.push(...log.events)
    }
  }
  return events
}

async function fetchL2WithdrawalInfo(txHash: string) {
  const url = `${ROLLUP_REST_URL.replace(/\/$/, '')}/cosmos/tx/v1beta1/txs/${txHash}`
  const res = await fetch(url)
  if (!res.ok) throw new Error(`L2 tx fetch ${res.status}`)
  const body = await res.json() as { tx_response?: TxResponse }
  const tx = body.tx_response
  if (!tx) throw new Error('missing tx_response')
  const events = flattenEvents(tx)
  const initiate = events.find((e) => e.type === 'initiate_token_withdrawal')
  const sequence = initiate ? getAttr(initiate, 'l2_sequence') : undefined
  return {
    height: tx.height,
    sequence,
  }
}

interface ExecutorWithdrawalResponse {
  sequence: number
  output_index: number
  bridge_id: number
  tx_hash?: string
  tx_height?: number
}

async function fetchExecutorWithdrawal(sequence: string): Promise<ExecutorWithdrawalResponse | null> {
  const url = `${OPINIT_EXECUTOR_API.replace(/\/$/, '')}/withdrawal/${sequence}`
  try {
    const res = await fetch(url)
    if (!res.ok) return null
    return (await res.json()) as ExecutorWithdrawalResponse
  } catch {
    return null
  }
}

interface OutputProposalResponse {
  output_proposal?: {
    l1_block_time?: string
    l1_block_number?: string
    l2_block_number?: string
  }
}

async function fetchOutputProposal(bridgeId: string, outputIndex: string) {
  const url = `${L1_REST_URL.replace(/\/$/, '')}/opinit/ophost/v1/bridges/${bridgeId}/outputs/${outputIndex}`
  const res = await fetch(url)
  if (!res.ok) return null
  return (await res.json()) as OutputProposalResponse
}

interface BridgeConfigResponse {
  bridge_config?: {
    finalization_period?: string
  }
}

function parseDurationSeconds(raw: string | undefined): number | null {
  if (!raw) return null
  const match = raw.match(/^([\d.]+)s$/)
  if (!match) return null
  const seconds = Number(match[1])
  return Number.isFinite(seconds) ? seconds : null
}

interface CosmosTxSearchResponse {
  tx_responses?: TxResponse[]
}

async function fetchL1FinalizeTx(bridgeId: string, sequence: string): Promise<string | null> {
  const base = `${L1_REST_URL.replace(/\/$/, '')}/cosmos/tx/v1beta1/txs`
  const conds = [
    `finalize_token_withdrawal.bridge_id='${bridgeId}'`,
    `finalize_token_withdrawal.l2_sequence='${sequence}'`,
  ]
  // SDK v0.50+ uses `query`; older builds accept repeated `events`. Try both.
  const candidates = [
    `${base}?query=${encodeURIComponent(conds.join(' AND '))}&pagination.limit=1`,
    `${base}?${conds.map((c) => `events=${encodeURIComponent(c)}`).join('&')}&pagination.limit=1`,
  ]
  for (const url of candidates) {
    try {
      const res = await fetch(url)
      if (!res.ok) continue
      const body = (await res.json()) as CosmosTxSearchResponse
      const tx = body.tx_responses?.[0]
      if (tx?.txhash) return tx.txhash
    } catch {
      /* try next */
    }
  }
  return null
}

async function fetchFinalizationPeriodMs(bridgeId: string): Promise<number | null> {
  const url = `${L1_REST_URL.replace(/\/$/, '')}/opinit/ophost/v1/bridges/${bridgeId}`
  const res = await fetch(url)
  if (!res.ok) return null
  const body = (await res.json()) as BridgeConfigResponse
  const seconds = parseDurationSeconds(body.bridge_config?.finalization_period)
  return seconds === null ? null : seconds * 1000
}

export function useWithdrawalTracker(owner: string | null | undefined) {
  const [records, setRecords] = useState<WithdrawalRecord[]>([])
  const ownerRef = useRef<string | null>(null)

  useEffect(() => {
    if (!owner) {
      ownerRef.current = null
      setRecords([])
      return
    }
    ownerRef.current = owner
    setRecords(loadRecords(owner))
  }, [owner])

  const persist = useCallback((next: WithdrawalRecord[]) => {
    setRecords(next)
    if (ownerRef.current) saveRecords(ownerRef.current, next)
  }, [])

  const updateRecord = useCallback((l2TxHash: string, patch: Partial<WithdrawalRecord>) => {
    setRecords((prev) => {
      const next = prev.map((r) => (r.l2TxHash === l2TxHash ? { ...r, ...patch } : r))
      if (ownerRef.current) saveRecords(ownerRef.current, next)
      return next
    })
  }, [])

  const addRecord = useCallback((record: WithdrawalRecord) => {
    setRecords((prev) => {
      const exists = prev.some((r) => r.l2TxHash === record.l2TxHash)
      const next = exists ? prev : [record, ...prev]
      if (ownerRef.current) saveRecords(ownerRef.current, next)
      return next
    })
  }, [])

  const removeRecord = useCallback((l2TxHash: string) => {
    setRecords((prev) => {
      const next = prev.filter((r) => r.l2TxHash !== l2TxHash)
      if (ownerRef.current) saveRecords(ownerRef.current, next)
      return next
    })
  }, [])

  useEffect(() => {
    if (!owner) return
    let cancelled = false

    async function advance(record: WithdrawalRecord): Promise<Partial<WithdrawalRecord> | null> {
      let { l2Sequence, l2Height, bridgeId, outputIndex, outputProposedAt, finalizationPeriodMs } = record
      const patch: Partial<WithdrawalRecord> = {}

      if (!l2Sequence) {
        const info = await fetchL2WithdrawalInfo(record.l2TxHash)
        l2Sequence = info.sequence
        l2Height = info.height
        if (l2Sequence) {
          patch.l2Sequence = l2Sequence
          patch.l2Height = l2Height
        }
      }
      if (!l2Sequence) return Object.keys(patch).length ? patch : null

      if (!bridgeId || !outputIndex) {
        const exec = await fetchExecutorWithdrawal(l2Sequence)
        if (exec) {
          bridgeId = String(exec.bridge_id)
          outputIndex = String(exec.output_index)
          patch.bridgeId = bridgeId
          patch.outputIndex = outputIndex
          patch.status = 'proven'
        }
      }
      if (!bridgeId || !outputIndex) return Object.keys(patch).length ? patch : null

      if (!finalizationPeriodMs) {
        const period = await fetchFinalizationPeriodMs(bridgeId)
        if (period !== null) {
          finalizationPeriodMs = period
          patch.finalizationPeriodMs = period
        }
      }

      if (!outputProposedAt) {
        const proposal = await fetchOutputProposal(bridgeId, outputIndex)
        const ts = proposal?.output_proposal?.l1_block_time
        if (ts) {
          const ms = new Date(ts).getTime()
          if (Number.isFinite(ms)) {
            outputProposedAt = ms
            patch.outputProposedAt = ms
          }
        }
      }

      const periodElapsed = !!(outputProposedAt && finalizationPeriodMs && Date.now() >= outputProposedAt + finalizationPeriodMs)

      // Try to find the actual L1 finalize tx once we're past the proven stage
      // or once enough time has elapsed that the relayer should have submitted it.
      const shouldQueryL1 = !record.l1TxHash && (periodElapsed || record.status === 'proven' || record.status === 'finalized')
      if (shouldQueryL1) {
        const l1TxHash = await fetchL1FinalizeTx(bridgeId, l2Sequence)
        if (l1TxHash) {
          patch.l1TxHash = l1TxHash
          patch.status = 'finalized'
          patch.l1FinalizedAt = Date.now()
          patch.lastError = undefined
          return patch
        }
      }

      if (periodElapsed && record.status !== 'finalized') {
        patch.status = 'finalized'
        patch.l1FinalizedAt = Date.now()
        patch.lastError = undefined
      }

      return Object.keys(patch).length ? patch : null
    }

    async function tick() {
      const current = ownerRef.current ? loadRecords(ownerRef.current) : []
      const active = current.filter(
        (r) =>
          r.status === 'pending' ||
          r.status === 'proven' ||
          (r.status === 'finalized' && !r.l1TxHash),
      )
      if (active.length === 0) return

      for (const record of active) {
        if (cancelled) return
        try {
          const patch = await advance(record)
          if (cancelled) return
          if (patch) updateRecord(record.l2TxHash, patch)
        } catch (error) {
          if (cancelled) return
          const message = error instanceof Error ? error.message : 'poll failed'
          updateRecord(record.l2TxHash, { lastError: message })
        }
      }
    }

    void tick()
    const id = window.setInterval(() => { void tick() }, POLL_INTERVAL_MS)
    return () => {
      cancelled = true
      window.clearInterval(id)
    }
  }, [owner, updateRecord])

  return { records, addRecord, updateRecord, removeRecord, persist }
}
