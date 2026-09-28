#!/usr/bin/env bash
# Fail if a known secret pattern appears in tracked files or the staged diff.
# The repository is public: committing credentials is forbidden
# (see docs/05_STORAGE_SECURITY.md §Secrets policy).
set -u

# Real secrets only. Client KEYS (Google client id, TikTok client key) are public
# identifiers; client SECRETS and tokens are not.
PATTERNS='GOCSPX-[A-Za-z0-9_-]{10,}|AIza[0-9A-Za-z_-]{30,}|client_secret"?\s*[:=]\s*"[A-Za-z0-9_-]{24,}|https://discord(app)?\.com/api/webhooks/[0-9]{15,}/[A-Za-z0-9_-]{50,}|rft\.[A-Za-z0-9]{20,}|act\.[A-Za-z0-9]{20,}|-----BEGIN [A-Z ]*PRIVATE KEY-----'

status=0

if git grep -nIE "$PATTERNS" -- . \
    ':(exclude)docs/**' \
    ':(exclude)*.md' \
    ':(exclude)social.example.json' \
    ':(exclude)scripts/check-secrets.sh' 2>/dev/null; then
  echo "FATAL: potential secret(s) found in tracked files (see above)." >&2
  echo "Never commit secrets; rotate first, then rewrite history." >&2
  status=1
fi

if git diff --cached -U0 | grep -nIE "^\+.*($PATTERNS)" 2>/dev/null; then
  echo "FATAL: staged changes contain potential secrets." >&2
  status=1
fi

if [ "$status" -eq 0 ]; then
  echo "check-secrets: clean"
fi
exit "$status"
