# Terminal Wrapping Strategy: Mobile & Web Access

**Task:** mu-design-terminal-wrapping
**Date:** 2026-03-25
**Depends on:** [Multi-User TUI Feasibility Research](../research/multi-user-tui-feasibility.md)

---

## Core Principle

**The TUI IS the interface.** We don't build separate UIs for web, Android, or iOS. We wrap the terminal. Every platform connects to the same `wg tui` process running on a server, through a connection layer that handles resilience and authentication.

This means:
- One codebase to maintain (the existing Rust TUI)
- Feature parity across all platforms by default
- Investment in the TUI benefits all platforms simultaneously

---

## 1. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                          SERVER (VPS / homelab)                     │
│                                                                     │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │ .wg/graph.jsonl  (shared state, flock-serialized)     │   │
│  └──────────────────────────┬───────────────────────────────────┘   │
│                             │ fs watcher (50ms)                     │
│  ┌──────────────────────────┴───────────────────────────────────┐   │
│  │  tmux sessions (one per user, named $USER-wg)                │   │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐          │   │
│  │  │  wg tui (A)  │  │  wg tui (B)  │  │  wg tui (C)  │          │   │
│  │  └─────────────┘  └─────────────┘  └─────────────┘          │   │
│  └──────────┬──────────────┬──────────────┬─────────────────────┘   │
│             │              │              │                          │
│  ┌──────────┴──────────────┴──────────────┴─────────────────────┐   │
│  │                   Connection Layer                            │   │
│  │  ┌───────────┐  ┌────────────┐  ┌────────────────────────┐   │   │
│  │  │  sshd     │  │  mosh-     │  │  ttyd (port 8080)      │   │   │
│  │  │  :22      │  │  server    │  │  → reverse proxy       │   │   │
│  │  │           │  │  :60000+   │  │  → Caddy/nginx (:443)  │   │   │
│  │  └─────┬─────┘  └─────┬──────┘  └──────────┬─────────────┘   │   │
│  └────────┼───────────────┼────────────────────┼─────────────────┘   │
└───────────┼───────────────┼────────────────────┼─────────────────────┘
            │               │                    │
    ┌───────┴────┐  ┌───────┴──────┐  ┌──────────┴──────────┐
    │  Desktop   │  │   Mobile     │  │     Browser          │
    │  SSH/mosh  │  │   Termux/    │  │     xterm.js         │
    │  client    │  │   Blink      │  │     (via ttyd)       │
    │            │  │   + mosh     │  │                      │
    └────────────┘  └──────────────┘  └─────────────────────┘
```

### Session Lifecycle

1. User authenticates (SSH key, mosh, or web login)
2. Connection layer attaches to user's named tmux session: `tmux new-session -A -s $USER-wg`
3. If tmux session is new, it starts `wg tui` inside it
4. If tmux session already exists (reconnection), user picks up exactly where they left off
5. All TUI instances share the same `.wg/graph.jsonl` — changes propagate via fs watcher

---

## 2. Platform Strategies

### 2.1 Web — ttyd + Reverse Proxy

**Recommended approach:** ttyd behind an authenticated reverse proxy.

**Why ttyd over alternatives:**

| Criterion | ttyd | wetty | gotty |
|-----------|------|-------|-------|
| Language | C (libwebsockets) | Node.js (socket.io) | Go |
| Latency | ~10-30ms | ~30-50ms | ~10-30ms |
| Active development | Yes (5k+ stars) | Slow | Stale since 2019 |
| Dependencies | Single binary | Node.js runtime | Single binary |
| xterm.js version | Current | Current | Uses hterm (not xterm.js) |
| Read-only mode | Yes (`-R`) | No | Yes (`--permit-write`) |
| TLS built-in | Yes | Via proxy | Yes |

ttyd wins on maturity, performance, zero-dependency deployment, and active maintenance.

**Architecture:**

```
Browser → Caddy (:443, TLS + auth) → ttyd (:8080, localhost only)
                                        └→ PTY → tmux attach → wg tui
```

**Authentication strategy (layered):**

1. **Minimum viable:** Caddy with `basicauth` directive + TLS via automatic Let's Encrypt
2. **Better:** Caddy with `forward_auth` to a small OAuth2 proxy (e.g., OAuth2 Proxy → GitHub/Google)
3. **Best:** Caddy with Authelia or Authentik for SSO + MFA

ttyd's built-in `-c user:pass` basic auth is a fallback but should not be the primary mechanism in production — it lacks rate limiting and session management.

**Session management:**

- **Persistent sessions via tmux.** ttyd launches `tmux new-session -A -s $USER-wg "wg tui"` per authenticated user. Browser close → tmux session persists → browser reopen → reattaches.
- ttyd's `--client-option reconnect=10` auto-reconnects the WebSocket after brief network disruptions.
- For multi-tab: ttyd spawns one PTY per WebSocket connection. If the tmux command uses `-A` (attach-or-create), all tabs share one TUI session. Without `-A`, each tab gets its own.

**PWA capability:**

Yes. ttyd's web frontend can be wrapped in a PWA manifest:
- Add `manifest.json` with `"display": "standalone"` and app icons
- Register a service worker for offline shell (shows "reconnecting..." when offline)
- Users can "Add to Home Screen" on mobile browsers → launches in standalone mode without browser chrome
- This is cosmetic — the terminal session is still server-side

**Deployment (single command):**

```bash
# Install
apt install ttyd caddy

# Minimal: no auth, LAN only
ttyd -p 8080 tmux new-session -A -s web-wg "wg tui"

# Production: Caddy reverse proxy with TLS + basic auth
cat > /etc/caddy/Caddyfile <<'EOF'
wg.example.com {
    basicauth * {
        erik $2a$14$... # caddy hash-password
    }
    reverse_proxy localhost:8080
}
EOF
ttyd -p 8080 -i lo tmux new-session -A -s '{username}-wg' 'wg tui'
systemctl start caddy
```

**Feasibility: 5/5** — Works today with zero code changes.

---

### 2.2 Android — Termux + Mosh

**Recommended approach:** Termux with mosh, connecting to the server's tmux session.

**Why Termux (not a custom app):**

- Termux is a full Linux environment on Android — `pkg install mosh tmux` and it works
- The TUI already detects Termux (`TERMUX_VERSION` env var) and adjusts mouse modes (`event.rs:48-50`)
- Building a custom Android app wrapping a terminal emulator (e.g., `jackpal/androidterm` or `niclas-niclas/terminal-emulator`) would duplicate work that Termux does better, and we'd have to maintain it
- Termux's keyboard extension (`Termux:Styling`, `Termux:API`) provides extra keys that map well to TUI shortcuts

**Connection stack:**

```
Termux → mosh → server → tmux attach → wg tui
```

Mosh is critical for mobile because:
- Survives Wi-Fi ↔ cellular transitions without dropping the session
- Handles high-latency connections with local echo / speculative rendering
- Resumes instantly after phone sleep (mosh client reconnects to the persistent mosh-server)

**Preconfigured Termux profile:**

We can ship a `wg-termux-setup.sh` script that users run once:

```bash
#!/data/data/com.termux/files/usr/bin/bash
# wg-termux-setup.sh — one-time Termux configuration for wg

pkg update -y
pkg install -y mosh tmux openssh

# Create a connection shortcut
mkdir -p ~/.shortcuts
cat > ~/.shortcuts/wg <<'SHORTCUT'
#!/bin/bash
mosh user@your-server.com -- tmux new-session -A -s $USER-wg "wg tui"
SHORTCUT
chmod +x ~/.shortcuts/wg

# Termux:Widget can launch ~/.shortcuts/* from the home screen
echo "Done! Install Termux:Widget for home screen shortcut."
echo "Edit ~/.shortcuts/wg to set your server address."
```

**Custom Android app (deferred):**

A custom app wrapping `termux-terminal-emulator` or `niclas-niclas/terminal-emulator` would provide:
- Branded app icon and splash screen
- Built-in server configuration UI (no script editing)
- Push notifications for task completions (via FCM + server-side webhook)
- Embedded mosh client

This is significant development effort (native Android/Kotlin) and only justified if Termux's UX proves insufficient. Recommendation: defer until there's user demand.

**Screen size considerations:**

The TUI runs at whatever terminal size Termux provides. Typical phone: ~40-50 columns, ~20-30 rows. The TUI's multi-panel layout will need responsive handling:

| Screen width | Recommended layout |
|-------------|-------------------|
| < 50 cols | Single panel mode — graph OR detail, not both |
| 50-80 cols | Narrow split — graph list (left), compact detail (right) |
| > 80 cols | Full layout (current default) |

This is a TUI code change (tracked separately), not an Android-specific issue.

**Feasibility: 4/5** — Works today for users comfortable with Termux. The setup script lowers the barrier. Loses a point for the small-screen UX gap.

---

### 2.3 iOS — Blink Shell + Mosh

**Recommended approach:** Point users to Blink Shell ($15.99) for mosh+tmux access.

**Why Blink Shell (not a custom app):**

| App | Mosh | tmux (remote) | True color | Hardware kbd | Price |
|-----|------|---------------|------------|--------------|-------|
| **Blink Shell** | Native impl | Yes | Yes | Full | $15.99 |
| a-Shell | No | No | Limited | Partial | Free |
| iSH | Possible (Alpine) | Yes | Limited | Yes | Free |

Blink Shell is the only iOS app with a production-quality native mosh implementation. a-Shell lacks mosh entirely. iSH can run mosh via Alpine Linux but the x86 emulation layer adds latency.

**App Store constraints:**

Shipping a custom iOS terminal app is feasible (Blink Shell proves this) but requires:
- Apple Developer Program membership ($99/year)
- App Store review (terminal apps are allowed — many exist)
- Maintaining a mosh client implementation (Blink's is native Swift, not trivial)
- Ongoing maintenance for iOS version compatibility

The cost-benefit strongly favors recommending Blink Shell unless wg reaches a scale where a branded app is justified.

**Connection stack (identical to Android):**

```
Blink Shell → mosh → server → tmux attach → wg tui
```

**iOS background limitations:**

iOS suspends apps after ~30 seconds in background. Mosh handles this gracefully:
- mosh-server persists on the server
- When user returns to Blink, mosh-client reconnects and resyncs screen state
- User sees a brief "reconnecting..." then full TUI state restored

This is a fundamental iOS constraint — no workaround exists. But mosh makes it transparent.

**Custom iOS app (deferred):**

Same analysis as Android: a custom app wrapping a terminal view (e.g., SwiftTerm) could provide branding, push notifications, and streamlined onboarding. But maintaining a mosh implementation in Swift is substantial. Defer until user demand warrants it.

**Feasibility: 3/5** — Works well but depends on a paid third-party app. iOS's background restrictions cause minor UX friction. Loses points for the cost barrier and lack of free alternatives with mosh support.

---

### 2.4 Web (Mobile Browsers) — PWA Fallback

For users unwilling to install Termux or Blink Shell, the web/ttyd approach works on mobile browsers:

```
Mobile Safari/Chrome → wg.example.com → Caddy → ttyd → tmux → wg tui
```

This is a viable fallback with caveats:
- On-screen keyboard handling varies by browser (Safari is worst)
- No mosh — pure WebSocket, so network switches drop the connection (ttyd reconnects, but with a visible gap)
- Add to Home Screen → PWA → feels like a native app

**This is the universal fallback** — any device with a browser can access the TUI.

**Feasibility: 4/5** — Works everywhere but lacks mosh resilience and native keyboard integration.

---

## 3. Cross-Cutting Concerns

### 3.1 Connection Resilience

| Platform | Transport | Reconnection | Network switch survival |
|----------|-----------|-------------|------------------------|
| Desktop SSH | TCP | Manual reconnect | No (TCP drops) |
| Desktop mosh | UDP | Automatic, instant | Yes |
| Android (Termux) | mosh (UDP) | Automatic, instant | Yes |
| iOS (Blink) | mosh (UDP) | Automatic, ~2s delay | Yes |
| Web (ttyd) | WebSocket (TCP) | Auto-reconnect (configurable) | Partial (reconnects, but loses state if ttyd restarts) |

**Recommendation:** mosh everywhere except web. For web, tmux is the resilience layer — even if the WebSocket drops, `tmux attach` restores the session.

### 3.2 tmux as Universal Session Layer

tmux is the linchpin of the architecture. It provides:

1. **Session persistence** — TUI survives disconnections
2. **Named sessions** — Each user gets `$USER-wg`, preventing collisions
3. **Attach-or-create** — `tmux new-session -A -s name` is idempotent
4. **Window management** — Users can create additional tmux windows for `wg` CLI commands alongside the TUI

**Server-side tmux configuration** (`.tmux.conf` recommendations):

```tmux
# Keep sessions alive after last client detaches
set -g destroy-unattached off

# Set reasonable history limit
set -g history-limit 10000

# Enable true color
set -g default-terminal "tmux-256color"
set -ga terminal-overrides ",*256col*:Tc"

# Mouse support (for TUI interaction through tmux)
set -g mouse on
```

### 3.3 Authentication Model

| Layer | Web | SSH/Mosh |
|-------|-----|----------|
| Transport auth | TLS (Caddy auto-cert) | SSH keys |
| User auth | OAuth2/basic auth (reverse proxy) | SSH key identity |
| Session binding | HTTP cookie → user → tmux session name | Unix user → tmux session name |

For SSH/mosh: standard SSH key authentication. Each Unix user on the server maps to a tmux session.

For web: the reverse proxy authenticates the user and passes the identity to ttyd. ttyd spawns a tmux session named after the authenticated user.

### 3.4 Server Requirements

| Component | Purpose | Install |
|-----------|---------|---------|
| tmux | Session management | `apt install tmux` |
| mosh | Mobile resilience | `apt install mosh` |
| ttyd | Web terminal access | `apt install ttyd` or binary from GitHub |
| Caddy | Reverse proxy + TLS | `apt install caddy` |
| wg | The application | `cargo install --path .` or release binary |

Minimum VPS: 1 vCPU, 1GB RAM, any Linux. Each connected user adds ~20-50MB (tmux + wg tui process).

---

## 4. First-Run User Journeys

### 4.1 Web First-Run

```
1. User receives URL: https://wg.example.com
2. Browser navigates → Caddy prompts for login (OAuth or basic auth)
3. User authenticates → Caddy proxies to ttyd
4. ttyd spawns: tmux new-session -A -s $USER-wg "wg tui"
5. xterm.js renders the TUI in the browser tab
6. User sees the wg dashboard — can navigate, create tasks, view agents
7. (Optional) User clicks "Add to Home Screen" for PWA experience
```

**Time to first screen:** ~5 seconds (auth + WebSocket setup + tmux + TUI startup).

### 4.2 Android First-Run (Termux)

```
1. User installs Termux from F-Droid (not Play Store — the Play Store version is outdated)
2. User runs the setup script:
   curl -sL https://raw.githubusercontent.com/.../wg-termux-setup.sh | bash
3. Script installs mosh, tmux, openssh; creates ~/.shortcuts/wg
4. User edits the shortcut to set their server address + SSH key
5. User taps the shortcut (or runs it manually):
   mosh user@server -- tmux new-session -A -s $USER-wg "wg tui"
6. TUI renders in Termux — Termux touch mode auto-detected
7. (Optional) Install Termux:Widget for home screen shortcut
```

**Time to first screen:** ~10 minutes (install + setup + SSH key exchange). Subsequent launches: ~3 seconds.

### 4.3 iOS First-Run (Blink Shell)

```
1. User installs Blink Shell from App Store ($15.99)
2. User configures a host in Blink:
   Host: wg-server
   Hostname: server-ip-or-domain
   User: username
   Key: (import or generate SSH key)
   Mosh: enabled
3. User creates a Blink command or snippet:
   mosh wg-server -- tmux new-session -A -s $USER-wg "wg tui"
4. User runs the command → TUI renders in Blink
5. Blink supports custom fonts and themes for readability
```

**Time to first screen:** ~15 minutes (purchase + install + SSH setup). Subsequent launches: ~3 seconds.

### 4.4 Desktop First-Run (reference)

```
1. User has SSH key access to server
2. mosh user@server -- tmux new-session -A -s $USER-wg "wg tui"
3. Done.
```

**Time to first screen:** ~2 seconds.

---

## 5. Dependencies & Third-Party Components

### Server-Side (required)

| Component | Version | License | Purpose |
|-----------|---------|---------|---------|
| tmux | 3.x | ISC | Session persistence |
| mosh | 1.4+ | GPL-3.0 | UDP transport, network resilience |
| wg (wg) | current | MIT | The application |

### Server-Side (web access only)

| Component | Version | License | Purpose |
|-----------|---------|---------|---------|
| ttyd | 1.7+ | MIT | Terminal → WebSocket bridge |
| Caddy | 2.x | Apache-2.0 | Reverse proxy, TLS, auth |

### Client-Side

| Platform | Component | License | Cost |
|----------|-----------|---------|------|
| Web | Modern browser | — | Free |
| Android | Termux | GPL-3.0 | Free |
| Android | Termux:Widget (optional) | GPL-3.0 | Free |
| iOS | Blink Shell | — | $15.99 |
| Desktop | mosh client | GPL-3.0 | Free |

### No Custom Code Required

The entire stack is assembled from existing, maintained open-source components (except Blink Shell which is proprietary). No custom Android app, iOS app, or web frontend is needed for the initial deployment.

---

## 6. Feasibility Summary

| Platform | Feasibility | Rationale |
|----------|-------------|-----------|
| **Web (desktop browser)** | **5/5** | ttyd + Caddy, zero code changes, production-ready today |
| **Web (mobile browser / PWA)** | **4/5** | Same as desktop web but on-screen keyboard UX is mediocre |
| **Android (Termux + mosh)** | **4/5** | Works today, Termux already detected in TUI code. Small-screen layout needs work |
| **iOS (Blink + mosh)** | **3/5** | Works but requires paid app. No free mosh-capable iOS terminal exists |
| **Desktop (SSH/mosh)** | **5/5** | Already the primary access method |

### Risk Summary

| Risk | Severity | Mitigation |
|------|----------|------------|
| Termux removed from Play Store | Low | F-Droid is the canonical source; Termux is actively maintained |
| Blink Shell discontinued | Low | Source is available; alternatives may emerge |
| ttyd security vulnerability | Medium | Run behind reverse proxy, keep updated, monitor CVEs |
| Small-screen TUI unusable | Medium | Implement responsive breakpoints (separate task) |
| iOS kills background mosh | Low | mosh reconnects automatically; inherent to iOS model |

---

## 7. Recommended Implementation Order

### Phase 1: Web Access (effort: 1 day)
1. Document ttyd + Caddy deployment in `docs/guides/`
2. Test TUI rendering in xterm.js (mouse events, colors, resize)
3. Create a `docker-compose.yml` or deploy script for one-command setup

### Phase 2: Mobile Documentation (effort: 1 day)
1. Write Termux setup guide with the `wg-termux-setup.sh` script
2. Write Blink Shell configuration guide
3. Test TUI on small screens, document minimum viable terminal size

### Phase 3: Responsive TUI (effort: 3-5 days)
1. Implement screen-size breakpoints in `render.rs`
2. Single-panel mode for < 50 columns
3. Test on Termux (phone) and Blink (iPad vs iPhone)

### Phase 4: PWA & Polish (effort: 2-3 days)
1. Create PWA manifest for ttyd web frontend
2. Add service worker for offline "reconnecting" UX
3. Custom ttyd theme matching wg branding

### Phase 5: Custom Apps (defer)
- Only if user demand warrants the maintenance burden
- Android: Kotlin app wrapping terminal emulator + built-in mosh
- iOS: Swift app wrapping SwiftTerm + mosh

---

## 8. Decisions for Downstream Tasks

These decisions should be carried forward to `mu-design-synthesis`:

1. **ttyd is the web access layer** — no custom web frontend needed initially
2. **tmux is the universal session layer** — all platforms connect through tmux
3. **mosh is the mobile transport** — critical for Android/iOS, not applicable to web
4. **No custom mobile apps initially** — Termux (Android) and Blink Shell (iOS) are sufficient
5. **Responsive TUI is a prerequisite** — small-screen support is needed before mobile is truly viable
6. **Authentication is at the reverse proxy layer** — not in wg itself
7. **The screen_dump.rs IPC** (`tui.sock`) is a future integration point for richer web UIs but is not needed for Phase 1
