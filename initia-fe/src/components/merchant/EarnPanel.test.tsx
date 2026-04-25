import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const requestTxBlock = vi.fn()

let executorState = {
  data: {
    host: {
      node: {
        syncing: false,
        last_block_height: 100,
      },
      last_proposed_output_index: 40,
      last_proposed_output_l2_block_number: 95,
    },
    child: {
      node: {
        syncing: false,
        last_block_height: 100,
      },
      last_withdrawal_l2_sequence: 2,
      working_tree_index: 95,
    },
    bridge_id: 1912,
  },
  isLoading: false,
  error: null,
}

vi.mock('@initia/interwovenkit-react', () => ({
  useInterwovenKit: () => ({
    initiaAddress: 'init1merchant',
    address: '0x1111111111111111111111111111111111111111',
    requestTxBlock,
  }),
}))

vi.mock('../../lib/usdc', () => ({
  useRollupUsdcBalance: (address: string) => ({
    balance: address === '0x1111111111111111111111111111111111111111' ? 5_000_000n : 0n,
    formatted: address === '0x1111111111111111111111111111111111111111' ? '5' : '0',
    isLoading: false,
    error: null,
    refetch: vi.fn(),
  }),
}))

vi.mock('../../lib/stacker', () => ({
  useMerchantPosition: () => ({
    data: {
      principal_available: '0',
      principal_staked: '5000000',
      yield_earned: '12345',
      apy_bps: 2480,
    },
    isLoading: false,
    error: null,
  }),
}))

vi.mock('../../lib/opinit', () => ({
  useOpinitExecutorStatus: () => executorState,
}))

describe('EarnPanel', () => {
  beforeEach(() => {
    vi.resetModules()
    vi.clearAllMocks()
    vi.stubEnv('VITE_USDC_ROLLUP_DENOM', 'l2/test-usdc')
    executorState = {
      data: {
        host: {
          node: {
            syncing: false,
            last_block_height: 100,
          },
          last_proposed_output_index: 40,
          last_proposed_output_l2_block_number: 95,
        },
        child: {
          node: {
            syncing: false,
            last_block_height: 100,
          },
          last_withdrawal_l2_sequence: 2,
          working_tree_index: 95,
        },
        bridge_id: 1912,
      },
      isLoading: false,
      error: null,
    }
  })

  it('shows the rollup balance and enables the bridge button', async () => {
    const { EarnPanel } = await import('./EarnPanel')
    render(<EarnPanel />)

    expect(screen.getByText(/Rollup balance/i)).toBeInTheDocument()
    // Balance hero number, plus stake list "Principal" value
    expect(screen.getAllByText('5')).not.toHaveLength(0)
    expect(screen.getAllByText('USDC').length).toBeGreaterThanOrEqual(1)
    expect(
      screen.getByRole('button', { name: /Bridge to L1 & Earn/i }),
    ).not.toBeDisabled()
  })

  it('submits a direct withdrawal using the rollup wrapper denom from env', async () => {
    requestTxBlock.mockResolvedValue({
      transactionHash: '0xabc123',
    })

    const { EarnPanel } = await import('./EarnPanel')
    render(<EarnPanel />)

    fireEvent.click(screen.getByRole('button', { name: /Bridge to L1 & Earn/i }))

    await waitFor(() => {
      expect(requestTxBlock).toHaveBeenCalledWith({
        messages: [
          {
            typeUrl: '/opinit.opchild.v1.MsgInitiateTokenWithdrawal',
            value: {
              sender: 'init1merchant',
              to: 'init1merchant',
              amount: {
                denom: 'l2/test-usdc',
                amount: '5000000',
              },
            },
          },
        ],
      })
    })

    expect(screen.getByText(/Last withdrawal/i)).toBeInTheDocument()
    expect(screen.getByText('ABC123')).toBeInTheDocument()
  })

  it('disables the button while the local output submitter is far behind', async () => {
    executorState = {
      data: {
        ...executorState.data,
        host: {
          ...executorState.data.host,
          node: {
            ...executorState.data.host.node,
            syncing: true,
          },
          last_proposed_output_l2_block_number: 25,
        },
        child: {
          ...executorState.data.child,
          node: {
            ...executorState.data.child.node,
            last_block_height: 100,
          },
        },
      },
      isLoading: false,
      error: null,
    }

    const { EarnPanel } = await import('./EarnPanel')
    render(<EarnPanel />)

    expect(
      screen.getByRole('button', { name: /Bridge catching up/i }),
    ).toBeDisabled()
    expect(screen.getByText(/Local OPInit bot is catching up/i)).toBeInTheDocument()
  })

  it('renders staked principal, yield, and APY from stacker', async () => {
    const { EarnPanel } = await import('./EarnPanel')
    render(<EarnPanel />)

    expect(screen.getByText(/Staked on L1/i)).toBeInTheDocument()
    expect(screen.getByText('+0.012345')).toBeInTheDocument()
    expect(screen.getByText('24.80% APY')).toBeInTheDocument()
  })
})
