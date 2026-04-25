import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook, waitFor } from '@testing-library/react'
import { createElement, type ReactNode } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useMerchantPosition } from './stacker'

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
      },
    },
  })

  return createElement(QueryClientProvider, { client }, children)
}

beforeEach(() => {
  vi.stubGlobal('fetch', vi.fn())
})

describe('useMerchantPosition', () => {
  it('returns merchant position data from the stacker API', async () => {
    vi.mocked(fetch).mockResolvedValue({
      ok: true,
      json: async () => ({
        principal_available: '1000000',
        principal_staked: '5000000',
        yield_earned: '12345',
        apy_bps: 2480,
      }),
    } as Response)

    const { result } = renderHook(() => useMerchantPosition('init1merchant'), {
      wrapper,
    })

    await waitFor(() => expect(result.current.data).toBeDefined())
    expect(result.current.data?.principal_staked).toBe('5000000')
    expect(result.current.data?.apy_bps).toBe(2480)
  })

  it('surfaces an error for non-ok responses', async () => {
    vi.mocked(fetch).mockResolvedValue({
      ok: false,
      status: 500,
      json: async () => ({}),
    } as Response)

    const { result } = renderHook(() => useMerchantPosition('init1merchant'), {
      wrapper,
    })

    await waitFor(() => expect(result.current.error).toBeDefined())
  })
})
