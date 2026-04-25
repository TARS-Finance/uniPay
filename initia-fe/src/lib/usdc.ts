import { formatUnits } from 'viem'
import { useReadContract } from 'wagmi'
import { USDC_ERC20_ADDRESS } from './config'

const ERC20_ABI = [
  {
    name: 'balanceOf',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'account', type: 'address' }],
    outputs: [{ type: 'uint256' }],
  },
] as const

const ZERO_ADDRESS = '0x0000000000000000000000000000000000000000' as const
const DECIMALS = 6

function formatUsdc(balance: bigint) {
  return formatUnits(balance, DECIMALS).replace(/\.?0+$/, '') || '0'
}

export function useRollupUsdcBalance(address: `0x${string}` | undefined) {
  const { data, isLoading, error, refetch } = useReadContract({
    address: USDC_ERC20_ADDRESS,
    abi: ERC20_ABI,
    functionName: 'balanceOf',
    args: [address ?? ZERO_ADDRESS] as const,
    query: {
      enabled: !!address && USDC_ERC20_ADDRESS !== ZERO_ADDRESS,
    },
  })

  const balance = (data ?? 0n) as bigint

  return {
    balance,
    formatted: formatUsdc(balance),
    isLoading,
    error,
    refetch,
  }
}
