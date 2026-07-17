use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::config::*;
use crate::ssh;

const API_BASE: &str = "https://api.hetzner.cloud/v1";

/// Hetzner Cloud API client.
struct HetznerClient {
    token: String,
    http: reqwest::blocking::Client,
}

// ── API response types ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ServersResponse {
    servers: Vec<ServerInfo>,
}

#[derive(Debug, Deserialize)]
struct ServerResponse {
    server: ServerInfo,
}

#[derive(Debug, Clone, Deserialize)]
struct ServerInfo {
    id: u64,
    name: String,
    status: String,
    public_net: PublicNet,
}

#[derive(Debug, Clone, Deserialize)]
struct PublicNet {
    ipv4: Option<Ipv4Info>,
}

#[derive(Debug, Clone, Deserialize)]
struct Ipv4Info {
    ip: String,
}

impl ServerInfo {
    /// Extract IPv4 address, if available.
    fn ipv4(&self) -> Option<&str> {
        self.public_net.ipv4.as_ref().map(|v| v.ip.as_str())
    }
}

#[derive(Debug, Deserialize)]
struct SshKeysResponse {
    ssh_keys: Vec<SshKeyInfo>,
}

#[derive(Debug, Deserialize)]
struct SshKeyResponse {
    ssh_key: SshKeyInfo,
}

#[derive(Debug, Deserialize)]
struct SshKeyInfo {
    id: u64,
    name: String,
    public_key: String,
}

// ── Request types ───────────────────────────────────────────────────────────

#[derive(Serialize)]
struct CreateServerRequest {
    name: String,
    server_type: String,
    image: String,
    location: String,
    ssh_keys: Vec<u64>,
    public_net: CreateServerPublicNet,
}

#[derive(Serialize)]
struct CreateServerPublicNet {
    enable_ipv4: bool,
    enable_ipv6: bool,
}

#[derive(Serialize)]
struct CreateSshKeyRequest {
    name: String,
    public_key: String,
}

// ── Implementation ──────────────────────────────────────────────────────────

impl HetznerClient {
    fn new(token: String) -> Self {
        Self {
            token,
            http: reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(60))
                .build()
                .expect("Failed to build HTTP client"),
        }
    }

    fn get(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.http
            .get(format!("{}{}", API_BASE, path))
            .bearer_auth(&self.token)
    }

    fn post(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.http
            .post(format!("{}{}", API_BASE, path))
            .bearer_auth(&self.token)
    }

    fn delete(&self, path: &str) -> reqwest::blocking::RequestBuilder {
        self.http
            .delete(format!("{}{}", API_BASE, path))
            .bearer_auth(&self.token)
    }

    /// Find a server by name.
    fn find_server(&self, name: &str) -> Result<Option<ServerInfo>> {
        let resp: ServersResponse = self
            .get(&format!("/servers?name={}", name))
            .send()
            .context("Failed to list servers")?
            .error_for_status()
            .context("Hetzner API error")?
            .json()
            .context("Failed to parse servers response")?;

        Ok(resp.servers.into_iter().find(|s| s.name == name))
    }

    /// Create a server with location fallback.
    fn create_server(&self, ssh_key_id: u64) -> Result<ServerInfo> {
        for &loc in HETZNER_LOCATIONS {
            info!(
                "Creating {} server '{}' in {} ...",
                HETZNER_SERVER_TYPE, HETZNER_SERVER_NAME, loc
            );

            let body = CreateServerRequest {
                name: HETZNER_SERVER_NAME.to_string(),
                server_type: HETZNER_SERVER_TYPE.to_string(),
                image: HETZNER_IMAGE.to_string(),
                location: loc.to_string(),
                ssh_keys: vec![ssh_key_id],
                public_net: CreateServerPublicNet {
                    enable_ipv4: true,
                    enable_ipv6: false,
                },
            };

            let resp = self
                .post("/servers")
                .json(&body)
                .send()
                .context("Failed to create server")?;

            if resp.status().is_success() {
                let created: ServerResponse =
                    resp.json().context("Failed to parse create response")?;
                info!(
                    "Server created: {} (id={})",
                    created.server.name, created.server.id
                );
                return Ok(created.server);
            }

            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            if text.to_lowercase().contains("unavailable")
                || text.to_lowercase().contains("disabled")
            {
                warn!("Location {} unavailable, trying next...", loc);
                continue;
            }

            bail!("Failed to create server (HTTP {}): {}", status, text);
        }

        bail!(
            "Could not create server in any location: {:?}",
            HETZNER_LOCATIONS
        );
    }

    /// Delete a server by ID.
    fn delete_server(&self, id: u64) -> Result<()> {
        self.delete(&format!("/servers/{}", id))
            .send()
            .context("Failed to delete server")?
            .error_for_status()
            .context("Hetzner API error deleting server")?;
        Ok(())
    }

    /// Poll until server status is "running" (up to 120 s).
    fn wait_for_running(&self, id: u64) -> Result<ServerInfo> {
        let timeout = Duration::from_secs(120);
        let start = Instant::now();
        loop {
            if start.elapsed() > timeout {
                bail!("Server did not reach 'running' within {:?}", timeout);
            }
            let resp: ServerResponse = self
                .get(&format!("/servers/{}", id))
                .send()?
                .error_for_status()?
                .json()?;

            if resp.server.status == "running" {
                return Ok(resp.server);
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    /// List all SSH keys in the account.
    fn list_ssh_keys(&self) -> Result<Vec<SshKeyInfo>> {
        let resp: SshKeysResponse = self.get("/ssh_keys").send()?.error_for_status()?.json()?;
        Ok(resp.ssh_keys)
    }

    /// Create an SSH key.
    fn create_ssh_key(&self, name: &str, pubkey: &str) -> Result<SshKeyInfo> {
        let body = CreateSshKeyRequest {
            name: name.to_string(),
            public_key: pubkey.to_string(),
        };
        let resp = self
            .post("/ssh_keys")
            .json(&body)
            .send()
            .context("Failed to create SSH key")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            bail!("Failed to create SSH key (HTTP {}): {}", status, text);
        }
        let parsed: SshKeyResponse = resp.json().context("Failed to parse SSH key response")?;
        Ok(parsed.ssh_key)
    }

    /// Find or create an SSH key from a public key file.
    fn find_or_create_ssh_key(&self, pubkey_path: &Path) -> Result<u64> {
        let pubkey = std::fs::read_to_string(pubkey_path)
            .with_context(|| format!("Failed to read SSH public key: {:?}", pubkey_path))?
            .trim()
            .to_string();

        // Extract key data (type + base64) for comparison, ignoring comments
        let pubkey_fingerprint = pubkey.split_whitespace().nth(1).unwrap_or(&pubkey);

        let existing = self.list_ssh_keys()?;
        for key in &existing {
            let trimmed = key.public_key.trim();
            let fp = trimmed.split_whitespace().nth(1).unwrap_or(trimmed);
            if fp == pubkey_fingerprint {
                info!("SSH key already registered: {}", key.name);
                return Ok(key.id);
            }
        }

        // Name includes a hash of the key material, so a stale key with the
        // same comment registered from elsewhere can't collide on name.
        // ponytail: 16-bit hash suffix — a 1-in-65k name collision fails the
        // run; widen short_hash if that ever happens.
        let name = format!(
            "{}-{}",
            ssh_key_name(pubkey.split_whitespace().nth(2)),
            short_hash(&pubkey)
        );
        let key = self.create_ssh_key(&name, &pubkey)?;
        info!("Uploaded SSH key: {}", key.name);
        Ok(key.id)
    }
}

/// Derive an SSH key display name from the key comment (e.g. `user@host`),
/// truncated on a char boundary so multi-byte comments (non-ASCII usernames
/// or hostnames) never panic.
fn ssh_key_name(comment: Option<&str>) -> String {
    match comment {
        Some(c) if !c.is_empty() => {
            let truncated: String = c.chars().take(16).collect();
            format!("asw-{}", truncated)
        }
        _ => "asw-key".to_string(),
    }
}

/// Short, deterministic, non-cryptographic hash of the key material, used
/// only to disambiguate a name collision — not a security property.
fn short_hash(input: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:x}", hasher.finish() & 0xffff)
}

// ── High-level operations ───────────────────────────────────────────────────

/// Provision a server. Returns the IPv4 address.
pub fn provision(token: &str, ssh_key_path: &Path) -> Result<String> {
    let client = HetznerClient::new(token.to_string());

    // Check for existing server
    if let Some(server) = client.find_server(HETZNER_SERVER_NAME)? {
        let ip = server
            .ipv4()
            .context("Existing server has no IPv4 address")?
            .to_string();
        info!("Server already exists: {} ({})", server.name, ip);
        return Ok(ip);
    }

    // The public key must sit next to the private key — an arbitrary other
    // key from ~/.ssh may not match the private key SSH will actually use.
    let pubkey_path = ssh_key_path.with_extension("pub");
    anyhow::ensure!(
        pubkey_path.exists(),
        "No public key found at {:?} (expected next to {:?})",
        pubkey_path,
        ssh_key_path
    );

    let ssh_key_id = client.find_or_create_ssh_key(&pubkey_path)?;

    // Create server
    let server = client.create_server(ssh_key_id)?;
    let ip = server
        .ipv4()
        .context("Newly created server has no IPv4 address")?
        .to_string();

    // Wait for running
    client.wait_for_running(server.id)?;
    info!("Server is running, waiting for SSH...");

    // Wait for SSH
    ssh::wait_for_ssh(&ip)?;

    // Bootstrap
    bootstrap(&ip, ssh_key_path)?;

    Ok(ip)
}

/// Teardown: find and delete the server.
pub fn teardown(token: &str) -> Result<()> {
    let client = HetznerClient::new(token.to_string());

    match client.find_server(HETZNER_SERVER_NAME)? {
        Some(server) => {
            let ip = server.ipv4().unwrap_or("?");
            info!("Deleting server '{}' ({}) ...", server.name, ip);
            client.delete_server(server.id)?;
            info!("Server deleted.");
        }
        None => {
            info!(
                "No server named '{}' found — nothing to tear down.",
                HETZNER_SERVER_NAME
            );
        }
    }
    Ok(())
}

/// Get server status. Returns Some((id, ip, status)) if exists.
pub fn status(token: &str) -> Result<Option<(u64, String, String)>> {
    let client = HetznerClient::new(token.to_string());
    match client.find_server(HETZNER_SERVER_NAME)? {
        Some(server) => {
            let ip = server.ipv4().unwrap_or("").to_string();
            Ok(Some((server.id, ip, server.status)))
        }
        None => Ok(None),
    }
}

fn bootstrap(ip: &str, ssh_key_path: &Path) -> Result<()> {
    info!("Bootstrapping server...");
    let cfg = ssh::SshConfig::new(ip.to_string(), ssh_key_path.to_path_buf());

    let script = format!(
        r#"set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq {}
mkdir -p {}
echo "Bootstrap complete — ready for asw build"
"#,
        BOOTSTRAP_PACKAGES, REMOTE_DATA_DIR
    );

    // root's shell on ubuntu-24.04 is bash; run the script directly.
    ssh::run_ssh_stream(&cfg, &script)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Finding 15: char-boundary-safe comment truncation ──────────────────

    #[test]
    fn ssh_key_name_truncates_ascii_comment() {
        assert_eq!(
            ssh_key_name(Some("ivan@buro-macbook")),
            "asw-ivan@buro-macboo"
        );
    }

    #[test]
    fn ssh_key_name_does_not_panic_on_non_ascii_comment() {
        // Same shape that panicked against the original byte-index slice:
        // 15 ASCII bytes followed by a 2-byte UTF-8 character, so a raw
        // `&s[..16]` lands mid-codepoint. `chars().take(16)` must not panic
        // and must not lose or corrupt the multi-byte character.
        let comment = "aaaaaaaaaaaaaaa\u{fc}"; // 15 'a's + 'ü'
        let name = ssh_key_name(Some(comment));
        assert_eq!(name, "asw-aaaaaaaaaaaaaaa\u{fc}");
    }

    #[test]
    fn ssh_key_name_handles_real_world_non_ascii_hostname() {
        // ivan@büro-macbook — the exact example from the finding.
        let name = ssh_key_name(Some("ivan@b\u{fc}ro-macbook"));
        // Must not panic; truncates by char count, not byte count.
        assert_eq!(name.chars().count(), 4 + 16); // "asw-" + 16 chars
    }

    #[test]
    fn ssh_key_name_falls_back_when_no_comment() {
        assert_eq!(ssh_key_name(None), "asw-key");
        assert_eq!(ssh_key_name(Some("")), "asw-key");
    }
}
