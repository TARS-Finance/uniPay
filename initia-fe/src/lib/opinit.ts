import { useQuery } from '@tanstack/react-query'
import { OPINIT_EXECUTOR_API } from './config'

export interface OpinitExecutorStatus {
  bridge_id: number
  host: {
    node: {
      syncing: boolean
      last_block_height: number
    }
    last_proposed_output_index: number
    last_proposed_output_l2_block_number: number
  }
  child: {
    node: {
      syncing: boolean
      last_block_height: number
    }
    last_withdrawal_l2_sequence: number
    working_tree_index: number
  }
}

async function fetchOpinitExecutorStatus(): Promise<OpinitExecutorStatus> {
  const response = await fetch(`${OPINIT_EXECUTOR_API}/status`)

  if (!response.ok) {
    throw new Error(`opinit executor returned ${response.status}`)
  }

  return response.json() as Promise<OpinitExecutorStatus>
}

export function useOpinitExecutorStatus(enabled = true) {
  return useQuery({
    queryKey: ['opinit', 'executor-status'],
    queryFn: fetchOpinitExecutorStatus,
    enabled,
    refetchInterval: 5_000,
    retry: false,
  })
}
