// auth.rs — OAuth 2.1 server-side authentication for Memoria
//
// Implements the minimum OAuth flow required by the MCP spec:
// - Protected Resource Metadata discovery (RFC 9728)
// - OAuth Authorization Server Metadata (RFC 8414)
// - Client Credentials token exchange
// - Bearer token validation on MCP requests
//
// Single-user design: one client_id, one hashed secret, one token.
// Credentials are generated on first run and stored in ~/.memoria/auth.json.
// The secret is shown once and never stored in plaintext.

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Stored credentials — client_id and hashed secret.
#[derive(Debug, Serialize, Deserialize)]
pub struct StoredCredentials {
    pub client_id: String,
    pub client_secret_hash: String,
}

/// A live Bearer token with expiry.
#[derive(Debug, Clone)]
struct ActiveToken {
    token: String,
    expires_at: u64, // unix timestamp
}

/// A pending authorization code waiting to be exchanged for a token.
#[derive(Debug, Clone)]
struct PendingCode {
    code: String,
    client_id: String,
    redirect_uri: String,
    expires_at: u64,
}

/// The auth state for the server.
#[derive(Debug, Clone)]
pub struct AuthState {
    credentials: Arc<StoredCredentials>,
    active_tokens: Arc<RwLock<Vec<ActiveToken>>>,
    pending_codes: Arc<RwLock<Vec<PendingCode>>>,
    token_secret: Arc<Vec<u8>>, // HMAC key for token generation
}

impl AuthState {
    /// Load or create credentials. On first run, generates and prints the secret.
    pub fn load_or_create(auth_dir: &Path) -> Result<Self, String> {
        let auth_path = auth_dir.join("auth.json");
        let token_secret_path = auth_dir.join("token.key");

        let credentials = if auth_path.exists() {
            let data = std::fs::read_to_string(&auth_path)
                .map_err(|e| format!("Failed to read auth.json: {}", e))?;
            serde_json::from_str::<StoredCredentials>(&data)
                .map_err(|e| format!("Failed to parse auth.json: {}", e))?
        } else {
            // Generate new credentials
            let client_id = format!("memoria-{}", &uuid::Uuid::new_v4().to_string()[..8]);
            let client_secret = generate_secret();

            // Hash the secret
            // Generate a random salt from random bytes
            let salt_bytes: [u8; 16] = rand::random();
            let salt = SaltString::encode_b64(&salt_bytes)
                .map_err(|e| format!("Salt generation failed: {}", e))?;
            let hash = Argon2::default()
                .hash_password(client_secret.as_bytes(), &salt)
                .map_err(|e| format!("Failed to hash secret: {}", e))?
                .to_string();

            let creds = StoredCredentials {
                client_id: client_id.clone(),
                client_secret_hash: hash,
            };

            // Store credentials
            std::fs::create_dir_all(auth_dir)
                .map_err(|e| format!("Failed to create auth dir: {}", e))?;
            std::fs::write(&auth_path, serde_json::to_string_pretty(&creds).unwrap())
                .map_err(|e| format!("Failed to write auth.json: {}", e))?;

            // Print secret — shown once, never again
            eprintln!("╔══════════════════════════════════════════════════╗");
            eprintln!("║         MEMORIA - NEW CREDENTIALS GENERATED     ║");
            eprintln!("╠══════════════════════════════════════════════════╣");
            eprintln!("║ Client ID:     {:<33} ║", client_id);
            eprintln!("║ Client Secret: {:<33} ║", client_secret);
            eprintln!("╠══════════════════════════════════════════════════╣");
            eprintln!("║ Enter these in the Claude connector UI.         ║");
            eprintln!("║ The secret will NOT be shown again.             ║");
            eprintln!("║ Delete auth.json to regenerate.                 ║");
            eprintln!("╚══════════════════════════════════════════════════╝");

            creds
        };

        // Load or create HMAC key for token signing
        let token_secret = if token_secret_path.exists() {
            std::fs::read(&token_secret_path)
                .map_err(|e| format!("Failed to read token.key: {}", e))?
        } else {
            let key: Vec<u8> = (0..32).map(|_| rand::rng().random::<u8>()).collect();
            std::fs::write(&token_secret_path, &key)
                .map_err(|e| format!("Failed to write token.key: {}", e))?;
            key
        };

        Ok(Self {
            credentials: Arc::new(credentials),
            active_tokens: Arc::new(RwLock::new(Vec::new())),
            pending_codes: Arc::new(RwLock::new(Vec::new())),
            token_secret: Arc::new(token_secret),
        })
    }

    /// Validate client credentials and return a Bearer token.
    pub fn exchange_token(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<(String, u64), String> {
        // Verify client_id
        if client_id != self.credentials.client_id {
            return Err("invalid_client".into());
        }

        // Verify client_secret against stored hash
        let parsed_hash = PasswordHash::new(&self.credentials.client_secret_hash)
            .map_err(|_| "internal hash error")?;
        Argon2::default()
            .verify_password(client_secret.as_bytes(), &parsed_hash)
            .map_err(|_| "invalid_client")?;

        // Generate a Bearer token (HMAC-signed timestamp)
        let expires_in: u64 = 3600; // 1 hour
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let expires_at = now + expires_in;

        let token = self.generate_token(now);

        // Store active token
        let mut tokens = self.active_tokens.write().unwrap();
        // Clean expired tokens while we're here
        tokens.retain(|t| t.expires_at > now);
        tokens.push(ActiveToken {
            token: token.clone(),
            expires_at,
        });

        Ok((token, expires_in))
    }

    /// Validate a Bearer token from an incoming request.
    pub fn validate_token(&self, token: &str) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let tokens = self.active_tokens.read().unwrap();
        tokens
            .iter()
            .any(|t| t.token == token && t.expires_at > now)
    }

    /// Generate an HMAC-signed token.
    fn generate_token(&self, timestamp: u64) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.token_secret).expect("HMAC key length invalid");
        mac.update(&timestamp.to_le_bytes());
        let nonce: u64 = rand::rng().random();
        mac.update(&nonce.to_le_bytes());
        let result = mac.finalize();
        format!("mem_{}", hex::encode(result.into_bytes()))
    }

    /// Create an authorization code for the authorization code flow.
    /// Returns the code to be sent back to the redirect_uri.
    pub fn create_authorization_code(
        &self,
        client_id: &str,
        redirect_uri: &str,
    ) -> Result<String, String> {
        if client_id != self.credentials.client_id {
            return Err("invalid_client".into());
        }

        let code = format!("mem_code_{}", hex::encode(self.generate_random_bytes(16)));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut codes = self.pending_codes.write().unwrap();
        // Clean expired codes
        codes.retain(|c| c.expires_at > now);
        codes.push(PendingCode {
            code: code.clone(),
            client_id: client_id.to_string(),
            redirect_uri: redirect_uri.to_string(),
            expires_at: now + 300, // 5 minute expiry
        });

        Ok(code)
    }

    /// Exchange an authorization code for a Bearer token.
    pub fn exchange_code(
        &self,
        code: &str,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
    ) -> Result<(String, u64), String> {
        // Verify client credentials
        if client_id != self.credentials.client_id {
            return Err("invalid_client".into());
        }
        let parsed_hash = PasswordHash::new(&self.credentials.client_secret_hash)
            .map_err(|_| "internal hash error")?;
        Argon2::default()
            .verify_password(client_secret.as_bytes(), &parsed_hash)
            .map_err(|_| "invalid_client")?;

        // Find and consume the pending code
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut codes = self.pending_codes.write().unwrap();
        let idx = codes
            .iter()
            .position(|c| {
                c.code == code
                    && c.client_id == client_id
                    && c.redirect_uri == redirect_uri
                    && c.expires_at > now
            })
            .ok_or("invalid_grant")?;
        codes.remove(idx);
        drop(codes);

        // Generate token (same as client_credentials flow)
        let expires_in: u64 = 3600;
        let token = self.generate_token(now);

        let mut tokens = self.active_tokens.write().unwrap();
        tokens.retain(|t| t.expires_at > now);
        tokens.push(ActiveToken {
            token: token.clone(),
            expires_at: now + expires_in,
        });

        Ok((token, expires_in))
    }

    /// Generate random bytes.
    fn generate_random_bytes(&self, len: usize) -> Vec<u8> {
        (0..len).map(|_| rand::rng().random::<u8>()).collect()
    }

    /// Get the client_id (for metadata responses).
    pub fn client_id(&self) -> &str {
        &self.credentials.client_id
    }
}

/// Generate a random client secret.
fn generate_secret() -> String {
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
        .chars()
        .collect();
    let mut rng = rand::rng();
    let secret: String = (0..32)
        .map(|_| chars[rng.random_range(0..chars.len())])
        .collect();
    format!("mem_{}", secret)
}

/// Build the OAuth metadata JSON responses.
pub fn resource_metadata_json(base_url: &str) -> serde_json::Value {
    serde_json::json!({
        "resource": base_url,
        "authorization_servers": [base_url],
        "scopes_supported": ["memoria"]
    })
}

/// Generate the HTML authorization page.
pub fn authorize_page_html(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    scope: &str,
    code_challenge: &str,
) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>Memoria — Authorize</title>
    <style>
        body {{ font-family: system-ui; max-width: 400px; margin: 80px auto; padding: 20px;
               background: #1a1a1a; color: #e0e0e0; }}
        h1 {{ font-size: 1.4em; }}
        .info {{ background: #2a2a2a; padding: 16px; border-radius: 8px; margin: 20px 0; }}
        .info p {{ margin: 4px 0; color: #999; font-size: 0.9em; }}
        button {{ background: #4a9eff; color: white; border: none; padding: 12px 24px;
                  border-radius: 6px; font-size: 1em; cursor: pointer; width: 100%; }}
        button:hover {{ background: #3a8eef; }}
    </style>
</head>
<body>
    <h1>Authorize Claude to access Memoria</h1>
    <div class="info">
        <p>Client: {client_id}</p>
        <p>Scope: {scope}</p>
    </div>
    <p>This will allow Claude to read and write to your memory store.</p>
    <form method="POST" action="/authorize">
        <input type="hidden" name="client_id" value="{client_id}">
        <input type="hidden" name="redirect_uri" value="{redirect_uri}">
        <input type="hidden" name="state" value="{state}">
        <input type="hidden" name="scope" value="{scope}">
        <input type="hidden" name="code_challenge" value="{code_challenge}">
        <button type="submit">Allow</button>
    </form>
</body>
</html>"#
    )
}

pub fn auth_server_metadata_json(base_url: &str) -> serde_json::Value {
    serde_json::json!({
        "issuer": base_url,
        "authorization_endpoint": format!("{}/authorize", base_url),
        "token_endpoint": format!("{}/token", base_url),
        "token_endpoint_auth_methods_supported": ["client_secret_post", "client_secret_basic"],
        "grant_types_supported": ["authorization_code", "client_credentials"],
        "scopes_supported": ["memoria"],
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_credential_generation_and_validation() {
        let dir = TempDir::new().unwrap();
        let auth = AuthState::load_or_create(dir.path()).unwrap();

        // Read back the stored credentials to get the client_id
        let auth_json = std::fs::read_to_string(dir.path().join("auth.json")).unwrap();
        let creds: StoredCredentials = serde_json::from_str(&auth_json).unwrap();

        // We can't test the full flow without knowing the secret (it was printed to stderr)
        // But we can verify the structure exists
        assert!(creds.client_id.starts_with("memoria-"));
        assert!(!creds.client_secret_hash.is_empty());
        assert!(dir.path().join("token.key").exists());

        // Loading again should use existing credentials
        let auth2 = AuthState::load_or_create(dir.path()).unwrap();
        assert_eq!(auth.client_id(), auth2.client_id());
    }

    #[test]
    fn test_token_exchange_wrong_client() {
        let dir = TempDir::new().unwrap();
        let auth = AuthState::load_or_create(dir.path()).unwrap();

        let result = auth.exchange_token("wrong-client", "wrong-secret");
        assert!(result.is_err());
    }

    #[test]
    fn test_token_validation() {
        let dir = TempDir::new().unwrap();

        // Create auth and capture the secret by reading the hash approach
        // We'll test with a known secret by constructing manually
        let secret = "test_secret_12345";
        // Generate a random salt from random bytes
        let salt_bytes: [u8; 16] = rand::random();
        let salt = SaltString::encode_b64(&salt_bytes).unwrap();
        let hash = Argon2::default()
            .hash_password(secret.as_bytes(), &salt)
            .unwrap()
            .to_string();

        let creds = StoredCredentials {
            client_id: "test-client".into(),
            client_secret_hash: hash,
        };
        std::fs::write(
            dir.path().join("auth.json"),
            serde_json::to_string_pretty(&creds).unwrap(),
        )
        .unwrap();

        let auth = AuthState::load_or_create(dir.path()).unwrap();

        // Exchange with correct credentials
        let (token, expires_in) = auth.exchange_token("test-client", secret).unwrap();
        assert!(token.starts_with("mem_"));
        assert_eq!(expires_in, 3600);

        // Token should be valid
        assert!(auth.validate_token(&token));

        // Random token should not be valid
        assert!(!auth.validate_token("mem_bogus"));
    }
}
