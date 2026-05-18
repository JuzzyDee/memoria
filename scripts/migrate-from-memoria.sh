#!/usr/bin/env bash
# migrate-from-memoria.sh — one-time migration helper for users who
# deployed Oneiro under its previous name (memoria) before the CLA-97
# rebrand.
#
# Stages:
#   1. Preflight   — verify source + dest resources, dest schema applied
#   2. D1          — export memoria-db, strip schema, apply to oneiro-db
#   3. Vectorize   — getByIds in batches from memoria-vectors → upsert into oneiro-vectors
#   4. R2          — list memoria-images keys, copy each to oneiro-images
#   5. Verify      — count rows / vectors / objects in dest, report deltas
#
# Properties:
#   - Read-only on source. Decommission of memoria-* resources is a
#     separate manual step (wrangler delete, or via the CF dashboard)
#     once you've verified the migrated copy.
#   - Idempotent: D1 inserts are OR IGNORE, vectorize insert is upsert
#     semantics by id, R2 put overwrites by key. Safe to re-run after
#     fixing whatever broke.
#   - Skips: --skip-d1 / --skip-vectors / --skip-r2 for partial re-runs.
#   - Dry-run: --dry-run prints what would happen without writes.

set -euo pipefail

# ──── Colors + helpers ──────────────────────────────────────────────────

if [ -t 1 ]; then
    BOLD=$(printf '\033[1m'); DIM=$(printf '\033[2m')
    RED=$(printf '\033[31m'); GREEN=$(printf '\033[32m')
    YELLOW=$(printf '\033[33m'); BLUE=$(printf '\033[34m')
    RESET=$(printf '\033[0m')
else
    BOLD=''; DIM=''; RED=''; GREEN=''; YELLOW=''; BLUE=''; RESET=''
fi

say()    { printf '%s\n' "$*"; }
ok()     { printf '%s✓%s %s\n' "$GREEN" "$RESET" "$*"; }
warn()   { printf '%s!%s %s\n' "$YELLOW" "$RESET" "$*"; }
err()    { printf '%s✗%s %s\n' "$RED" "$RESET" "$*" >&2; }
dim()    { printf '%s%s%s\n' "$DIM" "$*" "$RESET"; }
header() { printf '\n%s━━ %s ━━%s\n' "$BOLD" "$*" "$RESET"; }

# ──── Argument parsing ─────────────────────────────────────────────────

SOURCE_DB="memoria-db"
SOURCE_VECTORS="memoria-vectors"
SOURCE_IMAGES="memoria-images"
DRY_RUN=false
SKIP_D1=false
SKIP_VECTORS=false
SKIP_R2=false
VECTORIZE_BATCH=100

usage() {
    cat <<EOF
Usage: $0 [options]

Options:
  --source-db NAME         Source D1 database name           (default: memoria-db)
  --source-vectors NAME    Source Vectorize index name       (default: memoria-vectors)
  --source-images NAME     Source R2 bucket name             (default: memoria-images)
  --vectorize-batch N      Vectors per get/insert call       (default: 100)
  --skip-d1                Skip D1 migration stage
  --skip-vectors           Skip Vectorize migration stage
  --skip-r2                Skip R2 migration stage
  --dry-run                Print what would happen, no writes
  -h, --help               Show this help

The destination resources (oneiro-db, oneiro-vectors, oneiro-images) are
read from your current wrangler.toml. Run scripts/setup.sh first if you
haven't created them yet.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --source-db)        SOURCE_DB="$2"; shift 2 ;;
        --source-vectors)   SOURCE_VECTORS="$2"; shift 2 ;;
        --source-images)    SOURCE_IMAGES="$2"; shift 2 ;;
        --vectorize-batch)  VECTORIZE_BATCH="$2"; shift 2 ;;
        --skip-d1)          SKIP_D1=true; shift ;;
        --skip-vectors)     SKIP_VECTORS=true; shift ;;
        --skip-r2)          SKIP_R2=true; shift ;;
        --dry-run)          DRY_RUN=true; shift ;;
        -h|--help)          usage; exit 0 ;;
        *)                  err "Unknown option: $1"; usage; exit 1 ;;
    esac
done

# ──── Read destination from wrangler.toml ──────────────────────────────

if [ ! -f wrangler.toml ]; then
    err "wrangler.toml not found in current directory."
    err "Run from the repo root after scripts/setup.sh has populated it."
    exit 1
fi

DEST_DB=$(awk -F'"' '/^database_name/ {print $2; exit}' wrangler.toml)
DEST_VECTORS=$(awk -F'"' '/^index_name/ {print $2; exit}' wrangler.toml)
DEST_IMAGES=$(awk -F'"' '/^bucket_name/ {print $2; exit}' wrangler.toml)

if [ -z "$DEST_DB" ] || [ -z "$DEST_VECTORS" ] || [ -z "$DEST_IMAGES" ]; then
    err "Couldn't read destination names from wrangler.toml."
    err "Expected database_name, index_name, and bucket_name entries."
    exit 1
fi

header "Oneiro migration"
say "  Source:  ${BOLD}${SOURCE_DB}${RESET} / ${BOLD}${SOURCE_VECTORS}${RESET} / ${BOLD}${SOURCE_IMAGES}${RESET}"
say "  Dest:    ${BOLD}${DEST_DB}${RESET} / ${BOLD}${DEST_VECTORS}${RESET} / ${BOLD}${DEST_IMAGES}${RESET}"
if $DRY_RUN; then
    say "  Mode:    ${YELLOW}dry-run (no writes)${RESET}"
fi

if [ "$SOURCE_DB" = "$DEST_DB" ]; then
    err "Source and destination D1 are the same. Refusing to migrate onto itself."
    exit 1
fi

# jq is required for both Vectorize NDJSON shaping and the verification
# row-counts. Fail early — better than completing D1 and then dying
# halfway through Vectorize on a missing dependency.
if ! $SKIP_VECTORS && ! command -v jq >/dev/null 2>&1; then
    err "jq is required for Vectorize migration (NDJSON shaping)."
    err "Install jq (brew install jq / apt install jq) and re-run."
    err "Or pass --skip-vectors if you'll handle vectors separately."
    exit 1
fi

# Live-writes warning. The script reads the source D1 at export time;
# any writes to the old worker AFTER that point won't be in the migrated
# copy unless you re-run the migration. Cleanest path: stop the old
# worker's cron + dispatch first, OR plan to re-run after connector
# cutover to sweep stragglers.
say ""
warn "The old worker stays live during migration."
warn "Any writes to it AFTER the D1 export point won't be in the migrated copy"
warn "until you re-run this script. For a clean cutover, either:"
warn "  • Pause the old worker (wrangler triggers delete + secret put ${BOLD}MEMORIA_DIALECTIC_DISPATCH=off${RESET})"
warn "  • Or re-run this script after Claude-clients are pointed at the new worker"

# ──── Preflight ────────────────────────────────────────────────────────

header "[1/4] Preflight"

say "  Checking source D1 exists..."
if ! wrangler d1 list 2>/dev/null | grep -q "$SOURCE_DB"; then
    err "Source D1 '${SOURCE_DB}' not found in your account."
    exit 1
fi
ok "Source D1: $SOURCE_DB"

say "  Checking destination D1 exists..."
if ! wrangler d1 list 2>/dev/null | grep -q "$DEST_DB"; then
    err "Destination D1 '${DEST_DB}' not found. Run scripts/setup.sh first."
    exit 1
fi
ok "Destination D1: $DEST_DB"

say "  Checking destination schema applied (memories table present)..."
if ! wrangler d1 execute "$DEST_DB" --remote --command="SELECT 1 FROM memories LIMIT 1" >/dev/null 2>&1; then
    err "Destination D1 has no 'memories' table. Apply migrations first:"
    err "  wrangler d1 migrations apply ${DEST_DB} --remote"
    exit 1
fi
ok "Destination schema applied"

say "  Checking R2 buckets..."
R2_LIST=$(wrangler r2 bucket list 2>/dev/null || true)
if ! echo "$R2_LIST" | grep -q "$SOURCE_IMAGES"; then
    warn "Source R2 bucket '${SOURCE_IMAGES}' not visible — will skip R2 if it doesn't exist."
fi
if ! echo "$R2_LIST" | grep -q "$DEST_IMAGES"; then
    err "Destination R2 bucket '${DEST_IMAGES}' not found. Run scripts/setup.sh first."
    exit 1
fi
ok "R2 buckets present"

# ──── Stage 2: D1 ──────────────────────────────────────────────────────

TMPDIR=$(mktemp -d -t oneiro-migrate.XXXXXX)
# Only clean tmpdir if the script exits cleanly. On failure we want the
# SQL dump, transformed data file, and any captured wrangler logs to
# stick around so the operator can investigate.
MIGRATION_OK=false
cleanup_tmpdir() {
    if $MIGRATION_OK; then
        rm -rf "$TMPDIR"
    else
        warn "Migration did not complete cleanly — preserving tmpdir for inspection:"
        warn "  $TMPDIR"
    fi
}
trap cleanup_tmpdir EXIT

if $SKIP_D1; then
    header "[2/4] D1 migration — ${YELLOW}skipped${RESET}"
else
    header "[2/4] D1 migration"

    DUMP_FILE="$TMPDIR/source-dump.sql"
    DATA_FILE="$TMPDIR/data-only.sql"

    say "  Exporting ${SOURCE_DB} (data only, no schema)..."
    if $DRY_RUN; then
        dim "[dry-run] wrangler d1 export ${SOURCE_DB} --remote --no-schema --output=${DUMP_FILE}"
    else
        # --no-schema tells wrangler to skip CREATE TABLE / CREATE INDEX
        # / PRAGMA / BEGIN / COMMIT entirely; we only get the data
        # INSERTs. The destination already has its schema from setup.sh
        # running migrations 0001–0006, so we don't want any of that
        # anyway. Earlier versions of this script burned a lot of regex
        # cycles trying to filter schema out post-hoc, including a
        # multi-line CREATE INDEX that produced misleading "near ON at
        # offset 5" errors. Letting wrangler omit schema at the source
        # is the right answer — and per Justin: "why are we creating
        # tables anyway? setup.sh already did that."
        if ! wrangler d1 export "$SOURCE_DB" --remote --no-schema --output="$DUMP_FILE" 2>&1 | tail -5; then
            err "D1 export failed."
            exit 1
        fi
        ok "Exported to $(wc -l < "$DUMP_FILE") lines"
    fi

    say "  Filtering D1 internal-table INSERTs..."
    if $DRY_RUN; then
        dim "[dry-run] sed transform → ${DATA_FILE}"
    else
        # Even with --no-schema, wrangler still emits INSERTs for D1's
        # internal bookkeeping tables (d1_migrations, sqlite_sequence,
        # _cf*). Those belong to CF's per-database state — dest has its
        # own copies from setup.sh — so we drop them here. Application
        # INSERTs pass through unchanged; they use the dump's upsert
        # syntax (`ON CONFLICT(id) DO UPDATE SET ...`) which is
        # idempotent on re-runs.
        #
        # `"?` in each pattern makes the leading quote optional — wrangler
        # currently quotes table names but a future version might not.
        sed -E \
            -e '/^INSERT INTO "?d1_migrations"?/d' \
            -e '/^INSERT INTO "?sqlite_sequence"?/d' \
            -e '/^INSERT INTO "?_cf/d' \
            "$DUMP_FILE" > "$DATA_FILE"
        INSERTS=$(grep -c "^INSERT" "$DATA_FILE" || true)
        ok "Filtered: $INSERTS INSERT statements ready"
    fi

    say "  Applying to ${DEST_DB}..."
    if $DRY_RUN; then
        dim "[dry-run] wrangler d1 execute ${DEST_DB} --remote --file=${DATA_FILE}"
    else
        EXEC_LOG="$TMPDIR/d1-import.log"
        if wrangler d1 execute "$DEST_DB" --remote --file="$DATA_FILE" > "$EXEC_LOG" 2>&1; then
            tail -3 "$EXEC_LOG"
            ok "D1 import complete"
        else
            err "D1 import failed. Last 30 lines of wrangler output:"
            tail -30 "$EXEC_LOG" | sed 's/^/    /' >&2
            err "Full output preserved at: ${EXEC_LOG}"
            err "Transformed SQL preserved at: ${DATA_FILE}"
            err "Source dump preserved at:    ${DUMP_FILE}"
            err "(tmpdir is not deleted on failure — inspect freely)"
            exit 1
        fi
    fi
fi

# ──── Stage 3: Vectorize ───────────────────────────────────────────────

if $SKIP_VECTORS; then
    header "[3/4] Vectorize migration — ${YELLOW}skipped${RESET}"
else
    header "[3/4] Vectorize migration"

    say "  Collecting memory IDs from ${DEST_DB}..."
    if $DRY_RUN; then
        dim "[dry-run] would list memory IDs from D1"
        ID_COUNT=0
    else
        IDS_JSON=$(wrangler d1 execute "$DEST_DB" --remote --json --command="SELECT id FROM memories" 2>/dev/null || echo "[]")
        # jq is preflight-required at this point, so the JSON parse is direct.
        echo "$IDS_JSON" | jq -r '.[0].results[]?.id // empty' > "$TMPDIR/ids.txt"
        ID_COUNT=$(wc -l < "$TMPDIR/ids.txt" | tr -d ' ')
        ok "Found $ID_COUNT memory IDs"
    fi

    if [ "$ID_COUNT" -gt 0 ] && ! $DRY_RUN; then
        say "  Migrating vectors in batches of ${VECTORIZE_BATCH}..."
        BATCH=0
        TOTAL_VECTORS=0

        while [ -s "$TMPDIR/ids.txt" ]; do
            head -n "$VECTORIZE_BATCH" "$TMPDIR/ids.txt" > "$TMPDIR/batch.txt"
            tail -n "+$((VECTORIZE_BATCH + 1))" "$TMPDIR/ids.txt" > "$TMPDIR/ids.next" && mv "$TMPDIR/ids.next" "$TMPDIR/ids.txt"

            BATCH=$((BATCH + 1))
            IDS_CSV=$(paste -sd, "$TMPDIR/batch.txt")
            BATCH_COUNT=$(wc -l < "$TMPDIR/batch.txt" | tr -d ' ')

            # Get vectors from source as JSON. Capture stderr to a per-batch
            # log so failures surface inline — earlier we swallowed stderr
            # with `2>/dev/null` and got silent "0 vectors copied"
            # outcomes with no diagnostic.
            VEC_ERR="$TMPDIR/vec-batch-${BATCH}.err"
            if ! VEC_JSON=$(wrangler vectorize get-vectors "$SOURCE_VECTORS" --ids="$IDS_CSV" 2>"$VEC_ERR"); then
                warn "Batch $BATCH: get-vectors from $SOURCE_VECTORS failed:"
                head -10 "$VEC_ERR" | sed 's/^/      /' >&2
                continue
            fi

            # Transform to NDJSON for vectorize insert. Each line:
            #   {"id":"...","values":[...],"metadata":{...}}
            # jq guaranteed by preflight.
            echo "$VEC_JSON" | jq -c '.[] | {id, values, metadata}' > "$TMPDIR/batch.ndjson"

            VECS_IN_BATCH=$(wc -l < "$TMPDIR/batch.ndjson" | tr -d ' ')
            if [ "$VECS_IN_BATCH" -eq 0 ]; then
                continue
            fi

            INS_ERR="$TMPDIR/vec-ins-${BATCH}.err"
            if ! wrangler vectorize insert "$DEST_VECTORS" --file="$TMPDIR/batch.ndjson" >/dev/null 2>"$INS_ERR"; then
                warn "Batch $BATCH: insert into $DEST_VECTORS failed:"
                head -10 "$INS_ERR" | sed 's/^/      /' >&2
                continue
            fi

            TOTAL_VECTORS=$((TOTAL_VECTORS + VECS_IN_BATCH))
            dim "  batch $BATCH: $VECS_IN_BATCH vectors copied (total $TOTAL_VECTORS / $ID_COUNT)"
        done
        ok "Vectorize migration complete: $TOTAL_VECTORS vectors"
    elif $DRY_RUN; then
        dim "[dry-run] would batch-copy vectors via get-vectors + insert"
    fi
fi

# ──── Stage 4: R2 ──────────────────────────────────────────────────────

if $SKIP_R2; then
    header "[4/4] R2 migration — ${YELLOW}skipped${RESET}"
else
    header "[4/4] R2 migration"

    say "  Listing objects in ${SOURCE_IMAGES}..."
    if $DRY_RUN; then
        dim "[dry-run] would list + copy each object key"
        KEY_COUNT=0
    else
        R2_LIST_ERR="$TMPDIR/r2-list.err"
        if ! R2_LIST=$(wrangler r2 object list "$SOURCE_IMAGES" --remote 2>"$R2_LIST_ERR"); then
            warn "Couldn't list ${SOURCE_IMAGES}:"
            head -10 "$R2_LIST_ERR" | sed 's/^/      /' >&2
            warn "Skipping R2 stage."
            KEY_COUNT=0
        else
            # wrangler r2 object list output format varies by version. Try
            # JSON parse first; fall back to whitespace parse.
            if command -v jq >/dev/null 2>&1; then
                echo "$R2_LIST" | jq -r '.[]?.key // empty' > "$TMPDIR/r2-keys.txt" 2>/dev/null || \
                    echo "$R2_LIST" | awk 'NR>1 {print $1}' > "$TMPDIR/r2-keys.txt"
            else
                echo "$R2_LIST" | awk 'NR>1 {print $1}' > "$TMPDIR/r2-keys.txt"
            fi
            # Filter out blank/header lines
            grep -E '\.(jpg|jpeg|png|webp)$' "$TMPDIR/r2-keys.txt" > "$TMPDIR/r2-keys.filtered" || true
            mv "$TMPDIR/r2-keys.filtered" "$TMPDIR/r2-keys.txt"
            KEY_COUNT=$(wc -l < "$TMPDIR/r2-keys.txt" | tr -d ' ')
            ok "Found $KEY_COUNT image keys"
        fi
    fi

    if [ "$KEY_COUNT" -gt 0 ] && ! $DRY_RUN; then
        say "  Copying objects (this can take a while for large buckets)..."
        COPIED=0
        FAILED=0
        while IFS= read -r KEY; do
            [ -z "$KEY" ] && continue
            TMP_OBJ="$TMPDIR/obj-$$"
            if wrangler r2 object get "$SOURCE_IMAGES/$KEY" --remote --file="$TMP_OBJ" >/dev/null 2>&1 && \
               wrangler r2 object put "$DEST_IMAGES/$KEY" --remote --file="$TMP_OBJ" >/dev/null 2>&1; then
                COPIED=$((COPIED + 1))
            else
                FAILED=$((FAILED + 1))
                warn "  failed: $KEY"
            fi
            rm -f "$TMP_OBJ"
            if [ $((COPIED % 10)) -eq 0 ] && [ "$COPIED" -gt 0 ]; then
                dim "  copied $COPIED / $KEY_COUNT"
            fi
        done < "$TMPDIR/r2-keys.txt"
        ok "R2 migration: $COPIED copied, $FAILED failed"
    fi
fi

# ──── Verification ─────────────────────────────────────────────────────

header "Verification"

if ! $DRY_RUN; then
    DEST_COUNT=$(wrangler d1 execute "$DEST_DB" --remote --json --command="SELECT COUNT(*) AS n FROM memories" 2>/dev/null \
                 | (jq -r '.[0].results[0].n // 0' 2>/dev/null || echo "?"))
    SRC_COUNT=$(wrangler d1 execute "$SOURCE_DB" --remote --json --command="SELECT COUNT(*) AS n FROM memories" 2>/dev/null \
                | (jq -r '.[0].results[0].n // 0' 2>/dev/null || echo "?"))
    say "  D1 memories:   source=${SRC_COUNT}   dest=${DEST_COUNT}"

    TOMB_SRC=$(wrangler d1 execute "$SOURCE_DB" --remote --json --command="SELECT COUNT(*) AS n FROM tombstones" 2>/dev/null \
               | (jq -r '.[0].results[0].n // 0' 2>/dev/null || echo "?"))
    TOMB_DST=$(wrangler d1 execute "$DEST_DB" --remote --json --command="SELECT COUNT(*) AS n FROM tombstones" 2>/dev/null \
               | (jq -r '.[0].results[0].n // 0' 2>/dev/null || echo "?"))
    say "  D1 tombstones: source=${TOMB_SRC}   dest=${TOMB_DST}"
fi

header "Done"
say "  Next steps:"
say "    1. Deploy the renamed worker:      ${BOLD}wrangler deploy${RESET}"
say "    2. Update the MCP connector at:    ${BOLD}https://claude.ai/settings/connectors${RESET}"
say "       Point at: https://${DEST_DB%-db}.\${YOUR_SUBDOMAIN}.workers.dev"
say "    3. Run a recall in Claude. Confirm your memories surface."
say "    4. When you're satisfied, decommission the old worker:"
say "       ${DIM}wrangler delete --name memoria${RESET}"
say "       ${DIM}wrangler d1 delete ${SOURCE_DB}${RESET}"
say "       ${DIM}wrangler vectorize delete ${SOURCE_VECTORS}${RESET}"
say "       ${DIM}wrangler r2 bucket delete ${SOURCE_IMAGES}${RESET}"

# Signal the trap that we got here cleanly — tmpdir is safe to remove.
MIGRATION_OK=true
