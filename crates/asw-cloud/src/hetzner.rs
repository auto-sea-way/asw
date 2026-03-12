use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::config::*;
use crate::ssh;

const API_BASE: &str = "https://api.hetzner.cloud/v1";

/// Hetzner Cloud API client.
pub struct HetznerClient {
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

#[derive(Debug, Deserialize)]
struct ServerCreateResponse {
    server: ServerInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerInfo {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub public_net: PublicNet,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PublicNet {
    pub ipv4: Option<Ipv4Info>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ipv4Info {
    pub ip: String,
}

impl ServerInfo {
    /// Extract IPv4 address, if available.
    pub fn ipv4(&self) -> Option<&str> {
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
    pub fn new(token: String) -> Self {
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
    pub fn find_server(&self, name: &str) -> Result<Option<ServerInfo>> {
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
                let created: ServerCreateResponse =
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
    pub fn delete_server(&self, id: u64) -> Result<()> {
        self.delete(&format!("/servers/{}", id))
            .send()
            .context("Failed to delete server")?
            .error_for_status()
            .context("Hetzner API error deleting server")?;
        Ok(())
    }

    /// Poll until server status is "running".
    fn wait_for_running(&self, id: u64, timeout: Duration) -> Result<ServerInfo> {
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
        let resp: SshKeyResponse = self
            .post("/ssh_keys")
            .json(&body)
            .send()?
            .error_for_status()
            .context("Failed to create SSH key")?
            .json()?;
        Ok(resp.ssh_key)
    }

    /// Find or create an SSH key from a public key file.
    pub fn find_or_create_ssh_key(&self, pubkey_path: &Path) -> Result<u64> {
        let pubkey = std::fs::read_to_string(pubkey_path)
            .with_context(|| format!("Failed to read SSH public key: {:?}", pubkey_path))?
            .trim()
            .to_string();

        // Extract key data (type + base64) for comparison, ignoring comments
        let pubkey_parts: Vec<&str> = pubkey.split_whitespace().collect();
        let pubkey_str = pubkey.as_str();
        let pubkey_fingerprint = pubkey_parts.get(1).unwrap_or(&pubkey_str);

        let existing = self.list_ssh_keys()?;
        for key in &existing {
            let trimmed = key.public_key.trim().to_string();
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            let binding = trimmed.as_str();
            let fp = parts.get(1).unwrap_or(&binding);
            if fp == pubkey_fingerprint {
                info!("SSH key already registered: {}", key.name);
                return Ok(key.id);
            }
        }

        let name = if pubkey_parts.len() > 2 {
            format!("asw-{}", &pubkey_parts[2][..pubkey_parts[2].len().min(16)])
        } else {
            "asw-key".to_string()
        };

        match self.create_ssh_key(&name, &pubkey) {
            Ok(key) => {
                info!("Uploaded SSH key: {}", key.name);
                Ok(key.id)
            }
            Err(e) => {
                // Key may exist with different name — use first available
                if format!("{:?}", e).to_lowercase().contains("uniqueness") {
                    let keys = self.list_ssh_keys()?;
                    if let Some(key) = keys.first() {
                        info!("Using existing SSH key: {}", key.name);
                        return Ok(key.id);
                    }
                }
                Err(e)
            }
        }
    }
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

    // Find or create SSH key
    let pubkey_path = ssh_key_path.with_extension("pub");
    let pubkey_file = if pubkey_path.exists() {
        pubkey_path
    } else {
        // Try common public key locations
        let ssh_dir = ssh_key_path.parent().unwrap_or(Path::new("~/.ssh"));
        let mut found = None;
        for name in &["id_ed25519.pub", "id_rsa.pub", "id_ecdsa.pub"] {
            let p = ssh_dir.join(name);
            if p.exists() {
                found = Some(p);
                break;
            }
        }
        found.context("No SSH public key found")?
    };

    let ssh_key_id = client.find_or_create_ssh_key(&pubkey_file)?;

    // Create server
    let server = client.create_server(ssh_key_id)?;
    let ip = server
        .ipv4()
        .context("Newly created server has no IPv4 address")?
        .to_string();

    // Wait for running
    client.wait_for_running(server.id, Duration::from_secs(120))?;
    info!("Server is running, waiting for SSH...");

    // Wait for SSH
    ssh::wait_for_ssh(&ip, Duration::from_secs(120))?;

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

    // Write script to server and execute
    ssh::run_ssh(
        &cfg,
        &format!(
            "cat > /tmp/asw-bootstrap.sh << 'BOOTSTRAP_EOF'\n{}\nBOOTSTRAP_EOF",
            script
        ),
    )?;
    ssh::run_ssh_stream(&cfg, "bash /tmp/asw-bootstrap.sh")?;

    Ok(())
}
