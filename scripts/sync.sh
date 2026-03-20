#!/bin/bash
# sync.sh — Bidirectional merge sync between local and remote Memoria databases
#
# Instead of blindly overwriting one with the other, this merges unique
# memories from both sides while respecting tombstones (forgotten memories).
#
# Usage:
#   ./scripts/sync.sh                    # Sync with default remote
#   ./scripts/sync.sh user@host          # Sync with specific remote
#
# Requires: sqlite3, ssh, scp

REMOTE="${1:-justindavis@100.74.207.81}"
LOCAL_DB="$HOME/.memoria/memoria.db"
REMOTE_DB_PATH=".memoria/memoria.db"
TEMP_DIR=$(mktemp -d)
REMOTE_COPY="$TEMP_DIR/remote.db"

echo "═══ Memoria Sync ═══"
echo "Local:  $LOCAL_DB"
echo "Remote: $REMOTE:$REMOTE_DB_PATH"
echo ""

# Step 1: Pull remote DB to temp location (don't overwrite local)
echo "Pulling remote database..."
scp "$REMOTE:$REMOTE_DB_PATH" "$REMOTE_COPY" 2>/dev/null

# Step 2: Get tombstones from both sides
echo "Checking tombstones..."
LOCAL_TOMBSTONES=$(sqlite3 "$LOCAL_DB" "SELECT memory_id FROM tombstones;" 2>/dev/null || echo "")
REMOTE_TOMBSTONES=$(sqlite3 "$REMOTE_COPY" "SELECT memory_id FROM tombstones;" 2>/dev/null || echo "")
ALL_TOMBSTONES=$(echo -e "$LOCAL_TOMBSTONES\n$REMOTE_TOMBSTONES" | sort -u | grep -v '^$')

# Step 3: Find memories unique to each side
echo "Comparing databases..."
LOCAL_IDS=$(sqlite3 "$LOCAL_DB" "SELECT id FROM memories;" | sort)
REMOTE_IDS=$(sqlite3 "$REMOTE_COPY" "SELECT id FROM memories;" | sort)

# Memories in local but not remote
ONLY_LOCAL=$(comm -23 <(echo "$LOCAL_IDS") <(echo "$REMOTE_IDS"))
# Memories in remote but not local
ONLY_REMOTE=$(comm -13 <(echo "$LOCAL_IDS") <(echo "$REMOTE_IDS"))

# Filter out tombstoned memories
if [ -n "$ALL_TOMBSTONES" ]; then
    ONLY_LOCAL=$(echo "$ONLY_LOCAL" | grep -v -F "$ALL_TOMBSTONES" 2>/dev/null || echo "")
    ONLY_REMOTE=$(echo "$ONLY_REMOTE" | grep -v -F "$ALL_TOMBSTONES" 2>/dev/null || echo "")
fi

LOCAL_COUNT=$(echo "$ONLY_LOCAL" | grep -c -v '^$' 2>/dev/null || echo "0")
REMOTE_COUNT=$(echo "$ONLY_REMOTE" | grep -c -v '^$' 2>/dev/null || echo "0")

echo "  Unique to local:  $LOCAL_COUNT memories"
echo "  Unique to remote: $REMOTE_COUNT memories"
echo ""

# Step 4: Copy unique remote memories to local
if [ "$REMOTE_COUNT" -gt 0 ]; then
    echo "Importing from remote → local..."
    echo "$ONLY_REMOTE" | while read -r id; do
        [ -z "$id" ] && continue
        sqlite3 "$REMOTE_COPY" ".mode insert memories
SELECT * FROM memories WHERE id = '$id';" | sqlite3 "$LOCAL_DB" 2>/dev/null
        echo "  ← $id"
    done
fi

# Step 5: Copy unique local memories to remote (via temp file)
if [ "$LOCAL_COUNT" -gt 0 ]; then
    echo "Exporting from local → remote..."
    EXPORT_SQL="$TEMP_DIR/export.sql"
    > "$EXPORT_SQL"
    echo "$ONLY_LOCAL" | while read -r id; do
        [ -z "$id" ] && continue
        sqlite3 "$LOCAL_DB" ".mode insert memories
SELECT * FROM memories WHERE id = '$id';" >> "$EXPORT_SQL" 2>/dev/null
        echo "  → $id"
    done

    if [ -s "$EXPORT_SQL" ]; then
        scp "$EXPORT_SQL" "$REMOTE:/tmp/memoria_import.sql" 2>/dev/null
        ssh "$REMOTE" "sqlite3 $REMOTE_DB_PATH < /tmp/memoria_import.sql && rm /tmp/memoria_import.sql" 2>/dev/null
    fi
fi

# Step 6: Sync tombstones both ways
if [ -n "$ALL_TOMBSTONES" ]; then
    echo "Syncing tombstones..."
    echo "$ALL_TOMBSTONES" | while read -r tid; do
        [ -z "$tid" ] && continue
        NOW=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
        sqlite3 "$LOCAL_DB" "INSERT OR IGNORE INTO tombstones (memory_id, forgotten_at) VALUES ('$tid', '$NOW');" 2>/dev/null
    done
    # Push tombstones to remote
    TOMBSTONE_SQL="$TEMP_DIR/tombstones.sql"
    sqlite3 "$LOCAL_DB" ".mode insert tombstones
SELECT * FROM tombstones;" > "$TOMBSTONE_SQL" 2>/dev/null
    if [ -s "$TOMBSTONE_SQL" ]; then
        scp "$TOMBSTONE_SQL" "$REMOTE:/tmp/memoria_tombstones.sql" 2>/dev/null
        ssh "$REMOTE" "sqlite3 $REMOTE_DB_PATH 'DELETE FROM tombstones;' && sqlite3 $REMOTE_DB_PATH < /tmp/memoria_tombstones.sql && rm /tmp/memoria_tombstones.sql" 2>/dev/null
    fi
fi

# Step 7: Clean up any tombstoned memories that exist on either side
if [ -n "$ALL_TOMBSTONES" ]; then
    echo "Cleaning tombstoned memories..."
    echo "$ALL_TOMBSTONES" | while read -r tid; do
        [ -z "$tid" ] && continue
        sqlite3 "$LOCAL_DB" "DELETE FROM co_activations WHERE memory_a = '$tid' OR memory_b = '$tid'; DELETE FROM memories WHERE id = '$tid';" 2>/dev/null
    done
    ssh "$REMOTE" "echo \"$ALL_TOMBSTONES\" | while read -r tid; do [ -z \"\$tid\" ] && continue; sqlite3 $REMOTE_DB_PATH \"DELETE FROM co_activations WHERE memory_a = '\$tid' OR memory_b = '\$tid'; DELETE FROM memories WHERE id = '\$tid';\"; done" 2>/dev/null
fi

# Cleanup
rm -rf "$TEMP_DIR"

# Report final state
echo ""
echo "── Sync Complete ──"
LOCAL_COUNTS=$(sqlite3 "$LOCAL_DB" "SELECT memory_type, COUNT(*) FROM memories GROUP BY memory_type;")
echo "Local store: $LOCAL_COUNTS"
echo ""
echo "═══ Done ═══"
