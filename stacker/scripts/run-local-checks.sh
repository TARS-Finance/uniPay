#!/usr/bin/env bash

set -euo pipefail

pnpm lint
pnpm typecheck
pnpm db:migrate
pnpm test
