# Security Policy

## Reporting a Vulnerability

If you discover a security issue in CopyDeck — for example, a way for
another local process to read clipboard history from outside the
`~/.local/share/copydeck/` directory, an IPC socket exposure, or a
privilege escalation via the systemd service — please **do not open a
public issue**.

Instead, use GitHub's private reporting:

1. Go to the
   [Security tab](https://github.com/rahul-anb/Copydeck/security/advisories/new)
   of this repository.
2. Click **Report a vulnerability**.
3. Include steps to reproduce, affected versions, and the impact.

I aim to acknowledge reports within 7 days. This is a personal side
project, so response times may vary — but security issues take priority
over everything else.

## Supported versions

Only the latest published release on PyPI is actively supported. Older
versions will not receive fixes.
