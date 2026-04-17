# Native Executor Security Hardening Analysis

## Executive Summary

This document analyzes the current native executor implementation for security vulnerabilities and proposes hardening measures. The previous `harden-native-executor` task (commit cd7aa490) successfully resolved test flakiness through improved worktree isolation, but did not address fundamental security hardening of the execution environment.

## Current Security Posture

### Strengths
- **Tool output truncation**: Prevents context overflow with configurable limits
- **Timeout controls**: Commands are bounded by configurable timeouts (5-15 minutes)
- **Concurrent execution limits**: Read-only tools parallelized safely, mutating tools serialized
- **Working directory isolation**: Tools execute in controlled working directory
- **Provider abstraction**: Clean separation between LLM providers and execution

### Critical Vulnerabilities

#### 1. Command Injection (CRITICAL)
**Location**: `src/executor/native/tools/bash.rs`
**Risk**: Arbitrary command execution

The bash tool directly executes user-provided commands without sanitization:
```rust
Command::new("bash")
    .arg("-c")
    .arg(command)  // Direct user input execution
```

**Attack vectors**:
- Shell metacharacters: `; rm -rf /`
- Command substitution: `$(malicious_command)`
- Process substitution: `<(malicious_command)`
- Pipe chains: `legitimate_cmd | malicious_cmd`

#### 2. Unrestricted File System Access (HIGH)
**Location**: `src/executor/native/tools/file.rs`
**Risk**: Data exfiltration, file system corruption

File tools can access any readable/writable files:
- No path traversal prevention (`../../../etc/passwd`)
- No sensitive file protection (`/home/.ssh/`, `/etc/shadow`)
- No file size limits for writes
- No disk quota enforcement

#### 3. Unrestricted Network Access (HIGH)
**Location**: `src/executor/native/tools/web_search.rs`
**Risk**: Data exfiltration, SSRF attacks

Web search tool can access any external URLs:
- No domain allowlists
- No private network protection (SSRF to `127.0.0.1`, `10.0.0.0/8`)
- No request rate limiting
- No response size limits

#### 4. Resource Exhaustion (MEDIUM)
**Locations**: Various tool implementations
**Risk**: Denial of service

No comprehensive resource limits:
- Memory usage unbounded
- CPU usage unbounded (beyond timeout)
- Disk usage unbounded
- Network bandwidth unbounded

#### 5. Insufficient Input Validation (MEDIUM)
**Locations**: All tool implementations
**Risk**: Various injection attacks

Tool inputs lack comprehensive validation:
- Path parameters not validated for traversal
- Command parameters not sanitized
- JSON inputs not strictly validated
- No input size limits

## Proposed Hardening Measures

### Phase 1: Command Injection Prevention (CRITICAL)

#### 1.1 Secure Command Execution
Replace direct bash execution with structured command builders:

```rust
// Instead of: bash -c "$USER_INPUT"
// Use secure command parsing and validation
pub struct SecureCommandBuilder {
    allowed_commands: HashSet<String>,
    allowed_args: Vec<Regex>,
    env_allowlist: HashSet<String>,
}

impl SecureCommandBuilder {
    pub fn parse_and_validate(command: &str) -> Result<ValidatedCommand> {
        // Parse command into components
        // Validate each component against allowlists
        // Reject shell metacharacters
        // Return structured command or error
    }
}
```

#### 1.2 Command Allow-listing
Implement configurable command allowlists:
- Default safe commands: `cargo`, `git`, `wg`, `ls`, `cat`, `grep`
- Configurable per-project allowlists
- Regex-based argument validation
- Environment variable filtering

### Phase 2: File System Sandboxing (HIGH)

#### 2.1 Path Validation and Restriction
```rust
pub struct FileSystemSandbox {
    allowed_read_paths: Vec<PathBuf>,
    allowed_write_paths: Vec<PathBuf>,
    forbidden_patterns: Vec<Regex>,
    max_file_size: usize,
}

impl FileSystemSandbox {
    pub fn validate_path(&self, path: &Path, operation: FileOperation) -> Result<PathBuf> {
        // Canonicalize path to prevent traversal
        // Check against allowed/forbidden lists
        // Validate file size limits
    }
}
```

#### 2.2 File Operation Monitoring
- Log all file operations for audit
- Implement file size quotas per task
- Monitor disk usage in real-time
- Automatic cleanup of temporary files

### Phase 3: Network Access Controls (HIGH)

#### 3.1 Network Sandbox
```rust
pub struct NetworkSandbox {
    allowed_domains: HashSet<String>,
    blocked_cidrs: Vec<IpNet>,  // Block private networks
    max_request_size: usize,
    max_response_size: usize,
    rate_limits: RateLimiter,
}
```

#### 3.2 SSRF Prevention
- Block private IP ranges (10.0.0.0/8, 192.168.0.0/16, 127.0.0.0/8)
- Block metadata endpoints (169.254.169.254)
- Validate DNS resolution before requests
- Implement request timeout and size limits

### Phase 4: Resource Management (MEDIUM)

#### 4.1 Resource Limits
```rust
pub struct ResourceLimits {
    max_memory: usize,          // 512MB default
    max_cpu_time: Duration,     // 300s default
    max_disk_usage: usize,      // 1GB default
    max_network_bandwidth: usize, // 10MB/s default
    max_open_files: usize,      // 1000 default
}
```

#### 4.2 Resource Monitoring
- Real-time memory usage tracking
- CPU time accounting per tool execution
- Disk usage monitoring with automatic cleanup
- Network bandwidth throttling

### Phase 5: Enhanced Input Validation (MEDIUM)

#### 5.1 Strict JSON Schema Validation
- Define comprehensive schemas for all tool inputs
- Validate string lengths and patterns
- Sanitize and escape special characters
- Implement input size limits

#### 5.2 Parameter Sanitization
```rust
pub trait ParameterSanitizer {
    fn sanitize_path(path: &str) -> Result<PathBuf>;
    fn sanitize_command(cmd: &str) -> Result<String>;
    fn sanitize_url(url: &str) -> Result<Url>;
    fn sanitize_json(json: &Value) -> Result<Value>;
}
```

## Implementation Roadmap

### Immediate Actions (Week 1)
1. Implement command allowlisting for bash tool
2. Add path traversal prevention for file tools
3. Block private networks in web search tool
4. Add comprehensive logging for all tool operations

### Short Term (Weeks 2-4)
1. Implement resource limits and monitoring
2. Add file system sandboxing with quotas
3. Enhance input validation across all tools
4. Create security configuration framework

### Long Term (Months 2-3)
1. Add runtime security policy enforcement
2. Implement advanced anomaly detection
3. Create security audit and compliance reports
4. Add integration with external security tools

## Risk Assessment Matrix

| Vulnerability | Likelihood | Impact | Risk Level | Effort to Fix |
|---------------|------------|---------|------------|---------------|
| Command Injection | High | Critical | CRITICAL | Medium |
| File System Access | Medium | High | HIGH | Medium |
| Network SSRF | Medium | High | HIGH | Low |
| Resource Exhaustion | Low | Medium | MEDIUM | High |
| Input Validation | Medium | Medium | MEDIUM | Low |

## Configuration Framework

Proposed security configuration in `.workgraph/security.toml`:

```toml
[command_execution]
enabled = true
allowed_commands = ["cargo", "git", "wg", "ls", "cat", "grep"]
blocked_patterns = [";", "|", "&", "`", "$"]
timeout_ms = 300000

[file_system]
max_read_size_mb = 100
max_write_size_mb = 50
allowed_read_paths = [".", ".workgraph"]
allowed_write_paths = [".", ".workgraph", "/tmp/workgraph-*"]
forbidden_patterns = ["../", "/etc/", "/home/.ssh/"]

[network]
enabled = true
allowed_domains = ["*.github.com", "api.anthropic.com"]
blocked_cidrs = ["10.0.0.0/8", "192.168.0.0/16", "127.0.0.0/8"]
max_request_size_mb = 10
max_response_size_mb = 100

[resources]
max_memory_mb = 512
max_cpu_time_sec = 300
max_disk_usage_mb = 1000
max_open_files = 1000
```

## Backward Compatibility

All proposed changes maintain backward compatibility through:
- Feature flags for gradual rollout
- Default configurations that preserve current behavior
- Opt-in security modes for enhanced protection
- Clear migration paths for existing workflows

## Security Testing Strategy

1. **Static Analysis**: Integrate with security linters
2. **Dynamic Testing**: Automated penetration testing
3. **Fuzzing**: Input validation stress testing
4. **Compliance**: Regular security audits
5. **Monitoring**: Runtime security event logging

## Conclusion

The native executor requires comprehensive security hardening to address critical vulnerabilities. The proposed measures provide defense-in-depth protection while maintaining usability and backward compatibility. Implementation should prioritize command injection prevention and file system sandboxing as the highest-impact security improvements.