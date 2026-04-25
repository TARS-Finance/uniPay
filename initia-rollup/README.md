# initia-rollup

Tooling for the Tars hackathon rollup: launch, seed the USDC bridge, redeploy HTLC, run E2E demo.

## Order of operations

1. `npm run install-tools` — installs `weave`, `minitiad`, `initiad` (one-time, requires sudo + Go).
2. `npm run preflight` — verifies tools.
3. Copy `.env.example` to `.env` and fill in merchant key + addresses.
4. `npm run launch` — interactive: launches the rollup; appends rollup endpoints to `.env`.
5. `npm run seed-bridge` — seeds first uusdc deposit; captures spawned ERC20 wrapper address.
6. `npm run redeploy-htlc` — deploys `HTLC.sol` against the rollup with the spawned USDC ERC20.
7. The seed and redeploy scripts also write the equivalent `VITE_*` keys into `../initia-fe/.env.local`.
8. Configure `../stacker/.env` with merchant key + USDC/INIT pool id.
9. `npm run demo` — guided end-to-end smoke for the demo recording.

See `docs/superpowers/specs/2026-04-25-rollup-usdc-bridge-design.md` for the design rationale and `docs/superpowers/plans/2026-04-25-rollup-usdc-bridge-implementation.md` for the implementation plan.
