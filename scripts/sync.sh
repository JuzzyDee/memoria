#!/bin/bash
# sync.sh — Bidirectional merge sync between local and remote Memoria databases
#
# Merges unique memories from both sides while respecting tombstones.
#
# Usage:
#   ./scripts/sync.sh                    # Sync with default remote
#   ./scripts/sync.sh user@host          # Sync with specific remote

REMOTE="${1:-justindavis@100.74.207.81}"
LOCAL_DB="$HOME/.memoria/memoria.db"
REMOTE_DB_PATH=".memoria/memoria.db"
TEMP_DIR=$(mktemp -d)
REMOTE_COPY="$TEMP_DIR/remote.db"

echo "═══ Memoria Sync ═══"
echo "Local:  $LOCAL_DB"
echo "Remote: $REMOTE:$REMOTE_DB_PATH"
echo ""

# Step 1: Pull remote DB to temp location
echo "Pulling remote database..."
scp "$REMOTE:$REMOTE_DB_PATH" "$REMOTE_COPY" 2>/dev/null
if [ ! -f "$REMOTE_COPY" ]; then
    echo "Error: Could not pull remote database"
    rm -rf "$TEMP_DIR"
    exit 1
fi

# Step 2: Ensure tombstone tables exist on both
sqlite3 "$LOCAL_DB" "CREATE TABLE IF NOT EXISTS tombstones (memory_id TEXT PRIMARY KEY, forgotten_at TEXT NOT NULL);" 2>/dev/null
sqlite3 "$REMOTE_COPY" "CREATE TABLE IF NOT EXISTS tombstones (memory_id TEXT PRIMARY KEY, forgotten_at TEXT NOT NULL);" 2>/dev/null

# Step 3: Collect all tombstones from both sides
echo "Checking tombstones..."
sqlite3 "$LOCAL_DB" "SELECT memory_id FROM tombstones;" > "$TEMP_DIR/local_tombstones.txt" 2>/dev/null
sqlite3 "$REMOTE_COPY" "SELECT memory_id FROM tombstones;" > "$TEMP_DIR/remote_tombstones.txt" 2>/dev/null
cat "$TEMP_DIR/local_tombstones.txt" "$TEMP_DIR/remote_tombstones.txt" | sort -u > "$TEMP_DIR/all_tombstones.txt"

# Step 4: Get memory IDs from both sides
sqlite3 "$LOCAL_DB" "SELECT id FROM memories;" | sort > "$TEMP_DIR/local_ids.txt"
sqlite3 "$REMOTE_COPY" "SELECT id FROM memories;" | sort > "$TEMP_DIR/remote_ids.txt"

# Find unique IDs
comm -23 "$TEMP_DIR/local_ids.txt" "$TEMP_DIR/remote_ids.txt" > "$TEMP_DIR/only_local.txt"
comm -13 "$TEMP_DIR/local_ids.txt" "$TEMP_DIR/remote_ids.txt" > "$TEMP_DIR/only_remote.txt"

# Filter out tombstoned IDs
if [ -s "$TEMP_DIR/all_tombstones.txt" ]; then
    grep -v -F -f "$TEMP_DIR/all_tombstones.txt" "$TEMP_DIR/only_local.txt" > "$TEMP_DIR/only_local_filtered.txt" 2>/dev/null || true
    grep -v -F -f "$TEMP_DIR/all_tombstones.txt" "$TEMP_DIR/only_remote.txt" > "$TEMP_DIR/only_remote_filtered.txt" 2>/dev/null || true
    mv "$TEMP_DIR/only_local_filtered.txt" "$TEMP_DIR/only_local.txt" 2>/dev/null || true
    mv "$TEMP_DIR/only_remote_filtered.txt" "$TEMP_DIR/only_remote.txt" 2>/dev/null || true
fi

LOCAL_COUNT=$(wc -l < "$TEMP_DIR/only_local.txt" | tr -d ' ')
REMOTE_COUNT=$(wc -l < "$TEMP_DIR/only_remote.txt" | tr -d ' ')

echo "Comparing databases..."
echo "  Unique to local:  $LOCAL_COUNT memories"
echo "  Unique to remote: $REMOTE_COUNT memories"
echo ""

# Step 5: Import remote → local using ATTACH
if [ "$REMOTE_COUNT" -gt 0 ]; then
    echo "Importing from remote → local..."
    # Build a comma-separated list of IDs for SQL IN clause
    ID_LIST=$(awk '{printf "\x27%s\x27,", $0}' "$TEMP_DIR/only_remote.txt" | sed 's/,$//')

    sqlite3 "$LOCAL_DB" "ATTACH '$REMOTE_COPY' AS remote;
INSERT OR IGNORE INTO memories SELECT * FROM remote.memories WHERE id IN ($ID_LIST);
DETACH remote;" 2>/dev/null

    while read -r id; do
        [ -z "$id" ] && continue
        echo "  ← $(echo $id | cut -c1-8)"
    done < "$TEMP_DIR/only_remote.txt"
fi

# Step 6: Export local → remote using ATTACH on the remote copy, then push
if [ "$LOCAL_COUNT" -gt 0 ]; then
    echo "Exporting from local → remote..."
    ID_LIST=$(awk '{printf "\x27%s\x27,", $0}' "$TEMP_DIR/only_local.txt" | sed 's/,$//')

    sqlite3 "$REMOTE_COPY" "ATTACH '$LOCAL_DB' AS local;
INSERT OR IGNORE INTO memories SELECT * FROM local.memories WHERE id IN ($ID_LIST);
DETACH local;" 2>/dev/null

    while read -r id; do
        [ -z "$id" ] && continue
        echo "  → $(echo $id | cut -c1-8)"
    done < "$TEMP_DIR/only_local.txt"

    # Push updated remote copy back
    scp "$REMOTE_COPY" "$REMOTE:$REMOTE_DB_PATH" 2>/dev/null
fi

# Step 7: Sync tombstones both ways
if [ -s "$TEMP_DIR/all_tombstones.txt" ]; then
    echo "Syncing tombstones..."
    while read -r tid; do
        [ -z "$tid" ] && continue
        NOW=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
        sqlite3 "$LOCAL_DB" "INSERT OR IGNORE INTO tombstones (memory_id, forgotten_at) VALUES ('$tid', '$NOW');" 2>/dev/null
        sqlite3 "$REMOTE_COPY" "INSERT OR IGNORE INTO tombstones (memory_id, forgotten_at) VALUES ('$tid', '$NOW');" 2>/dev/null
        # Also delete the actual memory if it exists despite tombstone
        sqlite3 "$LOCAL_DB" "DELETE FROM co_activations WHERE memory_a = '$tid' OR memory_b = '$tid'; DELETE FROM memories WHERE id = '$tid';" 2>/dev/null
    done < "$TEMP_DIR/all_tombstones.txt"

    # Push tombstone updates to remote
    scp "$REMOTE_COPY" "$REMOTE:$REMOTE_DB_PATH" 2>/dev/null
fi

# Cleanup
rm -rf "$TEMP_DIR"

# Report
echo ""
echo "── Sync Complete ──"
sqlite3 "$LOCAL_DB" "SELECT memory_type, COUNT(*) FROM memories GROUP BY memory_type;" | while IFS='|' read -r type count; do
    echo "  $type: $count"
done
echo ""
echo "═══ Done ═══"
