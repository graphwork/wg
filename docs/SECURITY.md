# Security Guide

## Secret Management

### Overview

This project follows a defense-in-depth approach to prevent secret leakage:

1. **Pre-commit hooks** scan for secrets before they can be committed
2. **GitIgnore rules** prevent common secret-containing files from being tracked
3. **GitGuardian configuration** provides customizable secret detection
4. **Environment-based configuration** keeps secrets out of source code

### Secret Detection

#### Pre-commit Hook

A pre-commit hook automatically scans staged files for secrets using GitGuardian's `ggshield`. This prevents secrets from being committed in the first place.

**Installation**: The hook is automatically installed when you run `scripts/install-pre-commit-hook.sh`.

**Requirements**: Install ggshield with:
```bash
pip install ggshield
```

**Enhanced detection**: Set your GitGuardian API key for cloud-based detection:
```bash
export GITGUARDIAN_API_KEY=your_api_key_here
```

#### GitGuardian Configuration

The `.gitguardian.yml` file configures secret detection rules:

- **Paths scanned**: All files except those in `.wg/`, `target/`, etc.
- **File size limit**: 1MB per file
- **Output format**: Human-readable text
- **False positives**: Can be configured in the `secrets-ignore` section

### Secure Configuration

#### Environment Variables

Use environment variables for sensitive configuration:

```bash
# Export in your shell profile or use a .env file (NOT committed)
export GITGUARDIAN_API_KEY=your_api_key
export OPENROUTER_API_KEY=your_openrouter_key
export CLAUDE_API_KEY=your_claude_key
```

#### Configuration Files

For structured configuration with secrets:

1. **Create template files** (committed) with placeholder values:
   ```toml
   # notify.toml.template
   [telegram]
   bot_token = "YOUR_BOT_TOKEN_HERE"
   chat_id = "YOUR_CHAT_ID_HERE"
   ```

2. **Copy and customize** locally:
   ```bash
   cp notify.toml.template notify.toml
   # Edit notify.toml with real values
   ```

3. **Ensure exclusion**: The real config files are in `.gitignore`

#### Workgraph Notification Configuration

The `notify.toml` file contains notification secrets and is automatically excluded from git:

```toml
[telegram]
bot_token = "your_telegram_bot_token"
chat_id = "your_chat_id"

[email]
smtp_password = "your_email_password"
```

### Protected File Patterns

The following file patterns are automatically ignored by git:

- `.env` and `.env.*` files
- Certificate files: `*.pem`, `*.key`, `*.p12`, `*.pfx`
- Credential files: `credentials.json`, `secrets.yaml`, etc.
- API key files: `api-keys.txt`, `tokens.txt`
- Secret directories: `**/secrets/`, `.secrets/`
- Configuration with secrets: `auth.toml`, `config/secrets.toml`

### GitHub Integration

#### Secret Scanning

Enable GitHub's secret scanning alerts:

1. Go to your repository settings
2. Navigate to "Security & analysis"
3. Enable "Secret scanning alerts"
4. Enable "Push protection" to prevent pushes with secrets

#### Dependabot Security Updates

Enable Dependabot for automatic security updates:

1. Go to repository settings → "Security & analysis"
2. Enable "Dependabot security updates"
3. Configure `.github/dependabot.yml` if needed

### Best Practices

#### For Developers

1. **Never hardcode secrets** in source files
2. **Use environment variables** or external config files
3. **Test the pre-commit hook** before relying on it
4. **Review .gitignore** when adding new config files
5. **Set up your local environment** with proper secret management

#### For CI/CD

1. **Use GitHub Secrets** for workflow secrets
2. **Limit secret scope** to minimum required permissions
3. **Rotate secrets regularly**
4. **Monitor for exposed secrets** in logs and artifacts

#### Testing Secret Detection

To test that the pre-commit hook catches secrets:

1. Create a test file with a fake secret:
   ```bash
   echo "api_key = sk-fake_key_for_testing_secret_detection" > test-secret.txt
   git add test-secret.txt
   ```

2. Try to commit:
   ```bash
   git commit -m "test secret detection"
   ```

3. The hook should block the commit and show the detected secret

4. Clean up:
   ```bash
   git reset HEAD test-secret.txt
   rm test-secret.txt
   ```

### Incident Response

If secrets are accidentally committed:

1. **Immediately rotate** the exposed secret
2. **Remove from git history**:
   ```bash
   # For recent commits
   git reset --soft HEAD~1
   git reset HEAD path/to/file-with-secret
   # Edit file to remove secret
   git add path/to/file-with-secret
   git commit -m "remove secret"
   
   # For older commits, use git-filter-repo or BFG
   ```

3. **Force push** (if safe) to update remote history
4. **Review access logs** for the exposed secret
5. **Update security measures** to prevent recurrence

### Tools and Resources

- **GitGuardian CLI**: https://docs.gitguardian.com/ggshield-docs/
- **GitHub Secret Scanning**: https://docs.github.com/en/code-security/secret-scanning
- **BFG Repo-Cleaner**: https://rtyley.github.io/bfg-repo-cleaner/
- **git-secrets**: https://github.com/awslabs/git-secrets