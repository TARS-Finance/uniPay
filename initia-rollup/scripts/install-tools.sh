#!/usr/bin/env bash
set -euo pipefail

echo ">> Installing weave (Initia, from initia-labs/tap)..."

# If the wrong-tap "weave" (git merge tool) is installed, uninstall it first
if command -v weave >/dev/null && weave --help 2>&1 | grep -q 'semantic merge for Git'; then
  echo "   detected git-merge weave; uninstalling so we can install the Initia one"
  brew uninstall --ignore-dependencies weave || true
fi

brew tap initia-labs/tap 2>/dev/null || true
brew install initia-labs/tap/weave

echo ">> Installing initiad (L1) from initia-labs/initia..."
rm -rf /tmp/initia
git clone --depth 1 https://github.com/initia-labs/initia.git /tmp/initia
( cd /tmp/initia && make install )
rm -rf /tmp/initia

echo ">> Installing minitiad (minievm) from initia-labs/minievm..."
rm -rf /tmp/minievm
git clone --depth 1 https://github.com/initia-labs/minievm.git /tmp/minievm
( cd /tmp/minievm && make install )
rm -rf /tmp/minievm

echo
echo ">> Done. Verify with:"
echo "     weave version"
echo "     initiad version"
echo "     minitiad version --long | head -3"
