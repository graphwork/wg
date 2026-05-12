# Server Setup Guide

This guide covers server-side configuration for hosting a multi-user wg instance. It focuses on mosh, the primary transport for mobile and roaming clients, plus the surrounding infrastructure (firewall, systemd, connection dispatcher).

---

## Prerequisites

- A Linux VPS (Ubuntu 22.04/24.04 or Debian 12 recommended)
- Root or sudo access
- SSH server running and accessible
- wg binary installed (`cargo install wg` or from [GitHub Releases](https://github.com/graphwork/wg/releases))
- tmux installed (`apt install tmux`)

---

## mosh Server Configuration

### What is mosh?

[mosh](https://mosh.org/) (Mobile Shell) is a remote terminal application that replaces SSH for interactive use. It uses UDP instead of TCP, which makes it resilient to:

- **Network changes** — roaming between Wi-Fi and cellular doesn't drop the session
- **High latency** — local echo provides instant feedback while waiting for the server
- **Intermittent connectivity** — sessions survive laptop sleep, network outages, and IP changes

For wg, mosh is the recommended transport for mobile clients (Android via Termux, iOS via Blink Shell) and for desktop users on unreliable networks.

### Installation

**Ubuntu / Debian:**

```bash
sudo apt update
sudo apt install mosh
```

**Verify installation:**

```bash
mosh-server --version
# Expected: mosh 1.4.0 or later
which mosh-server
# Expected: /usr/bin/mosh-server
```

**RHEL / Fedora (if applicable):**

```bash
sudo dnf install mosh
```

**Arch Linux:**

```bash
sudo pacman -S mosh
```

### Firewall Configuration

mosh uses UDP ports 60000-61000 for its encrypted data channel. Each concurrent mosh session uses one port from this range, so the range supports up to 1001 simultaneous connections.

**ufw (Ubuntu default):**

```bash
sudo ufw allow 60000:61000/udp comment "mosh"
sudo ufw reload
sudo ufw status | grep mosh
```

**iptables (manual):**

```bash
sudo iptables -A INPUT -p udp --dport 60000:61000 -j ACCEPT
# Persist across reboots:
sudo apt install iptables-persistent
sudo netfilter-persistent save
```

**nftables:**

```bash
sudo nft add rule inet filter input udp dport 60000-61000 accept
```

**Cloud provider firewalls:** If your VPS is on AWS, GCP, DigitalOcean, or similar, you must also open UDP 60000-61000 in the cloud provider's security group / firewall rules. The OS-level firewall alone is not sufficient.

> **Port range tuning:** If you expect fewer than ~10 concurrent mosh users, you can restrict the range (e.g., 60000:60019) to reduce your attack surface. Set the range consistently in both the firewall and `MOSH_SERVER_NETWORK_TMOUT` / mosh-server invocation.

### Systemd Configuration

mosh-server is normally started on-demand by the mosh client via SSH — it does not require a persistent daemon. However, you can tune its behavior through environment variables and systemd overrides.

**Default behavior (no systemd unit needed):**

When a client runs `mosh user@server`, the mosh client:
1. SSHs to the server
2. Launches `mosh-server new` on the server, which picks a free UDP port
3. Returns the port and session key to the client
4. Client connects directly via UDP

This means mosh-server processes are ephemeral — they start per-session and exit when the session ends.

**Environment configuration via systemd user service (optional):**

If you want to set default environment variables for all mosh sessions launched by a user, create a user-level environment override:

```bash
# /etc/environment.d/mosh.conf
MOSH_PREDICTION_DISPLAY=adaptive
MOSH_SERVER_NETWORK_TMOUT=604800
MOSH_SERVER_SIGNAL_TMOUT=60
```

Or set them in the user's shell profile (`~/.bashrc`, `~/.zshrc`):

```bash
# mosh server configuration
export MOSH_PREDICTION_DISPLAY=adaptive
export MOSH_SERVER_NETWORK_TMOUT=604800  # 7 days before idle disconnect
export MOSH_SERVER_SIGNAL_TMOUT=60       # 60s grace period after SIGHUP
```

**Systemd resource limits for mosh sessions (optional):**

If you want to limit resources consumed by mosh sessions, create a systemd slice:

```ini
# /etc/systemd/system/user-mosh.slice
[Slice]
Description=Resource limits for mosh sessions
MemoryMax=512M
TasksMax=64
```

Apply it by wrapping mosh-server invocations (advanced — typically not needed for small deployments).

### Performance Tuning

#### MOSH_PREDICTION_DISPLAY

This is the most impactful mosh setting. It controls local echo (speculative rendering of keystrokes before the server confirms them).

| Value | Behavior | Best for |
|-------|----------|----------|
| `adaptive` | Shows predictions when latency is noticeable (~70ms+) | **Recommended default** |
| `always` | Always show predictions | Very high-latency links (satellite, intercontinental) |
| `never` | No predictions | Low-latency LAN connections |
| `experimental` | Aggressive predictions including cursor movement | Not recommended for production |

**Set on the server** (applies to all sessions):

```bash
echo 'export MOSH_PREDICTION_DISPLAY=adaptive' | sudo tee -a /etc/profile.d/mosh.sh
sudo chmod +x /etc/profile.d/mosh.sh
```

**Set on the client** (per-user override):

```bash
export MOSH_PREDICTION_DISPLAY=always
mosh user@server
```

The client-side setting takes precedence.

#### Session Timeout Tuning

| Variable | Default | Description |
|----------|---------|-------------|
| `MOSH_SERVER_NETWORK_TMOUT` | 0 (never) | Seconds of network silence before server kills the session. Set to `604800` (7 days) for shared servers to clean up abandoned sessions. |
| `MOSH_SERVER_SIGNAL_TMOUT` | 60 | Seconds to wait after SIGHUP before terminating. |

**Recommended server-wide settings:**

```bash
cat <<'EOF' | sudo tee /etc/profile.d/mosh.sh
# mosh server defaults for wg deployment
export MOSH_PREDICTION_DISPLAY=adaptive
export MOSH_SERVER_NETWORK_TMOUT=604800  # Clean up after 7 days idle
export MOSH_SERVER_SIGNAL_TMOUT=60
EOF
sudo chmod +x /etc/profile.d/mosh.sh
```

#### Locale Configuration

mosh requires UTF-8 locales on both client and server. If you see `mosh-server needs a UTF-8 native locale to run` errors:

```bash
sudo apt install locales
sudo locale-gen en_US.UTF-8
sudo update-locale LANG=en_US.UTF-8
```

Verify:

```bash
locale | grep UTF-8
# LANG=en_US.UTF-8
```

---

## Security Model

### mosh Encryption (AES-128-OCB)

mosh provides strong, authenticated encryption for the UDP data channel:

- **Algorithm:** AES-128-OCB (Offset Codebook Mode)
- **Key exchange:** The 128-bit AES session key is generated on the server and delivered to the client via SSH. The initial SSH connection handles authentication and key transport.
- **Authentication model:** mosh inherits SSH's authentication. It does not implement its own user authentication — SSH handles that (passwords, keys, certificates). After the SSH handshake, mosh takes over with its own encrypted UDP channel.
- **Forward secrecy:** Each session generates a fresh AES key. Compromising one session key does not reveal past or future sessions.
- **Integrity:** OCB mode provides authenticated encryption — it detects tampering and replay attacks. Each datagram has a unique sequence number.
- **No TCP fallback:** mosh never falls back to an unencrypted channel. If UDP is blocked, the connection fails rather than degrading.

### Security properties comparison

| Property | SSH | mosh |
|----------|-----|------|
| Authentication | SSH keys / passwords / certificates | Delegates to SSH |
| Transport encryption | AES-256 (typically) over TCP | AES-128-OCB over UDP |
| Replay protection | TCP sequence numbers | Per-datagram sequence numbers |
| Forward secrecy | Via key exchange (DH/ECDH) | Fresh key per session (via SSH) |
| Port exposure | TCP 22 | TCP 22 + UDP 60000-61000 |
| Roaming support | No (TCP breaks on IP change) | Yes (UDP tolerates IP change) |

### Security considerations for shared wg servers

1. **SSH key-only authentication:** Disable password authentication for the server.

   ```bash
   # /etc/ssh/sshd_config
   PasswordAuthentication no
   PubkeyAuthentication yes
   ```

2. **Restrict mosh to authorized users:** mosh inherits SSH access controls. Use `AllowUsers` or `AllowGroups` in `sshd_config`:

   ```bash
   # /etc/ssh/sshd_config
   AllowGroups wg-users
   ```

3. **Session cleanup:** Set `MOSH_SERVER_NETWORK_TMOUT` to avoid stale sessions consuming resources. List active mosh sessions:

   ```bash
   pgrep -a mosh-server
   # Or, to see which ports are in use:
   ss -unap | grep mosh
   ```

4. **Audit trail:** mosh sessions are visible in process listings but do not appear in `who` or `utmp` by default. For audit purposes, rely on SSH auth logs:

   ```bash
   journalctl -u ssh --since "1 hour ago" | grep Accepted
   ```

---

## Integration with wg-connect.sh

The `wg-connect.sh` dispatcher script (see task `mu-c-connect-dispatcher`) provides a consistent entry point for all transport types. mosh clients connect through it as follows:

### Desktop connection via mosh

```bash
mosh user@server -- /path/to/wg-connect.sh
```

Or, if `wg-connect.sh` is in the user's `$PATH`:

```bash
mosh user@server -- wg-connect.sh
```

This launches `wg-connect.sh` on the server, which:
1. Determines `WG_USER` from the SSH user or `$USER`
2. Runs `tmux new-session -A -s "${WG_USER:-$USER}-wg" "wg tui"`
3. Attaches to an existing session or creates a new one

### SSH ForceCommand integration

For a fully locked-down wg server, configure SSH to automatically route users into the dispatcher:

```bash
# /etc/ssh/sshd_config.d/wg.conf
Match Group wg-users
    ForceCommand /usr/local/bin/wg-connect.sh
```

This works with both SSH and mosh connections — mosh's initial SSH handshake respects `ForceCommand`.

### Mobile client connection strings

**Android (Termux):**

```bash
mosh user@server -- wg-connect.sh
```

**iOS (Blink Shell):** Configure a host in Blink with:
- Host: `server`
- User: `user`
- Mosh: enabled
- Remote command: `wg-connect.sh`

Or as a manual command:

```bash
mosh user@server -- tmux new-session -A -s "$USER-wg" "wg tui"
```

---

## Monitoring and Troubleshooting

### Check active mosh sessions

```bash
# List all mosh-server processes with their ports
pgrep -a mosh-server

# Check UDP port usage in the mosh range
ss -unap | grep -E '6[0-9]{4}'
```

### Common issues

**"mosh-server not found":**
The mosh client SSHs to the server and runs `mosh-server`. Ensure mosh is installed and in the `$PATH` for non-interactive shells. If installed in a non-standard location:

```bash
mosh --server=/usr/local/bin/mosh-server user@server
```

**"mosh-server needs a UTF-8 native locale":**
See [Locale Configuration](#locale-configuration) above.

**UDP ports blocked:**
If the connection hangs after "mosh: Nothing received from server on UDP port 600XX", the firewall is blocking UDP. Verify:

```bash
# On the server, check the port is open:
ss -unl | grep 60001
# From the client, test UDP connectivity:
nc -u -z server 60001
```

**Stale sessions:**
If `pgrep -a mosh-server` shows sessions with no active clients, they can be cleaned up:

```bash
# Kill a specific session by PID:
kill <pid>

# Or kill all stale mosh-server processes for a user:
pkill -u username mosh-server
```

Setting `MOSH_SERVER_NETWORK_TMOUT=604800` prevents indefinite accumulation.

**High latency / prediction artifacts:**
If users see "phantom text" from speculative rendering, switch to conservative prediction:

```bash
export MOSH_PREDICTION_DISPLAY=adaptive  # or 'never' for LAN
```

---

## Quick Reference

```bash
# Install
sudo apt install mosh

# Firewall
sudo ufw allow 60000:61000/udp comment "mosh"

# Server-wide performance defaults
cat <<'EOF' | sudo tee /etc/profile.d/mosh.sh
export MOSH_PREDICTION_DISPLAY=adaptive
export MOSH_SERVER_NETWORK_TMOUT=604800
export MOSH_SERVER_SIGNAL_TMOUT=60
EOF
sudo chmod +x /etc/profile.d/mosh.sh

# Connect (from client)
mosh user@server -- wg-connect.sh

# Monitor
pgrep -a mosh-server
ss -unap | grep mosh
```
