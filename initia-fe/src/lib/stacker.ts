import { useQuery } from '@tanstack/react-query'
import { STACKER_API } from './config'

// ── Types ─────────────────────────────────────────────────────────────────────

export interface MerchantPosition {
  principal_available: string
  principal_staked: string
  yield_earned: string
  claimable_init_rewards?: string
  apy_bps: number
}

export interface MerchantOverview extends MerchantPosition {
  pool_count: number
  total_executions: number
}

export interface MerchantPool {
  id: string
  poolId: string | null
  name: string
  inputDenom: string
  tokens: [string, string]
  staked: string
  available: string
  apy_bps: number
  earned: string
  status: string
  lastExecutedAt: string | null
  executionCount: number
}

export interface MerchantActivityItem {
  id: string
  strategyId: string
  inputDenom: string
  amount: string
  lpAmount: string
  status: string
  staked: boolean
  provideTxHash: string | null
  delegateTxHash: string | null
  txUrl: string | null
  errorMessage: string | null
  startedAt: string
  finishedAt: string | null
}

export interface MerchantChartPoint {
  date: string
  cumulative_staked: string
}

// ── Fetchers ──────────────────────────────────────────────────────────────────

async function jsonGet<T>(path: string): Promise<T> {
  const response = await fetch(`${STACKER_API}${path}`)
  if (!response.ok) {
    throw new Error(`stacker ${path} returned ${response.status}`)
  }
  return response.json() as Promise<T>
}

// ── Hooks ─────────────────────────────────────────────────────────────────────

export function useMerchantPosition(merchantId: string | undefined) {
  return useQuery({
    queryKey: ['stacker', 'merchant-position', merchantId],
    queryFn: () => jsonGet<MerchantPosition>(`/merchants/${merchantId}/balance`),
    enabled: !!merchantId,
    refetchInterval: 5_000,
  })
}

export function useMerchantOverview(merchantId: string | undefined) {
  return useQuery({
    queryKey: ['stacker', 'merchant-overview', merchantId],
    queryFn: () => jsonGet<MerchantOverview>(`/merchants/${merchantId}/overview`),
    enabled: !!merchantId,
    refetchInterval: 10_000,
  })
}

export function useMerchantPools(merchantId: string | undefined) {
  return useQuery({
    queryKey: ['stacker', 'merchant-pools', merchantId],
    queryFn: () =>
      jsonGet<{ pools: MerchantPool[] }>(`/merchants/${merchantId}/pools`).then((r) => r.pools),
    enabled: !!merchantId,
    refetchInterval: 10_000,
  })
}

export function useMerchantActivity(merchantId: string | undefined, limit = 50) {
  return useQuery({
    queryKey: ['stacker', 'merchant-activity', merchantId, limit],
    queryFn: () =>
      jsonGet<{ activity: MerchantActivityItem[] }>(
        `/merchants/${merchantId}/activity?limit=${limit}`,
      ).then((r) => r.activity),
    enabled: !!merchantId,
    refetchInterval: 10_000,
  })
}

export function useMerchantChart(merchantId: string | undefined) {
  return useQuery({
    queryKey: ['stacker', 'merchant-chart', merchantId],
    queryFn: () =>
      jsonGet<{ points: MerchantChartPoint[] }>(`/merchants/${merchantId}/chart`).then(
        (r) => r.points,
      ),
    enabled: !!merchantId,
    refetchInterval: 30_000,
  })
}
