#!/bin/bash
# backup.sh — Disaster recovery for Memoria
#
# Uploads the SQLite database to S3 with a timestamp.
# Run daily via launchd, after the subconscious pass.
#
# Usage:
#   ./scripts/backup.sh              # Full backup
#   ./scripts/backup.sh --verify     # Verify latest backup exists

set -euo pipefail

export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"

BUCKET="memoria-backup-juzzydee"
DB_PATH="$HOME/.memoria/memoria.db"
TIMESTAMP=$(date -u +"%Y%m%dT%H%M%SZ")
S3_KEY="backups/memoria-${TIMESTAMP}.db"

if [ ! -f "$DB_PATH" ]; then
    echo "[backup] ERROR: Database not found at $DB_PATH" >&2
    exit 1
fi

if [ "${1:-}" = "--verify" ]; then
    echo "[backup] Checking latest backup..."
    LATEST=$(aws s3 ls "s3://${BUCKET}/backups/" --region ap-southeast-2 | tail -1)
    if [ -n "$LATEST" ]; then
        echo "[backup] Latest: $LATEST"
    else
        echo "[backup] WARNING: No backups found!"
        exit 1
    fi
    exit 0
fi

# Get DB size for logging
DB_SIZE=$(du -h "$DB_PATH" | cut -f1)

echo "[backup] Uploading memoria.db (${DB_SIZE}) to s3://${BUCKET}/${S3_KEY}"
aws s3 cp "$DB_PATH" "s3://${BUCKET}/${S3_KEY}" \
    --region ap-southeast-2 \
    --quiet

echo "[backup] Done. Backup: s3://${BUCKET}/${S3_KEY}"
