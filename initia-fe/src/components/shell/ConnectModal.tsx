import { useEffect } from 'react';
import { useInterwovenKit } from '@initia/interwovenkit-react';

interface Props {
  onClose: () => void;
}

/**
 * ConnectModal — delegates to InterwovenKit's built-in connect modal.
 *
 * The legacy custom connect modal has been replaced: instead of rendering a
 * custom overlay we simply call openConnect() from InterwovenKit, which opens the
 * SDK's own wallet selection UI. This component is kept in the tree so
 * App.tsx code paths that conditionally render it continue to work without
 * changes. It triggers the SDK modal on mount and calls onClose immediately
 * so the parent can clean up its own state flag.
 */
export function ConnectModal({ onClose }: Props) {
  const { openConnect, isConnected } = useInterwovenKit();

  useEffect(() => {
    openConnect();
    // Once the SDK modal opens it manages its own lifecycle.
    // Notify the parent that our wrapper is done so the flag is cleared.
    onClose();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // Close wrapper when user connects
  useEffect(() => {
    if (isConnected) onClose();
  }, [isConnected, onClose]);

  return null;
}
