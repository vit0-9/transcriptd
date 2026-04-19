# Security Policy

## Supported Versions

We provide security updates for the **latest `main` branch** and the **latest tagged release**.

| Version | Supported |
| ------- | --------- |
| Latest release (`v*`) | ✅ |
| `main` branch | ✅ |
| Older releases | ❌ |
| Development branches/forks | ❌ |

## Reporting a Vulnerability

If you discover a security vulnerability in `transcriptd`, please report it privately. **Do not create a public GitHub Issue.**

To report a vulnerability, please use **GitHub Security Advisories**:

1. Go to the [Security Advisories tab](https://github.com/vit0-9/transcriptd/security) in this repository.
2. Click **Report a Vulnerability**.
3. Provide as much detail as possible.

Alternatively, you can email <info@transcriptd.dev> directly.

### What to include in your report

Please include:

* A description of the vulnerability and its impact
* Product version, OS, compiler version (if applicable)
* Steps to reproduce the issue, or a proof-of-concept
* Suggested remediation (optional but appreciated)

### Response timeline

* **Acknowledgment**: We aim to acknowledge receipt of your vulnerability report within **48 hours**.
* **Triage**: We aim to complete initial triage within **7 days**.
* **Disclosure**: We will coordinate with you on public disclosure after a fix has been rolled out.

## Scope

**In Scope:**

* The core `transcriptd` daemon, CLI, and TUI implementation in this repository
* The MCP server implementation (`src/mcp.rs`)
* The extraction adapters (`crates/transcriptd-*`)
* Inter-process communication logic (`src/ipc.rs`)
* Vulnerabilities in the shipped GitHub Actions CI pipelines (e.g. supply chain attacks)

**Out of Scope:**

* Vulnerabilities in the underlying IDEs (Zed, VS Code, Claude Code)
* Issues arising from local machine compromise (e.g. if root access was gained independently, the attacker can already read the AI databases directly)
* Features marked explicitly as experimental or prototype without production intent
* Unofficial forks or third-party builds

## Common Weaknesses

When investigating `transcriptd`, please be mindful of these areas:
* **Paths and symlinks:** Extractors must not follow malicious symlinks or allow path traversal outside the user's workspace.
* **Daemon exposure:** The MCP HTTP endpoint and Unix domain socket must not inadvertently expose sensitive transcript data across OS user boundaries.
* **XSS in TUI:** Though terminal-based, maliciously crafted input from AI platforms should not lead to terminal escape sequence execution.
* **SQL Injection:** Though we use parameter binding in `transcriptd-store`, `LIKE` / `GLOB` bounds in FTS queries should remain strictly sanitized.
