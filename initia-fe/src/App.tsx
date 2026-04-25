import { useCallback, useEffect, useState } from 'react';
import './styles.css';
import type { PersonaType, AnyPage, CustomerPage, MerchantPage } from './types';
import { Landing } from './components/landing/Landing';
import { CustomerView } from './components/customer/CustomerView';
import { MerchantView } from './components/merchant/MerchantView';
import { Sidebar } from './components/shell/Sidebar';
import { ConnectModal } from './components/shell/ConnectModal';
import { useWallet } from './lib/wallet-context';
import { parsePaymentInvoice } from './lib/invoice';
import { useSecretPoller } from './hooks/useSecretPoller';

const FALLBACK_PAGE: AnyPage = 'pay';

function normalizeCustomerPage(page: AnyPage | null): CustomerPage {
  return page === 'history' ? 'history' : 'pay';
}

function normalizeMerchantPage(page: AnyPage | null): MerchantPage {
  return page === 'pools' || page === 'activity' || page === 'earn' ? page : 'overview';
}

function normalizePageForPersona(persona: PersonaType, page: AnyPage | null): AnyPage {
  if (persona === 'customer') return normalizeCustomerPage(page);
  if (persona === 'merchant') return normalizeMerchantPage(page);
  return FALLBACK_PAGE;
}

function routePath(persona: PersonaType, page: AnyPage): string {
  if (persona === 'customer') {
    return `/customer/${normalizeCustomerPage(page)}`;
  }
  if (persona === 'merchant') {
    return `/merchant/${normalizeMerchantPage(page)}`;
  }
  return '/';
}

function defaultPage(persona: PersonaType): AnyPage {
  return persona === 'merchant' ? 'overview' : 'pay';
}

function readStoredPersona(): PersonaType {
  if (typeof window === 'undefined') return null;
  try {
    const raw = localStorage.getItem('initia.persona');
    return raw === 'customer' || raw === 'merchant' ? raw : null;
  } catch {
    return null;
  }
}

function readStoredPage(persona: NonNullable<PersonaType>): AnyPage {
  if (typeof window === 'undefined') return FALLBACK_PAGE;
  try {
    const raw = localStorage.getItem('initia.page');
    return normalizePageForPersona(persona, raw as AnyPage | null);
  } catch {
    return persona === 'merchant' ? 'overview' : 'pay';
  }
}

function parseRouteFromLocation(): {
  persona: PersonaType;
  page: AnyPage;
  canonicalPath: string;
} {
  if (typeof window === 'undefined') {
    return {
      persona: null,
      page: FALLBACK_PAGE,
      canonicalPath: '/',
    };
  }

  const [personaSlug, pageSlugRaw] = window.location.pathname
    .split('?')[0]
    .split('/')
    .filter(Boolean);
  const pageSlug = normalizeCustomerPage(pageSlugRaw as AnyPage | null);
  const pageSlugMerchant = normalizeMerchantPage(pageSlugRaw as AnyPage | null);

  if (personaSlug === 'customer') {
    const page = pageSlug;
    return {
      persona: 'customer',
      page,
      canonicalPath: routePath('customer', page),
    };
  }

  if (personaSlug === 'merchant') {
    const page = pageSlugMerchant;
    return {
      persona: 'merchant',
      page,
      canonicalPath: routePath('merchant', page),
    };
  }

  const storedPersona = readStoredPersona();
  if (storedPersona) {
    const page = readStoredPage(storedPersona);
    return {
      persona: storedPersona,
      page,
      canonicalPath: routePath(storedPersona, page),
    };
  }

  return {
    persona: null,
    page: FALLBACK_PAGE,
    canonicalPath: '/',
  };
}

export default function App() {
  useSecretPoller();
  const { address: wallet, isConnectModalOpen, closeConnectModal } = useWallet();
  const [invoicePrefill] = useState(() => {
    if (typeof window === 'undefined') return null;
    return parsePaymentInvoice(window.location.search);
  });

  const routeFromUrl = parseRouteFromLocation();
  const initialRoute: ReturnType<typeof parseRouteFromLocation> = invoicePrefill
    ? { persona: 'customer', page: 'pay', canonicalPath: routePath('customer', 'pay') }
    : routeFromUrl;
  const [persona, setPersona] = useState<PersonaType>(() => initialRoute.persona);
  const [page, setPage] = useState<AnyPage>(() => initialRoute.page);
  const [flipping, setFlipping] = useState(false);
  const [flipDir, setFlipDir] = useState(1);

  const writeRoute = useCallback((targetPersona: PersonaType, targetPage: AnyPage, replace = false) => {
    if (typeof window === 'undefined') return;
    const path = routePath(targetPersona, normalizePageForPersona(targetPersona, targetPage));
    const method = replace ? 'replaceState' : 'pushState';
    if (window.location.pathname === path) return;
    window.history[method]({}, '', path);
  }, []);

  const syncFromLocation = useCallback(() => {
    const route = parseRouteFromLocation();
    setPersona(route.persona);
    setPage(route.page);
  }, []);

  const savePage = (p: AnyPage) => {
    const normalizedPage = normalizePageForPersona(persona, p);
    setPage(normalizedPage);
    if (persona) {
      writeRoute(persona, normalizedPage);
    }
  };

  useEffect(() => {
    try {
      if (persona) {
        localStorage.setItem('initia.persona', persona);
      } else {
        localStorage.removeItem('initia.persona');
      }
    } catch {
      /* ignore */
    }
  }, [persona]);

  useEffect(() => {
    try { localStorage.setItem('initia.page', page); } catch { /* ignore */ }
  }, [page]);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    if (window.location.pathname !== initialRoute.canonicalPath) {
      window.history.replaceState({}, '', initialRoute.canonicalPath);
    }

    const onPopState = () => syncFromLocation();
    window.addEventListener('popstate', onPopState);
    return () => window.removeEventListener('popstate', onPopState);
  }, [syncFromLocation, initialRoute.canonicalPath]);

  const transition = (next: PersonaType, dir: number) => {
    setFlipping(true);
    setFlipDir(dir);
    setTimeout(() => {
      setPersona(next);
      if (next) {
        const nextPage = defaultPage(next);
        setPage(nextPage);
        writeRoute(next, nextPage);
      } else {
        writeRoute(null, FALLBACK_PAGE);
      }
    }, 300);
    setTimeout(() => setFlipping(false), 700);
  };

  const goHome = () => transition(null, -1);

  const selectPersona = (target: 'customer' | 'merchant') => {
    if (flipping) return;
    if (persona === null) {
      transition(target, 1);
    } else if (persona !== target) {
      transition(target, persona === 'customer' ? -1 : 1);
    }
  };


  const flipStyle: React.CSSProperties = {
    position: 'absolute',
    inset: 0,
    transformStyle: 'preserve-3d',
    transition: 'transform 600ms cubic-bezier(.65,0,.35,1), opacity 600ms ease',
    transform: flipping
      ? `perspective(2000px) rotateY(${flipDir * 14}deg) rotateX(-6deg) scale(0.96)`
      : 'none',
    transformOrigin: flipDir > 0 ? 'right center' : 'left center',
  };

  return (
    <div className={`persona-${persona || 'customer'}`} style={{ position: 'absolute', inset: 0, overflow: 'hidden', background: 'var(--bg-0)' }}>
      <div style={flipStyle}>
        {!persona && <Landing onPick={(p) => transition(p, 1)} />}

        {persona && (
          <div className="app-frame">
            <div className="app-shell">
              <Sidebar
                persona={persona}
                page={page}
                setPage={savePage}
                wallet={wallet}
                onSwitchPersona={() => selectPersona(persona === 'customer' ? 'merchant' : 'customer')}
                onHome={goHome}
              />
              <div className="app-main">
                {persona === 'customer' && (
                  <CustomerView
                    wallet={wallet}
                    page={page as 'pay' | 'history'}
                    setPage={(p) => savePage(p as AnyPage)}
                    invoicePrefill={invoicePrefill}
                  />
                )}
                {persona === 'merchant' && (
                  <MerchantView
                    page={page as 'overview' | 'pools' | 'activity' | 'earn'}
                    setPage={(p) => savePage(p as AnyPage)}
                  />
                )}
              </div>
            </div>
          </div>
        )}
      </div>

      {isConnectModalOpen && <ConnectModal onClose={closeConnectModal} />}

      {flipping && (
        <div style={{ position: 'absolute', inset: 0, pointerEvents: 'none', zIndex: 20, overflow: 'hidden' }}>
          <div style={{
            position: 'absolute', top: 0, right: 0, width: '200%', height: '200%',
            background: 'linear-gradient(135deg, rgba(20,30,50,0.0) 45%, rgba(100,130,180,0.35) 50%, rgba(10,14,24,0.85) 55%, rgba(5,7,14,0.95) 100%)',
            transform: 'translateX(-10%) translateY(-10%)',
            animation: 'curl-sweep 600ms ease-in-out forwards',
          }} />
        </div>
      )}
    </div>
  );
}
