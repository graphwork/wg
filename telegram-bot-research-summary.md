# Telegram Bot Configuration Research Summary

## Overview
The wg repository **has a fully functional Telegram bot configured and working**. The bot is set up for user communication and task coordination.

## Bot Configuration

### Location
The Telegram bot configuration is located in the **global config file**:
- **Path**: `~/.config/wg/notify.toml`
- **No project-local configuration** found in `.wg/notify.toml`

### Current Configuration
```toml
[routing]
default = ["telegram"]
urgent = ["telegram"]
approval = ["telegram"]

[escalation]
approval_timeout = 1800
urgent_timeout = 3600

[telegram]
bot_token = "***REMOVED***"
chat_id = "107103998"
```

### Bot Details
- **Bot Token**: `***REMOVED***`
- **Chat ID**: `107103998` (this is Erik's personal chat)
- **Status**: ✅ **FUNCTIONAL** - Test message sent successfully

## How Agents Should Use the Bot

### CLI Commands Available
Agents can interact with the Telegram bot using these `wg telegram` commands:

1. **Send messages to Erik**:
   ```bash
   wg telegram send "Your message here"
   wg telegram send "Message" --chat-id "custom_chat_id"  # Optional custom chat
   ```

2. **Check bot status**:
   ```bash
   wg telegram status           # Human readable
   wg telegram status --json    # JSON format
   ```

3. **Start listener** (for interactive commands from Erik):
   ```bash
   wg telegram listen           # Use configured chat_id
   wg telegram listen --chat-id "custom_chat_id"  # Custom chat
   ```

### Supported wg Commands via Telegram
When the listener is running, Erik can send these commands via Telegram:
- `claim <task>` / `claim <task> as <actor>` - Claim tasks
- `done <task>` - Mark tasks complete  
- `fail <task> [reason]` - Mark tasks failed
- `input <task> <text>` - Add log entries
- `unclaim <task>` - Release tasks
- `ready` - List ready tasks
- `status` - Project status
- `help` - Show help

Commands can be prefixed with `wg` if needed (e.g., `wg claim task-1`).

### Interactive Features
- **Button Actions**: The bot supports interactive buttons for approve/reject/claim actions
- **Bidirectional**: Both outbound notifications AND inbound command processing
- **Rich Messages**: Supports Markdown and HTML formatting

## Architecture Notes

### Notification Routing
The bot is configured as the **default channel** for:
- **Default notifications**: General task updates
- **Urgent alerts**: High-priority events (3600s escalation timeout)
- **Approval requests**: Human-in-the-loop approvals (1800s escalation timeout)

### Implementation Details
- **Backend**: Direct Telegram Bot API via `reqwest` (not teloxide)
- **Polling**: Uses long-polling with `getUpdates` endpoint
- **Global Routing**: Supports shared bot across multiple wg repos via file-lock leader election
- **Message Format**: Plain text, Markdown, and HTML support
- **Action Buttons**: Inline keyboards for interactive approvals

### Configuration Priority
1. **Project-local**: `.wg/notify.toml` (not present)
2. **Global**: `~/.config/wg/notify.toml` (✅ active)

## Agent Usage Recommendations

### For Escalation/Alerts
```bash
# Send urgent notification to Erik
wg telegram send "🚨 Task failed: build-system-broken - requires immediate attention"
```

### For Status Updates  
```bash
# Send progress update
wg telegram send "📋 Task completed: implement-feature-x - deployed successfully"
```

### For Approval Requests
Agents should use the notification system which will automatically route approval requests to Telegram with interactive buttons.

## Verification Status
- ✅ **Configuration located and documented**  
- ✅ **Bot token and chat ID identified**
- ✅ **Functionality confirmed** (test message sent successfully)
- ✅ **CLI commands documented**
- ✅ **Integration patterns identified**

## Missing Pieces
**None** - The Telegram bot is fully configured and operational. Agents have everything needed to contact Erik via Telegram.