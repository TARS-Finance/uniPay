import { describe, expect, it, vi } from 'vitest'
import { renderHook } from '@testing-library/react'
import { useRollupUsdcBalance } from './usdc'

vi.mock('wagmi', () => ({
  useReadContract: ({ args }: { args: readonly [`0x${string}`] }) => ({
    data: args[0] === '0x1111111111111111111111111111111111111111' ? 5_000_000n : 0n,
    isLoading: false,
    error: null,
    refetch: vi.fn(),
  }),
}))

describe('useRollupUsdcBalance', () => {
  it('returns the formatted balance for a funded address', () => {
    const { result } = renderHook(() =>
      useRollupUsdcBalance('0x1111111111111111111111111111111111111111'),
    )

    expect(result.current.balance).toBe(5_000_000n)
    expect(result.current.formatted).toBe('5')
  })

  it('returns zero for an unfunded address', () => {
    const { result } = renderHook(() =>
      useRollupUsdcBalance('0x0000000000000000000000000000000000000000'),
    )

    expect(result.current.balance).toBe(0n)
    expect(result.current.formatted).toBe('0')
  })
})
