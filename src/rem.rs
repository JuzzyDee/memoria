#![allow(dead_code)] // embed module is shared with MCP server — not all functions used here
// rem.rs — REM processing engine
//
// The overnight maintenance cycle for Memoria. Named after the sleep stage
// where the brain consolidates memories — replaying, reorganising, pruning.
//
// This binary runs via cron, does its work, and exits. No server, no API
// calls, just pure computation on the SQLite database:
//
// 1. Apply Ebbinghaus decay to all non-orientation memories
// 2. Report what changed (memories weakened, memories effectively forgotten)
// 3. Future: Hebbian consolidation, pruning, re-embedding
//
// The circadian rhythm:
//   Daytime   — active reflection via Claude Code (inference, expensive)
//   Overnight — REM processing via this binary (computation, free)

mod embed;
mod store;

use std::path::PathBuf;

use store::MemoryStore;

fn main() {
    let db_path = std::env::var("MEMORIA_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut path = dirs_or_default();
            path.push("memoria.db");
            path
        });

    if !db_path.exists() {
        eprintln!(
            "No database found at {}. Nothing to process.",
            db_path.display()
        );
        std::process::exit(0);
    }

    println!("═══ Memoria REM Processing ═══");
    println!("Database: {}", db_path.display());
    println!(
        "Time: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!();

    let store = match MemoryStore::open(&db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to open database: {}", e);
            std::process::exit(1);
        }
    };

    // --- Phase 1: Snapshot before decay ---
    let (ep_before, sem_before, ori_before) = store.count_by_type().unwrap_or((0, 0, 0));
    let total_before = ep_before + sem_before + ori_before;
    println!(
        "Memory store: {} total ({} episodic, {} semantic, {} orientation)",
        total_before, ep_before, sem_before, ori_before
    );

    // Get pre-decay strengths for reporting
    let pre_decay = store.recall_active(0.0, 1000).unwrap_or_default();
    println!("Non-orientation memories: {}", pre_decay.len());
    println!();

    // --- Phase 2: Apply Ebbinghaus decay ---
    println!("── Applying Ebbinghaus decay ──");
    let decayed_count = match store.apply_decay() {
        Ok(n) => n,
        Err(e) => {
            eprintln!("Decay failed: {}", e);
            std::process::exit(1);
        }
    };
    println!("Processed {} memories", decayed_count);

    // --- Phase 3: Report changes ---
    let post_decay = store.recall_active(0.0, 1000).unwrap_or_default();

    let mut weakened = 0;
    let mut forgotten = 0; // below 0.1 threshold
    let mut faded = 0; // below 0.01 — effectively gone

    for memory in &post_decay {
        // Find the pre-decay version
        let pre_strength = pre_decay
            .iter()
            .find(|m| m.id == memory.id)
            .map(|m| m.strength)
            .unwrap_or(1.0);

        if memory.strength < pre_strength {
            weakened += 1;
        }
        if memory.strength < 0.1 {
            forgotten += 1;
        }
        if memory.strength < 0.01 {
            faded += 1;
        }
    }

    println!();
    println!("── Decay Report ──");
    println!("Weakened:  {} memories lost strength", weakened);
    println!(
        "Forgotten: {} memories below recall threshold (0.1)",
        forgotten
    );
    println!("Faded:     {} memories effectively gone (<0.01)", faded);
    println!();

    // Show individual memory states
    println!("── Memory States ──");
    for memory in &post_decay {
        let status = if memory.strength > 0.8 {
            "vivid"
        } else if memory.strength > 0.5 {
            "clear"
        } else if memory.strength > 0.1 {
            "fading"
        } else if memory.strength > 0.01 {
            "dim"
        } else {
            "forgotten"
        };

        let age = chrono::Utc::now() - memory.last_accessed;
        let age_str = if age.num_days() > 0 {
            format!("{}d", age.num_days())
        } else if age.num_hours() > 0 {
            format!("{}h", age.num_hours())
        } else {
            format!("{}m", age.num_minutes())
        };

        println!(
            "  [{:>9}] str:{:.4} stab:{:.1} age:{:>4} acc:{} | {}",
            status,
            memory.strength,
            memory.stability,
            age_str,
            memory.access_count,
            truncate(&memory.summary, 60),
        );
    }

    // --- Phase 4: Hebbian co-activation report ---
    let co_activations = store.get_co_activations(2, 20).unwrap_or_default();
    if !co_activations.is_empty() {
        println!();
        println!("── Hebbian Co-activations ──");
        println!("Memories frequently recalled together (consolidation candidates):");
        for (a, b, count) in &co_activations {
            let summary_a = store
                .get(a)
                .ok()
                .flatten()
                .map(|m| truncate(&m.summary, 40))
                .unwrap_or_else(|| a[..8].to_string());
            let summary_b = store
                .get(b)
                .ok()
                .flatten()
                .map(|m| truncate(&m.summary, 40))
                .unwrap_or_else(|| b[..8].to_string());
            println!("  [{:>3}x] {} ↔ {}", count, summary_a, summary_b);
        }
    }

    println!();
    println!("═══ REM complete ═══");
}

/// Truncate a string to a maximum length, adding "..." if truncated.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max - 3).collect();
        format!("{}...", truncated)
    }
}

/// Default data directory for Memoria.
fn dirs_or_default() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let mut path = PathBuf::from(home);
        path.push(".memoria");
        path
    } else {
        PathBuf::from(".memoria")
    }
}
