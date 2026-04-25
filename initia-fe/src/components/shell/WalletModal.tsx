import { useInterwovenKit } from '@initia/interwovenkit-react';
import { Icons } from '../shared/Icons';

interface Props {
  onClose: () => void;
  onPick?: (addr: string) => void;
}

/**
 * WalletModal — delegates to InterwovenKit's built-in wallet management modal.
 *
 * The legacy custom wallet-picker has been replaced with openWallet() from the
 * SDK. The onPick callback is kept for backwards compatibility but is no longer
 * called in this implementation; callers should rely on useWallet().address
 * updating reactively once the user connects.
 */
export function WalletModal({ onClose }: Props) {
  const { openWallet, initiaAddress } = useInterwovenKit();

  const handleManage = () => {
    if (initiaAddress) {
      openWallet();
    }
    onClose();
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="glass" onClick={(e) => e.stopPropagation()} style={{ width: 400, padding: 24, position: 'relative' }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 18 }}>
          <div>
            <div className="eyebrow" style={{ marginBottom: 4 }}>Wallet</div>
            <div style={{ fontSize: 18, fontWeight: 500 }}>Manage wallet</div>
          </div>
          <button className="iconbtn" onClick={onClose}><Icons.x /></button>
        </div>

        <button
          className="lift"
          onClick={handleManage}
          style={{
            display: 'flex', alignItems: 'center', gap: 14,
            padding: '14px 16px',
            background: 'rgba(255,255,255,0.02)',
            border: '1px solid var(--hairline)',
            borderRadius: 12,
            color: 'var(--text-0)',
            cursor: 'pointer',
            textAlign: 'left',
            width: '100%',
          }}
        >
          <div style={{ width: 36, height: 36, borderRadius: 10, background: 'linear-gradient(135deg, #e8ecf3, #8792a6)', display: 'grid', placeItems: 'center', color: '#06090f', fontWeight: 700, fontFamily: 'var(--font-mono)' }}>I</div>
          <div style={{ flex: 1 }}>
            <div style={{ fontSize: 14, fontWeight: 500 }}>InterwovenKit wallet</div>
            <div className="mono" style={{ fontSize: 11, color: 'var(--text-3)' }}>initia · cosmos</div>
          </div>
          <Icons.chevron style={{ transform: 'rotate(-90deg)', color: 'var(--text-3)' }} />
        </button>

        <div className="mono" style={{ fontSize: 10, color: 'var(--text-3)', letterSpacing: '0.06em', marginTop: 16, textAlign: 'center' }}>
          POWERED BY INTERWOVENKIT
        </div>
      </div>
    </div>
  );
}
