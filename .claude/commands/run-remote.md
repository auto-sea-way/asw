---
allowed-tools: Bash, Read, Glob, Grep
---

# /run-remote — Run commands on the remote Hetzner build server

Run the user's command on the remote Hetzner server: $ARGUMENTS

## Instructions

You manage the Hetzner server lifecycle and execute commands remotely. Follow these steps:

### 1. Detect user intent

- If the user says "done", "cleanup", "teardown", "destroy", or similar: **delete the server** and confirm.
- If the user says "status" or "check": just report whether the server exists and its IP.
- Otherwise: run the provided command on the server.

### 2. Check if server exists

The CLI reads `HETZNER_TOKEN` from `.env` automatically (via dotenvy). No need to extract it manually for CLI commands. For raw curl calls, extract it:

```bash
HETZNER_TOKEN=$(grep HETZNER_TOKEN .env | cut -d= -f2)
curl -s -H "Authorization: Bearer $HETZNER_TOKEN" \
  "https://api.hetzner.cloud/v1/servers?name=asw-builder"
```

Or use the CLI:
```bash
cargo run -p asw-cli -- cloud status
```

Parse the JSON response. If `servers` array is non-empty, extract the `public_net.ipv4.ip`.

### 3. Provision if needed

If no server exists and the user wants to run a command:

```bash
cargo run -p asw-cli -- cloud provision
```

No need to pass `--hetzner-token` — it's read from `.env` automatically.
Wait for provisioning + bootstrap to complete. This takes ~3-5 minutes.

### 4. Run the command

```bash
ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i ~/.ssh/id_rsa root@$SERVER_IP "$COMMAND"
```

### 5. Server teardown

When user requests teardown:
```bash
cargo run -p asw-cli -- cloud teardown
```

### Key details

- The CLI loads `.env` automatically — `HETZNER_TOKEN` is picked up without `--hetzner-token`
- Always use `-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null` for SSH
- Remote data directory: `/data/asw`
- Remote binary: `/usr/local/bin/asw`
- Server type: cpx62 (32 GB RAM, 16 vCPU), Ubuntu 24.04, location: nbg1
- Keep the server alive between commands unless explicitly asked to tear down
- Show the command output to the user clearly
