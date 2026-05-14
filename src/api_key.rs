// api_key.rs — Service API keys for Memoria
//
// Additive to OAuth (auth.rs). User-facing clients (Claude Code, Web, iOS)
// continue to flow through the OAuth path. Service callers — the rover,
// future automation — authenticate with static, hashed-at-rest API keys.
//
// Security shape (see CLA-86 ticket for the full design rationale):
//   1. Hashed at rest: memoria stores Argon2 hashes; raw keys exist only
//      on the client side.
//   2. Self-identifying format: mk_<role>_<32-byte-random>. The role is
//      readable from a leaked key at a glance.
//   3. Per-role capability allowlists (added in phase 3) — leaked keys
//      have bounded blast radius, can never reframe/forget/reflect.
//   4. Entity binding on writes (added in phase 4) — rover-role writes
//      are forced to entity="rover" server-side regardless of payload.
//   5. Per-key rate limits (added in phase 5).
//   6. Audit trail (added in phase 6) — every api-key-authenticated call
//      logs {timestamp, key_id, tool, success}.
//
// This file currently implements phase 1: key generation and the Role enum.

use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
use rand::Rng;
use sha2::{Digest, Sha256};

/// Roles supported for service API keys. Roles are hardcoded rather than
/// per-key configurable — adding a new role is a deliberate code change,
/// not a config change. This keeps the capability surface auditable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The rover's heartbeat loop. Reads + remember/remember_with_image,
    /// with entity forced to "rover" on writes (enforced in phase 4).
    Rover,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Rover => "rover",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "rover" => Some(Role::Rover),
            _ => None,
        }
    }
}

/// A freshly minted API key. The raw key exists only at generation time and
/// must be captured by the caller — it is never persisted by memoria and
/// is mathematically unrecoverable from the hash.
pub struct GeneratedKey {
    pub role: Role,
    /// The raw key value. Format: `mk_<role>_<32-byte-random>`. Goes in the
    /// client's environment (e.g. rover's .env as MEMORIA_MCP_TOKEN).
    pub raw: String,
    /// Argon2id hash of the raw key. Goes in memoria's MEMORIA_API_KEYS env
    /// var alongside the role.
    pub hash: String,
    /// Short stable identifier derived from the raw key (first 8 hex chars
    /// of SHA-256). Used in audit logs to identify which key was used
    /// without revealing it. Same raw key always produces the same key_id.
    pub key_id: String,
}

impl GeneratedKey {
    /// Format suitable for the MEMORIA_API_KEYS env var: `<role>:<hash>`,
    /// with comma separation between entries.
    pub fn env_entry(&self) -> String {
        format!("{}:{}", self.role.as_str(), self.hash)
    }
}

/// Generate a new service API key for the given role.
pub fn generate_api_key(role: Role) -> Result<GeneratedKey, String> {
    // 32 chars of base62 randomness — ~190 bits of entropy, well past the
    // 128-bit threshold where brute force stops being a meaningful threat
    // model.
    let charset: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    let suffix: String = (0..32)
        .map(|_| {
            let idx = rng.random_range(0..charset.len());
            charset[idx] as char
        })
        .collect();
    let raw = format!("mk_{}_{}", role.as_str(), suffix);

    // Argon2id hash with a fresh random salt. Default params are appropriate
    // for an interactive-server context — fast enough for per-request verify,
    // slow enough to make offline brute force on a leaked hash list painful.
    let salt_bytes: [u8; 16] = rand::random();
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| format!("salt generation failed: {}", e))?;
    let hash = Argon2::default()
        .hash_password(raw.as_bytes(), &salt)
        .map_err(|e| format!("hash failed: {}", e))?
        .to_string();

    // Key ID: SHA-256 prefix of the raw key. Stable across restarts, one-way,
    // useful for audit ("the key with ID ab12cd34 was used"). 8 hex chars =
    // 32 bits of namespace, plenty for personal-scale.
    let digest = Sha256::digest(raw.as_bytes());
    let key_id = hex::encode(&digest[..4]);

    Ok(GeneratedKey {
        role,
        raw,
        hash,
        key_id,
    })
}

/// Print a newly-generated key to stderr in human-readable form.
///
/// The raw key is shown ONCE here — memoria does not retain it and it
/// cannot be recovered from the hash. The caller is responsible for
/// copying it before this output scrolls off-screen.
pub fn print_generated_key(key: &GeneratedKey) {
    eprintln!();
    eprintln!("═══ NEW SERVICE API KEY GENERATED ═══");
    eprintln!();
    eprintln!("  Role:    {}", key.role.as_str());
    eprintln!("  Key ID:  {}", key.key_id);
    eprintln!();
    eprintln!("  Raw key — paste into the client .env (e.g. as MEMORIA_MCP_TOKEN):");
    eprintln!();
    eprintln!("    {}", key.raw);
    eprintln!();
    eprintln!("  Hash entry — add to memoria's MEMORIA_API_KEYS (comma-separated):");
    eprintln!();
    eprintln!("    {}", key.env_entry());
    eprintln!();
    eprintln!("  ⚠ The raw key will NOT be shown again. Store it now.");
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::{PasswordHash, PasswordVerifier};

    #[test]
    fn role_round_trip() {
        assert_eq!(Role::from_str("rover"), Some(Role::Rover));
        assert_eq!(Role::Rover.as_str(), "rover");
        assert_eq!(Role::from_str("admin"), None);
        assert_eq!(Role::from_str(""), None);
    }

    #[test]
    fn raw_key_format() {
        let key = generate_api_key(Role::Rover).unwrap();
        assert!(key.raw.starts_with("mk_rover_"));
        assert_eq!(key.raw.len(), "mk_rover_".len() + 32);
    }

    #[test]
    fn hash_verifies_against_raw() {
        let key = generate_api_key(Role::Rover).unwrap();
        let parsed = PasswordHash::new(&key.hash).unwrap();
        Argon2::default()
            .verify_password(key.raw.as_bytes(), &parsed)
            .expect("hash should verify against the raw key it was derived from");
    }

    #[test]
    fn hash_rejects_other_keys() {
        let a = generate_api_key(Role::Rover).unwrap();
        let b = generate_api_key(Role::Rover).unwrap();
        let parsed = PasswordHash::new(&a.hash).unwrap();
        assert!(
            Argon2::default()
                .verify_password(b.raw.as_bytes(), &parsed)
                .is_err(),
            "key A's hash must not verify against key B's raw"
        );
    }

    #[test]
    fn key_id_is_stable_per_raw() {
        // Two GeneratedKey instances with the same `raw` should have the
        // same key_id. We verify this by re-hashing manually.
        let key = generate_api_key(Role::Rover).unwrap();
        let digest = Sha256::digest(key.raw.as_bytes());
        let expected = hex::encode(&digest[..4]);
        assert_eq!(key.key_id, expected);
        assert_eq!(key.key_id.len(), 8);
    }

    #[test]
    fn keys_are_unique() {
        // Generate a few and confirm no collisions. 190 bits of entropy
        // means collisions are vanishingly improbable, but a smoke test
        // catches catastrophic RNG misconfig.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..16 {
            let key = generate_api_key(Role::Rover).unwrap();
            assert!(seen.insert(key.raw), "duplicate raw key generated");
        }
    }

    #[test]
    fn env_entry_format() {
        let key = generate_api_key(Role::Rover).unwrap();
        let entry = key.env_entry();
        assert!(entry.starts_with("rover:"));
        assert!(entry.contains(&key.hash));
    }
}
